use std::time::SystemTimeError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("out of the storage: capacity is {cap} required is at least {required}")]
    Overflow { cap: usize, required: usize },
    #[error("invalid argument:{0}")]
    InvalidArgument(String),
    #[error("No ID specified")]
    NoIDSpecified,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Any(#[from] anyhow::Error),
    #[error(transparent)]
    Core(#[from] rust_p2p_core::error::Error),
    #[error(transparent)]
    SystemTimeError(#[from] SystemTimeError),
    #[error("Node ID not available")]
    NodeIDNotAvailable,
}

pub type Result<T> = std::result::Result<T, Error>;
