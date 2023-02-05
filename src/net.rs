//! Networking things that are not specific to either transport.

use serde::{Deserialize, Serialize};
use std::any::Any;
use std::fmt::{Debug, Display, Formatter};
use std::io;
use std::io::Error;
use std::ops::Deref;

/// The maximum safe message size that can be sent on udp,
/// after taking off the possible overheads from the transport.
///
/// The data must be [`MAX_SAFE_MESSAGE_SIZE`] or less to be guaranteed to
/// be deliverable on udp.
/// [source](https://newbedev.com/what-is-the-largest-safe-udp-packet-size-on-the-internet/)
pub const MAX_SAFE_MESSAGE_SIZE: usize = 508;

/// The absolute maximum size that udp supports.
///
/// The data must be less than [`MAX_MESSAGE_SIZE`] or it will be dropped.
pub const MAX_MESSAGE_SIZE: usize = 65507;

/// The size of carrier-pigeon's header.
pub const HEADER_SIZE: usize = 12;

/// A header to be sent before the message contents of a message.
#[derive(Debug, Eq, PartialEq, Copy, Clone, Hash)]
pub struct MsgHeader {
    /// The message type of this message.
    pub m_type: MType,
    /// An incrementing integer specific to this `m_type`. This allows us to order messages
    /// on arrival.
    pub order_num: OrderNum,
    /// The [`AckNumber`] of this outgoing message.
    pub sender_ack_num: AckNum,
    /// The header also contains a place to acknowledge the previously received messages that were
    /// from the destination of this message.
    ///
    /// This number is a offset for the `ack_bits`. Read `acknowledgements.md` and look at
    /// [AckSystem](crate::connection::ack_system::AckSystem) for more.
    pub receiver_acking_offset: AckNum,
    /// 32 bits representing weather the 32 ack numbers before the `receiver_acking_num` are acked.
    ///
    /// This allows us to acknowledge up to 32 messages at once.
    ///
    /// For example, with an `receiver_acking_num` of 32
    pub ack_bits: u32,
}

impl MsgHeader {
    /// Creates a [`MsgHeader`] with the given [`MType`], `ack_number` and `order_num`.
    pub fn new(
        m_type: MType,
        order_num: OrderNum,
        sender_ack_num: AckNum,
        receiver_acking_num: AckNum,
        ack_bits: u32,
    ) -> Self {
        MsgHeader {
            m_type,
            order_num,
            sender_ack_num,
            receiver_acking_offset: receiver_acking_num,
            ack_bits,
        }
    }

    /// Converts the [`MsgHeader`] to big endian bytes to be sent over the internet.
    pub fn to_be_bytes(&self) -> [u8; HEADER_SIZE] {
        let m_type_b = (self.m_type as u16).to_be_bytes();
        let order_num_b = self.order_num.to_be_bytes();
        let sender_ack_num_b = self.sender_ack_num.to_be_bytes();
        let receiver_acking_num_b = self.receiver_acking_offset.to_be_bytes();
        let ack_bits_b = self.ack_bits.to_be_bytes();
        debug_assert_eq!(m_type_b.len(), 2);
        debug_assert_eq!(order_num_b.len(), 2);
        debug_assert_eq!(sender_ack_num_b.len(), 2);
        debug_assert_eq!(receiver_acking_num_b.len(), 2);
        debug_assert_eq!(ack_bits_b.len(), 4);
        debug_assert_eq!(
            m_type_b.len()
                + order_num_b.len()
                + sender_ack_num_b.len()
                + receiver_acking_num_b.len()
                + ack_bits_b.len(),
            HEADER_SIZE
        );

        [
            m_type_b[0],
            m_type_b[1],
            order_num_b[0],
            order_num_b[1],
            sender_ack_num_b[0],
            sender_ack_num_b[1],
            receiver_acking_num_b[0],
            receiver_acking_num_b[1],
            ack_bits_b[0],
            ack_bits_b[1],
            ack_bits_b[2],
            ack_bits_b[3],
        ]
    }

    /// Converts the big endian bytes back into a [`MsgHeader`].
    ///
    /// You **must** pass in a slice that is [`HEADER_LEN`] long.
    pub fn from_be_bytes(bytes: &[u8]) -> Self {
        assert_eq!(
            bytes.len(),
            HEADER_SIZE,
            "The length of the buffer passed into `from_be_bytes` should have a length of {}",
            HEADER_SIZE
        );

        let m_type = u16::from_be_bytes(bytes[..2].try_into().unwrap()) as usize;
        let order_num = u16::from_be_bytes(bytes[2..4].try_into().unwrap());
        let sender_ack_num = u16::from_be_bytes(bytes[4..6].try_into().unwrap());
        let receiver_acking_num = u16::from_be_bytes(bytes[6..8].try_into().unwrap());
        let ack_bits = u32::from_be_bytes(bytes[8..12].try_into().unwrap());

        MsgHeader {
            m_type,
            order_num,
            sender_ack_num,
            receiver_acking_offset: receiver_acking_num,
            ack_bits,
        }
    }
}

/// The function used to deserialize a message.
///
/// fn(&[[u8]]) -> [`io::Result`]<[`Box`]<dyn [`Any`] + [`Send`] + [`Sync`]>>
pub type DeserFn = fn(&[u8]) -> io::Result<Box<dyn Any + Send + Sync>>;
/// The function used to serialize a message.
///
/// fn(&(dyn [`Any`] + [`Send`] + [`Sync`]), &mut [`Vec`]<[`u8`]>) -> [`io::Result`]<()>
pub type SerFn = fn(&(dyn Any + Send + Sync), &mut Vec<u8>) -> io::Result<()>;

#[derive(Debug)]
/// An enum for the possible states of a connection.
///
/// ```mermaid
/// ---
/// title: Status State Machine
/// ---
/// flowchart LR
///     NotConnected -- connect() --> Connecting
///     Connecting -- {{
///
/// ```
pub enum Status {
    /// Not connected to any peer.
    NotConnected,
    /// Currently connecting to a peer.
    ///
    /// We have sent a connection message, but have yet to hear a response.
    Connecting,
    /// We just got accepted.
    Accepted(Box<dyn Any + Send + Sync>),
    /// We just got rejected.
    Rejected(Box<dyn Any + Send + Sync>),
    /// The connection failed.
    ConnectionFailed(Error),
    /// The connection is established.
    Connected,
    /// The connection is closed because the peer disconnected by sending a disconnection message.
    Disconnected(Box<dyn Any + Send + Sync>),
    /// The connection was dropped without sending a disconnection message.
    Dropped(Error),
    /// Disconnecting from the peer.
    Disconnecting,
}

impl Display for Status {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Status::NotConnected => write!(f, "Not connected"),
            Status::Connecting => write!(f, "Connecting..."),
            Status::Accepted(_) => write!(f, "Accepted"),
            Status::Rejected(_) => write!(f, "Rejected"),
            Status::ConnectionFailed(e) => write!(f, "Connection failed with error {}", e),
            Status::Connected => write!(f, "Connected"),
            Status::Disconnected(_) => write!(f, "Disconnected gracefully"),
            Status::Dropped(e) => write!(f, "Dropped with error {}", e),
            Status::Disconnecting => write!(f, "Disconnecting..."),
        }
    }
}

impl Status {
    // matches functions

    /// Weather this status is [`NotConnected`](Self::NotConnected).
    pub fn is_not_connected(&self) -> bool {
        matches!(self, Status::NotConnected)
    }

    /// Weather this status is [`Connecting`](Self::Connecting).
    pub fn is_connecting(&self) -> bool {
        matches!(self, Status::Connecting)
    }

    /// Weather this status is [`Accepted`](Self::Accepted).
    pub fn is_accepted(&self) -> bool {
        matches!(self, Status::Accepted(_))
    }

    /// Weather this status is [`Rejected`](Self::Rejected).
    pub fn is_rejected(&self) -> bool {
        matches!(self, Status::Rejected(_))
    }

    /// Weather this status is [`Connected`](Self::Connected).
    pub fn is_connected(&self) -> bool {
        matches!(self, Status::Connected)
    }

    /// Weather this status is [`Disconnected`](Self::Disconnected).
    pub fn is_disconnected(&self) -> bool {
        matches!(self, Status::Disconnected(_))
    }

    /// Weather this status is [`Dropped`](Self::Dropped).
    pub fn is_dropped(&self) -> bool {
        matches!(self, Status::Dropped(_))
    }

    /// Weather this status is [`Disconnecting`](Self::Disconnecting).
    pub fn is_disconnecting(&self) -> bool {
        matches!(self, Status::Disconnecting)
    }

    // unwrapping functions

    /// Unwraps the acceptation message from the [`Accepted`](Self::Accepted) variant,
    /// and returns the boxed message. This is guaranteed to be the response message type
    /// (generic parameter `R`) that you passed into
    /// [`MsgTableBuilder::build`](crate::MsgTableBuilder::build).
    /// For a typed version, use [`unwrap_accepted`](Self::unwrap_accepted).
    pub fn unwrap_accepted_dyn(self) -> Option<Box<dyn Any + Send + Sync>> {
        match self {
            Status::Accepted(msg) => Some(msg),
            _ => None,
        }
    }

    /// Unwraps the rejection message from the [`Rejected`](Self::Rejected) variant,
    /// and returns the boxed message. This is guaranteed to be the response message type
    /// (generic parameter `R`) that you passed into
    /// [`MsgTableBuilder::build`](crate::MsgTableBuilder::build).
    /// For a typed version, use [`unwrap_rejected`](Self::unwrap_rejected).
    pub fn unwrap_rejected_dyn(self) -> Option<Box<dyn Any + Send + Sync>> {
        match self {
            Status::Rejected(msg) => Some(msg),
            _ => None,
        }
    }

    /// Unwraps the disconnection message from the [`Disconnected`](Self::Disconnected) variant,
    /// and returns the boxed message. This is guaranteed to be the disconnection message type
    /// (generic parameter `D`) that you passed into
    /// [`MsgTableBuilder::build`](crate::MsgTableBuilder::build).
    /// For a typed version, use [`unwrap_disconnected`](Self::unwrap_disconnected).
    pub fn unwrap_disconnected_dyn(self) -> Option<Box<dyn Any + Send + Sync>> {
        match self {
            Status::Disconnected(msg) => Some(msg),
            _ => None,
        }
    }

    /// Unwraps the acceptation message from the [`Accepted`](Self::Accepted) variant,
    /// and returns the message. If generic parameter `R` is not the response message type
    /// (generic parameter `R` that you passed into
    /// [`MsgTableBuilder::build`](crate::MsgTableBuilder::build)) this will return `None`.
    /// For an untyped version, use [`unwrap_accepted_dyn`](Self::unwrap_accepted_dyn).
    pub fn unwrap_accepted<R: Any + Send + Sync>(self) -> Option<R> {
        self.unwrap_accepted_dyn()?.downcast().ok().map(|msg| *msg)
    }

    /// Unwraps the rejection message from the [`Rejected`](Self::Rejected) variant,
    /// and returns the message. If generic parameter `R` is not the response message type
    /// (generic parameter `R` that you passed into
    /// [`MsgTableBuilder::build`](crate::MsgTableBuilder::build)) this will return `None`.
    /// For an untyped version, use [`unwrap_rejected_dyn`](Self::unwrap_rejected_dyn).
    pub fn unwrap_rejected<R: Any + Send + Sync>(self) -> Option<R> {
        self.unwrap_rejected_dyn()?.downcast().ok().map(|msg| *msg)
    }

    /// Unwraps the disconnection message from the [`Disconnected`](Self::Disconnected) variant,
    /// and returns the message. If generic parameter `D` is not the disconnection message type
    /// (generic parameter `D` that you passed into
    /// [`MsgTableBuilder::build`](crate::MsgTableBuilder::build)) this will return `None`.
    /// For an untyped version, use [`unwrap_disconnected_dyn`](Self::unwrap_disconnected_dyn).
    pub fn unwrap_disconnected<D: Any + Send + Sync>(self) -> Option<D> {
        self.unwrap_disconnected_dyn()?.downcast().ok().map(|msg| *msg)
    }

    /// Unwraps the dropped error from the [`Dropped`](Self::Dropped) variant.
    pub fn unwrap_dropped(self) -> Option<Error> {
        match self {
            Status::Dropped(err) => Some(err),
            _ => None,
        }
    }

    /// Turns this into an option with the disconnect message.
    ///
    /// ### Panics
    /// Panics if the generic parameter `D` isn't the disconnect message type (the same `D` that
    /// you passed into `MsgTable::build`).
    pub fn disconnected<D: Any + Send + Sync>(&self) -> Option<&D> {
        match self {
            Status::Disconnected(d) => Some(d.downcast_ref().expect("The generic parameter `D` must be the disconnection message type (the same `D` that you passed into `MsgTable::build`).")),
            _ => None,
        }
    }

    /// Turns this into an option with the disconnect message.
    pub fn disconnected_dyn(&self) -> Option<&Box<dyn Any + Send + Sync>> {
        match self {
            Status::Disconnected(d) => Some(d),
            _ => None,
        }
    }

    /// Turns this into an option with the drop error.
    pub fn dropped(&self) -> Option<&Error> {
        match self {
            Status::Dropped(e) => Some(e),
            _ => None,
        }
    }
}

/// Message Type.
///
/// This is an integer unique to each type of message.
pub type MType = usize;

/// Connection ID.
///
/// This is an integer incremented for every connection made to the server, so connections can
/// be uniquely identified.
pub type CId = u32;

/// Acknowledgement Number.
///
/// This is an integer incremented for every message sent, so messages can be uniquely identified.
/// This is used as a way to acknowledge reliable messages.
// TODO: this might need to be a wrapper type, as the comparing logic should consider wrapping
pub type AckNum = u16;

/// Ordering Number.
///
/// This is an integer specific to each [`MType`], incremented for every message sent,
/// This is so we can order the messages as they come in.
// TODO: this might need to be a wrapper type, as the comparing logic should consider wrapping
pub type OrderNum = u16;

/// A way to specify the valid [`CId`]s for an operation.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub enum CIdSpec {
    /// Matches all [`CId`]s
    All,
    /// Matches no [`CId`]s.
    None,
    /// Matches all except the inner [`CId`]
    Except(CId),
    /// Matches only the inner [`CId`]
    Only(CId),
}

impl CIdSpec {
    /// Weather the given cid matches the pattern.
    pub fn matches(&self, cid: CId) -> bool {
        match self {
            CIdSpec::All => true,
            CIdSpec::None => false,
            CIdSpec::Except(o) => cid != *o,
            CIdSpec::Only(o) => cid == *o,
        }
    }

    /// Checks if the `other` [`CIdSpec`] overlaps (shares at least on common [`CId`]).
    pub fn overlaps(&self, other: CIdSpec) -> bool {
        use CIdSpec::*;

        match (*self, other) {
            (None, _) => false,
            (_, None) => false,
            (All, _) => true,
            (_, All) => true,

            (Except(_), Except(_)) => true,
            (Only(o1), Only(o2)) => o1 == o2,

            (Only(only), Except(except)) => only != except,
            (Except(except), Only(only)) => only != except,
        }
    }
}

/// Configuration for a client.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize, Default)]
pub struct ClientConfig {}

impl ClientConfig {
    /// Creates a new client configuration.
    pub fn new() -> Self {
        ClientConfig {}
    }
}

/// Configuration for a server.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize, Default)]
pub struct ServerConfig {}

impl ServerConfig {
    /// Creates a new server configuration.
    pub fn new() -> Self {
        ServerConfig {}
    }
}

/// An untyped network message containing the message content, along with the metadata associated.
#[derive(Debug)]
pub(crate) struct ErasedNetMsg {
    /// The [`CId`] that the message was sent from.
    pub cid: CId,
    /// The acknowledgment number. This is an incrementing integer assigned by the sender for every
    /// message.
    ///
    /// This is not necessarily guaranteed to be unique as wrapping can happen after a lot of
    /// messages.
    pub ack_num: AckNum,
    /// The ordering number. This is an incrementing integer assigned by the sender, on a
    /// per-[`MType`] basis.
    ///
    /// This is not necessarily guaranteed to be unique as wrapping can happen after a lot of
    /// messages.
    pub order_num: OrderNum,
    /// The actual message.
    pub msg: Box<dyn Any + Send + Sync>,
}

impl ErasedNetMsg {
    pub(crate) fn new(
        cid: CId,
        ack_num: AckNum,
        order_num: OrderNum,
        msg: Box<dyn Any + Send + Sync>,
    ) -> Self {
        Self {
            cid,
            ack_num,
            order_num,
            msg,
        }
    }

    /// Converts this to NetMsg, borrowed from this.
    pub(crate) fn get_typed<T: Any + Send + Sync>(&self) -> Option<NetMsg<T>> {
        let msg = self.msg.downcast_ref()?;
        Some(NetMsg {
            cid: self.cid,
            ack_num: self.ack_num,
            order_num: self.order_num,
            m: msg,
        })
    }
}

/// A network message containing the message content, along with the metadata associated.
#[derive(Eq, PartialEq, Copy, Clone, Debug)]
pub struct NetMsg<'n, T: Any + Send + Sync> {
    /// The [`CId`] that the message was sent from.
    pub cid: CId,
    /// The acknowledgment number. This is an incrementing integer assigned by the sender for every
    /// message.
    ///
    /// This is not necessarily guaranteed to be unique as wrapping can happen after a lot of
    /// messages.
    pub ack_num: AckNum,
    /// The ordering number. This is an incrementing integer assigned by the sender, on a
    /// per-[`MType`] basis.
    ///
    /// This is not necessarily guaranteed to be unique as wrapping can happen after a lot of
    /// messages.
    pub order_num: OrderNum,
    /// The actual message.
    ///
    /// Borrowed from the client or server.
    pub m: &'n T,
}

impl<'n, T: Any + Send + Sync> Deref for NetMsg<'n, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.m
    }
}
