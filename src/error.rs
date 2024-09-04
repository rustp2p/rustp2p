use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("out of the storage: capacity is {cap} required is at least {required}")]
    Overflow { cap: usize, required: usize },
    #[error("invalid argument:{0}")]
    InvalidArgument(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
