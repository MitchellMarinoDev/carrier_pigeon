use crate::connection::ping_system::ClientPingSystem;
use crate::connection::reliable::ReliableSystem;
use crate::message_table::{DISCONNECT_M_TYPE, PING_M_TYPE, RESPONSE_M_TYPE};
use crate::messages::{NetMsg, PingMsg, PingType};
use crate::net::{AckNum, ErasedNetMsg, MsgHeader, HEADER_SIZE, Message, Status};
use crate::transport::ClientTransport;
use crate::{ClientConfig, MsgTable, Response};
use log::{debug, error, trace, warn};
use std::any::TypeId;
use std::io;
use std::io::{Error, ErrorKind};
use std::net::SocketAddr;
use std::sync::Arc;

/// [`ReliableSystem`] with the generic parameters set for a server.
type ClientReliableSystem<C, A, R, D> = ReliableSystem<Arc<Vec<u8>>, Box<dyn NetMsg>, C, A, R, D>;

/// A wrapper around the the [`ClientTransport`] that adds the reliability and ordering.
pub(crate) struct ClientConnection<T: ClientTransport, C: NetMsg, A: NetMsg, R: NetMsg, D: NetMsg> {
    /// The configuration of the client.
    config: ClientConfig,
    /// The [`MsgTable`] to use for sending and receiving messages.
    msg_table: MsgTable<C, A, R, D>,
    /// The status of the client. Whether it is connected/disconnected etc.
    status: Status<A, R, D>,
    /// The [`Transport`] to use to send and receive the messages, if the connection is open.
    transport: Option<T>,
    /// The system used to generate ping messages and estimate the RTT.
    ping_sys: ClientPingSystem,
    /// The [`ReliableSystem`] to add optional reliability to messages.
    reliable_sys: ClientReliableSystem<C, A, R, D>,
    /// The received message buffer.
    ///
    /// Each [`MType`](crate::MType) has its own vector.
    msg_buf: Vec<Vec<ErasedNetMsg>>,
}

impl<T: ClientTransport, C: NetMsg, A: NetMsg, R: NetMsg, D: NetMsg>
    ClientConnection<T, C, A, R, D>
{
    pub fn new(config: ClientConfig, msg_table: MsgTable<C, A, R, D>) -> Self {
        Self {
            config,
            msg_table: msg_table.clone(),
            status: Status::NotConnected,
            transport: None,
            ping_sys: ClientPingSystem::new(),
            msg_buf: (0..msg_table.mtype_count()).map(|_| vec![]).collect(),
            reliable_sys: ReliableSystem::new(msg_table),
        }
    }

    // TODO: make a custom error type. Add invalid state.
    pub fn connect(&mut self, local_addr: SocketAddr, peer_addr: SocketAddr, con_msg: &C) -> io::Result<()> {
        if !self.status.is_not_connected() {
            return Err(Error::new(
                ErrorKind::Other,
                "the client needs to be in the NotConnected status in order to call connect()",
            ));
        }

        let transport = T::new(local_addr, peer_addr)?;
        trace!(
            "ClientConnection created from {} to {}",
            transport
                .local_addr()
                .map(|addr| addr.to_string())
                .unwrap_or("UNKNOWN".to_owned()),
            transport
                .peer_addr()
                .map(|addr| addr.to_string())
                .unwrap_or("UNKNOWN".to_owned()),
        );

        // clean up from last connection
        self.ping_sys = ClientPingSystem::new();
        self.reliable_sys = ReliableSystem::new(self.msg_table.clone());
        for mut buf in self.msg_buf {
            buf.clear();
        }

        self.status = Status::Connecting;
        self.transport = Some(transport);
        self.send(con_msg)?;

        Ok(())
    }

    /// Disconnects from the server. You should call this method before dropping
    /// the client to let the server know that you intentionally disconnected.
    /// The `discon_msg` allows you to give a reason for the disconnect.
    pub fn disconnect(&mut self, discon_msg: &D) -> io::Result<()> {
        // TODO: change to custom error type.
        if !self.status.is_connected() {
            return Err(Error::new(
                ErrorKind::NotConnected,
                "Client is not connected.",
            ));
        }
        debug!("Client disconnecting from server.");
        match self.send(discon_msg) {
            Ok(ack_num) => self.status = Status::Disconnecting(ack_num),
            Err(err) => self.status_err(err),
        }
        Ok(())
    }

    // TODO: rework to not fail due to the transport. Only due to passing in a wrong message type.
    //      Then a custom error type may be helpful.
    pub fn send<M: NetMsg>(&mut self, msg: &M) -> io::Result<AckNum> {
        // TODO: convert to a custom error type?
        // TODO: fail if not connected for all.
        let transport = match &mut self.transport {
            Some(t) => t,
            None => {
                return Err(Error::new(
                    ErrorKind::NotConnected,
                    "Client is not connected",
                ))
            }
        };

        // verify type is valid
        self.msg_table.check_type::<M>()?;
        let tid = TypeId::of::<M>();

        // create the message header
        let m_type = self.msg_table.tid_map[&tid];
        let header = self.reliable_sys.get_send_header(m_type);

        // build the payload using the header and the message
        let mut payload = header.to_be_bytes().to_vec();

        let ser_fn = self.msg_table.ser[m_type];
        ser_fn(msg, &mut payload)?;
        let payload = Arc::new(payload);

        // send the payload based on the guarantees
        let guarantees = self.msg_table.guarantees[m_type];
        self.reliable_sys.save(header, guarantees, payload.clone());
        self.status_result(transport.send(m_type, payload));
        Ok(header.sender_ack_num)
    }

    /// Updates the status of the connection based on the error.
    fn status_err(&mut self, err: Error) {
        match &self.status {
            Status::Connected => self.status = Status::Dropped(err),
            Status::Connecting | Status::Accepted(_) | Status::Rejected(_) => self.status = Status::ConnectionFailed(err),
            Status::Disconnecting(_) => self.status = Status::NotConnected,
            _ => {}
        }
        self.transport = None;
    }

    /// Updates the status of the connection if there is an error.
    fn status_result(&mut self, result: io::Result<()>) {
        if let Err(err) = result {
            self.status_err(err);
        }
    }

    /// Sends an [`AckMsg`] to acknowledge all received messages.
    pub fn send_ack_msg(&mut self) {
        let ack_msg = match self.reliable_sys.get_ack_msg() {
            None => return,
            Some(ack_msg) => ack_msg,
        };

        if let Err(err) = self.send(&ack_msg) {
            error!("Error sending AckMsg: {}", err);
        }
    }

    /// Sends a ping message to the server if necessary.
    pub fn send_ping(&mut self) {
        if let Some(msg) = self.ping_sys.get_ping_msg() {
            if let Err(err) = self.send(&msg) {
                error!("Failed to send ping message: {}", err);
            }
        }
    }

    /// Gets an iterator for the messages of type `M`.
    ///
    /// ### Panics
    /// Panics if the type `M` was not registered.
    /// For a non-panicking version, see [try_get_msgs()](Self::try_get_msgs).
    pub fn recv<M: NetMsg>(&self) -> impl Iterator<Item = Message<M>> + '_ {
        self.msg_table.check_type::<M>().expect(
            "`get_msgs` panics if generic type `M` is not registered in the MsgTable. \
            For a non panicking version, use `try_get_msgs`",
        );
        let tid = TypeId::of::<M>();
        let m_type = self.msg_table.tid_map[&tid];

        self.msg_buf[m_type].iter().map(|m| m.get_typed().unwrap())
    }

    /// Gets an iterator for the messages of type `M`.
    ///
    /// Returns `None` if the type `M` was not registered.
    pub fn try_recv<M: NetMsg>(&self) -> Option<impl Iterator<Item = Message<M>> + '_> {
        let tid = TypeId::of::<M>();
        let m_type = *self.msg_table.tid_map.get(&tid)?;

        Some(self.msg_buf[m_type].iter().map(|m| m.get_typed().unwrap()))
    }

    /// Gets all outstanding messages from the [`Transport`], and adds them to an internal buffer.
    ///
    /// To get the actual messages, use [`recv`](Self::recv).
    pub fn get_msgs(&mut self) {
        match self.get_msgs_err() {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::WouldBlock => {}
            Err(err) => {
                self.status_err(err);
            }
        }
    }

    /// Gets all the outstanding messages from the [`Transport`] and adds them to the `self.ready`
    /// buffer. Any errors other than a [`WouldBlock`](ErrorKind::WouldBlock) are treated as
    /// unrecoverable errors and therefor close the connection.
    fn get_msgs_err(&mut self) -> io::Result<()> {
        // TODO: support blocking somehow.
        loop {
            let buf = match &mut self.transport {
                None => return Ok(()),
                Some(t) => t.recv()?,
            };

            let n = buf.len();
            if n < HEADER_SIZE {
                warn!(
                    "Client: Received a packet of length {} which is not big enough \
                    to be a carrier pigeon message. Discarding",
                    n
                );
                continue;
            }
            let header = MsgHeader::from_be_bytes(&buf[..HEADER_SIZE]);
            if !self.msg_table.valid_m_type(header.m_type) {
                warn!(
                    "Client: Received a message with an invalid MType: {}, Maximum MType is {}",
                    header.m_type,
                    self.msg_table.mtype_count()
                );
            }

            trace!(
                "Client: received message (MType: {}, len: {}, AckNum: {})",
                header.m_type,
                n,
                header.sender_ack_num,
            );

            let msg = match self.msg_table.deser[header.m_type](&buf[HEADER_SIZE..]) {
                Ok(msg) => msg,
                Err(err) => {
                    warn!("{}", err);
                    continue;
                }
            };

            match header.m_type {
                // TODO: Add other special types.
                PING_M_TYPE => {
                    let msg: PingMsg = *msg.downcast().expect("since the MType is `DISCONNECT_M_TYPE`, the message should be the disconnection type");
                    match msg.ping_type {
                        PingType::Req => {
                            if let Err(err) = self.send(&msg.response()) {
                                warn!("Error in responding to a ping: {}", err);
                            }
                        }
                        PingType::Res => {
                            self.ping_sys.recv_ping_msg(msg.ping_num);
                        }
                    }
                }
                DISCONNECT_M_TYPE => {
                    if self.status.is_connected() {
                        self.status = Status::Disconnected(*msg.downcast().expect("since the MType is `DISCONNECT_M_TYPE`, the message should be the disconnection type"));
                    }
                }
                RESPONSE_M_TYPE => {
                    if self.status.is_connecting() {
                        match *msg.downcast::<Response<A, R>>().expect("since the MType is `RESPONSE_M_TYPE`, the message should be the response type") {
                            Response::Accepted(a) => self.status = Status::Accepted(a),
                            Response::Rejected(r) => self.status = Status::Rejected(r),
                        }
                    }
                }
                _ => {
                    self.msg_buf[header.m_type].push(ErasedNetMsg::new(
                        0,
                        header.sender_ack_num,
                        header.order_num,
                        msg,
                    ));
                }
            }

            // handle reliability and ordering
            self.reliable_sys.push_received(header, msg);
            // get all messages from the reliable system and push them on the "ready" que.
            while let Some((header, msg)) = self.reliable_sys.get_received() {
                self.msg_buf[header.m_type].push(ErasedNetMsg::new(0, header.sender_ack_num, header.order_num, msg));
            }
        }
    }

    /// This handles everything that the client needs to do each frame.
    ///
    /// This includes:
    ///
    ///  - Clearing the message buffer. This gets rid of all the messages from last frame.
    ///  - Getting the messages for this frame.
    ///  - Resending messages that are needed for the reliability layer.
    ///  - Updating the status.
    pub fn tick(&mut self) {
        self.clear_msgs();
        self.send_ack_msg();
        self.send_ping();
        self.resend_reliable();
        self.get_msgs();
        self.update_status();
    }

    /// Resends any messages that it needs to for the reliability system to work.
    pub fn resend_reliable(&mut self) {
        for (header, payload) in self.reliable_sys.get_resend() {
            if let Some(transport) = &self.transport {
                self.status_result(transport.send(header.m_type, payload.clone()));
            }
        }
    }

    pub fn handle_status(&mut self) -> Status<A, R, D> {
        use Status::*;

        let new_status = match &self.status {
            NotConnected => NotConnected,
            Connecting => Connecting,
            Accepted(_) => Connected,
            Rejected(_) => NotConnected,
            ConnectionFailed(_) => NotConnected,
            Connected => Connected,
            Disconnected(_) => NotConnected,
            Dropped(_) => NotConnected,
            Disconnecting(ack_num) => Disconnecting(*ack_num),
        };

        std::mem::replace(&mut self.status, new_status)
    }

    /// Gets the status of the connection.
    pub fn get_status(&self) -> &Status<A, R, D> {
        &self.status
    }

    fn update_status(&mut self) {
        if let Status::Disconnecting(ack_num) = self.status {
            // TODO: if the disconnection message has been acknowledged, move states.
        }
    }

    /// Gets the [`NetConfig`] of the client.
    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    /// Gets the [`MsgTable`] of the client.
    pub fn msg_table(&self) -> &MsgTable<C, A, R, D> {
        &self.msg_table
    }

    /// Clears messages from the buffer.
    fn clear_msgs(&mut self) {
        for buf in self.msg_buf.iter_mut() {
            buf.clear();
        }
    }

    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.transport.as_ref()?.local_addr().ok()
    }

    pub fn peer_addr(&self) -> Option<SocketAddr> {
        self.transport.as_ref()?.peer_addr().ok()
    }

    pub fn rtt(&self) -> u32 {
        self.ping_sys.rtt()
    }
}
