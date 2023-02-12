use crate::connection::client_connection::ClientConnection;
use crate::connection::server_connection::ServerConnection;
use crate::messages::Response;
use crate::transport::client_std_udp::UdpClientTransport;
use crate::transport::server_std_udp::UdpServerTransport;
use crate::{Guarantees, MsgTable, MsgTableBuilder};
use serde::{Deserialize, Serialize};
use std::io::ErrorKind;
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

#[test]
#[cfg(target_os = "linux")]
fn test_reliability() {
    env_logger::init();

    let msg_table = get_msg_table();

    let server_addr = "127.0.0.1:7777".parse().unwrap();
    let client_addr = "127.0.0.1:0".parse().unwrap();

    let mut server_connection: ServerConnection<
        UdpServerTransport,
        Connection,
        Accepted,
        Rejected,
        Disconnect,
    > = ServerConnection::new(msg_table.clone(), server_addr).unwrap();
    let mut client_connection: ClientConnection<
        UdpClientTransport,
        Connection,
        Accepted,
        Rejected,
        Disconnect,
    > = ClientConnection::new(msg_table);

    client_connection
        .connect(client_addr, server_addr)
        .expect("Connection failed");

    client_connection.send(&Connection).unwrap();

    sleep(Duration::from_millis(10));
    // recv_from needs to be called in order for the connection to read the client's message.
    // Since the message is a connection type message, it will not be returned from the function.
    assert_eq!(
        server_connection.recv_from().unwrap_err().kind(),
        ErrorKind::WouldBlock
    );
    let handled = server_connection.handle_pending(|_cid, _addr, _msg: Connection| {
        Response::Accepted::<Accepted, Rejected>(Accepted)
    });
    assert_eq!(handled, 1);

    // simulate bad network conditions
    Command::new("bash")
        .arg("-c")
        .arg("sudo tc qdisc add dev lo root netem delay 10ms corrupt 5 duplicate 5 loss random 5 reorder 5")
        .output()
        .expect("failed to run `tc` to emulate an unstable network on the `lo` adapter");

    let msg = ReliableMsg::new("This is the message that is sent.");
    let mut results = vec![];

    // send 10 bursts of 10 messages.
    for _ in 0..10 {
        // make sure that the client is receiving the server's acks
        let _ = client_connection.recv();
        client_connection.resend_reliable();
        for _ in 0..10 {
            client_connection.send(&msg).expect("failed to send");
        }
        sleep(Duration::from_millis(150));
        loop {
            match server_connection.recv_from() {
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(err) => panic!("unexpected error: {}", err),
                Ok((_cid, header, msg)) => {
                    // TODO: This will also not be necessary once ignore Connection/Response msgs
                    //       after connected.
                    if header.m_type == 5 {
                        results.push(msg);
                    }
                }
            }
        }
        // send ack messages for the reliability system.
        server_connection.send_ack_msgs();
    }

    println!("All messages sent at least once. Looping 10 more times for reliability");
    // do some more receives to get the stragglers
    for _ in 0..10 {
        // make sure that the client is receiving the server's acks
        let _ = client_connection.recv();
        client_connection.resend_reliable();
        sleep(Duration::from_millis(150));
        loop {
            match server_connection.recv_from() {
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(err) => panic!("unexpected error: {}", err),
                Ok((_cid, header, msg)) => {
                    // TODO: This will also not be necessary once ignore Connection/Response msgs
                    //       after connected.
                    if header.m_type == 5 {
                        results.push(msg);
                    }
                }
            }
        }
        // send ack messages for the reliability system.
        server_connection.send_ack_msgs();
    }
    // remove the simulated conditions
    Command::new("bash")
        .arg("-c")
        .arg("sudo tc qdisc del dev lo root netem")
        .output()
        .expect("failed to run `tc` to remove the emulated network conditions on the `lo` adapter");

    for v in results.iter() {
        println!("{:?}", v);
    }

    // ensure all messages arrive uncorrupted
    for v in results.iter() {
        assert_eq!(
            v.downcast_ref::<ReliableMsg>().unwrap(),
            &msg,
            "message is not intact"
        )
    }

    // ensure all messages arrive
    assert_eq!(results.len(), 10 * 10, "not all messages arrived");
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize, Debug)]
/// A reliable test message.
pub struct ReliableMsg {
    pub msg: String,
}
impl ReliableMsg {
    pub fn new(msg: impl ToString) -> Self {
        ReliableMsg {
            msg: msg.to_string(),
        }
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize, Debug)]
/// An unreliable test message.
pub struct UnreliableMsg {
    pub msg: String,
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize, Debug)]
/// A test connection message.
pub struct Connection;

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize, Debug)]
/// A test disconnection message.
pub struct Disconnect;

/// The accepted message.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize, Debug)]
pub struct Accepted;
/// The rejected message.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize, Debug)]
pub struct Rejected;

/// Builds a table with all these test messages and returns it's parts.
pub fn get_msg_table() -> MsgTable<Connection, Accepted, Rejected, Disconnect> {
    let mut builder = MsgTableBuilder::new();
    builder
        .register_ordered::<ReliableMsg>(Guarantees::ReliableOrdered)
        .unwrap();
    builder
        .register_ordered::<UnreliableMsg>(Guarantees::Unreliable)
        .unwrap();
    builder
        .build::<Connection, Accepted, Rejected, Disconnect>()
        .unwrap()
}
