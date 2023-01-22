#![allow(unused)]
//! Test messages for use in tests.

use carrier_pigeon::{Guarantees, MsgTable, MsgTableBuilder};
use serde::{Deserialize, Serialize};

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize, Debug)]
/// A test message for TCP.
pub struct ReliableMsg {
    pub msg: String,
}
impl ReliableMsg {
    pub fn new<A: Into<String>>(msg: A) -> Self {
        ReliableMsg { msg: msg.into() }
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize, Debug)]
/// A test message for UDP.
pub struct UnreliableMsg {
    pub msg: String,
}
impl UnreliableMsg {
    pub fn new<A: Into<String>>(msg: A) -> Self {
        UnreliableMsg { msg: msg.into() }
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize, Debug)]
/// A test connection message.
pub struct Connection {
    pub usr: String,
}
impl Connection {
    pub fn new<A: Into<String>>(usr: A) -> Self {
        Connection { usr: usr.into() }
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize, Debug)]
/// A test disconnection message.
pub struct Disconnect {
    pub reason: String,
}
impl Disconnect {
    pub fn new<A: Into<String>>(reason: A) -> Self {
        Disconnect {
            reason: reason.into(),
        }
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize, Debug)]
/// A test response message.
pub enum Response {
    Accepted,
    Rejected(String),
}
impl Response {
    pub fn rejected<A: Into<String>>(reason: A) -> Self {
        Response::Rejected(reason.into())
    }
    pub fn accepted() -> Self {
        Response::Accepted
    }
}

/// Builds a table with all these test messages and returns it's parts.
pub fn get_table_parts() -> MsgTable {
    let mut table = MsgTableBuilder::new();
    table.register_ordered::<ReliableMsg>(Guarantees::ReliableOrdered).unwrap();
    table.register_ordered::<UnreliableMsg>(Guarantees::Unreliable).unwrap();
    table.build::<Connection, Response, Disconnect>().unwrap()
}
