use std::hash::Hash;
use std::io;

pub trait NodeID: Hash + Eq + Clone {
    const LEN: usize;
    fn as_bytes(&self) -> &[u8];
    fn parse(buf: &[u8]) -> io::Result<Self>;
}
