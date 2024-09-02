/*
   0                                            15                                              31
   0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |    unused(8)        | ID length(8)          |   protocol (8)       |first ttl(4) | ttl(4)   |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                           src ID                                            |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                          dest ID                                            |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                         payload(n)                                          |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
*/

use std::io;

use crate::protocol::protocol_type::ProtocolType;

pub mod node_id;
pub mod protocol_type;

pub struct NetPacket<B> {
    buffer: B,
}

impl<B: AsRef<[u8]>> NetPacket<B> {
    pub fn new(buffer: B) -> io::Result<NetPacket<B>> {
        let len = buffer.as_ref().len();
        if len < 4 {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        let packet = Self { buffer };
        let head_len = 4 + 2 * packet.id_length() as usize;
        if len < head_len {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        Ok(packet)
    }
    pub fn id_length(&self) -> u8 {
        self.buffer.as_ref()[1]
    }
    pub fn protocol(&self) -> ProtocolType {
        self.buffer.as_ref()[2].into()
    }
    pub fn first_ttl(&self) -> u8 {
        self.buffer.as_ref()[3] >> 4
    }
    pub fn ttl(&self) -> u8 {
        self.buffer.as_ref()[3] & 0xF
    }
    pub fn src_id(&self) -> &[u8] {
        let end = self.id_length() as usize + 4;
        &self.buffer.as_ref()[4..end]
    }
    pub fn dest_id(&self) -> &[u8] {
        let start = 4 + self.id_length() as usize;
        let end = start + self.id_length() as usize;
        &self.buffer.as_ref()[start..end]
    }
    pub fn payload(&self) -> &[u8] {
        let start = 4 + self.id_length() as usize * 2;
        &self.buffer.as_ref()[start..]
    }
}
