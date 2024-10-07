use std::io;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("io")]
    Io(#[from] io::Error),
    #[error("eof")]
    Eof,
    #[error("route not found: {0}")]
    RouteNotFound(String),
    #[error("invalid protocol")]
    InvalidProtocol,
    #[error("Not support IPV6")]
    NotSupportIPV6,
    #[error("index out of bounds:len is {len} but the index is {index}")]
    IndexOutOfBounds { len: usize, index: usize },
    #[error("Packet loss")]
    PacketLoss,
}

pub type Result<T, E = Error> = ::std::result::Result<T, E>;
