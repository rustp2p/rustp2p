/*
   0                                            15                                              31
   0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                      Custom prefix(n)                                       |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |               unused(16)                    |   protocol (8)       |first ttl(4) | ttl(4)   |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                         src ID(n)                                           |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                         dest ID(n)                                          |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                         payload(n)                                          |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
*/

use std::io;

pub struct NetPacket<B, ID> {
    _marker: std::marker::PhantomData<ID>,
    buffer: B,
}

impl<B: AsRef<[u8]>, ID: PeerID> NetPacket<B, ID> {
    pub fn new(buffer: B) -> NetPacket<B, ID> {
        Self {
            _marker: Default::default(),
            buffer,
        }
    }
    pub fn protocol(&self) -> u8 {
        self.buffer.as_ref()[2]
    }
    pub fn first_ttl(&self) -> u8 {
        self.buffer.as_ref()[3] >> 4
    }
    pub fn ttl(&self) -> u8 {
        self.buffer.as_ref()[3] & 0xF
    }
    pub fn src_id(&self) -> io::Result<ID> {
        let end = ID::LEN + 4;
        let buf = self.buffer.as_ref();
        if buf.len() < end {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        ID::parse(&buf[4..end])
    }
    pub fn dest_id(&self) -> io::Result<ID> {
        let start = ID::LEN + 4;
        let end = start + ID::LEN;
        let buf = self.buffer.as_ref();
        if buf.len() < end {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        ID::parse(&buf[start..end])
    }
}

pub trait PeerID: Clone {
    const LEN: usize;
    fn as_bytes(&self) -> &[u8];
    fn parse(buf: &[u8]) -> io::Result<Self>;
}
