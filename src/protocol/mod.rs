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

use node_id::NodeID;

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
impl<B: AsRef<[u8]> + AsMut<[u8]>> NetPacket<B> {
    pub fn payload_mut(&mut self) -> &mut [u8] {
        let start = 4 + self.id_length() as usize * 2;
        &mut self.buffer.as_mut()[start..]
    }
}

pub struct Builder<'a, B>(&'a mut B, usize);

impl<'a, B: AsMut<[u8]>> Builder<'a, B> {
    pub fn new(buf: &'a mut B, id_len: u8) -> std::io::Result<Self> {
        let slice = buf.as_mut();
        let head_len = 4 + 2 * id_len as usize;
        if slice.len() < head_len {
            return Err(std::io::Error::other("out of bounds"));
        }
        if id_len % 2 != 0 {
            return Err(std::io::Error::other("invalid id len"));
        }
        slice[1] = id_len;
        Ok(Builder(buf, id_len as usize))
    }
    pub fn protocol(&mut self, p: ProtocolType) -> std::io::Result<&mut Self> {
        let slice = self.0.as_mut();
        slice[2] = p.into();
        Ok(self)
    }
    pub fn ttl(&mut self, t: u8) -> std::io::Result<&mut Self> {
        let slice = self.0.as_mut();
        if t > 15 {
            return Err(std::io::Error::other("out of ttl bounds"));
        }
        let t_4 = (t << 4) | t;
        slice[3] = t_4;
        Ok(self)
    }
    pub fn src_id<T: Into<NodeID>>(&mut self, src: T) -> std::io::Result<&mut Self> {
        let src = src.into();
        let slice = self.0.as_mut();
        let bytes = src.as_ref();
        if bytes.len() != self.1 {
            return Err(std::io::Error::other("invalid src len"));
        }
        slice[4..4 + self.1].copy_from_slice(bytes);
        Ok(self)
    }
    pub fn dest_id<T: Into<NodeID>>(&mut self, dest: T) -> std::io::Result<&mut Self> {
        let dest = dest.into();
        let slice = self.0.as_mut();
        let bytes = dest.as_ref();
        if bytes.len() != self.1 {
            return Err(std::io::Error::other("invalid dest len"));
        }
        slice[4 + self.1..4 + 2 * self.1].copy_from_slice(bytes);
        Ok(self)
    }
    pub fn payload(&mut self, data: &[u8]) -> std::io::Result<&mut Self> {
        let slice = self.0.as_mut();
        let total = slice.len();
        let head_len = 4 + 2 * self.1 as usize;
        let need_size = head_len + data.len();
        if need_size > total {
            return Err(std::io::Error::other("no free storage to store data"));
        }
        slice[head_len..need_size].copy_from_slice(data);
        Ok(self)
    }
}

#[cfg(test)]
mod test {
    use crate::protocol::node_id::NodeID;

    use super::{protocol_type::ProtocolType, Builder};

    #[test]
    fn test_build() {
        let mut buf = [0u8; 12];
        let mut build = Builder::new(&mut buf, 4).unwrap();
        build
            .ttl(10)
            .unwrap()
            .protocol(ProtocolType::Unknown)
            .unwrap()
            .src_id(1i32)
            .unwrap()
            .dest_id(2i32)
            .unwrap();
        assert_eq!(
            buf,
            [
                0u8,
                4u8,
                ProtocolType::Unknown as u8,
                0b10101010u8,
                0,
                0,
                0,
                1,
                0,
                0,
                0,
                2
            ]
        );
        assert_eq!(1i32, i32::try_from(NodeID::Bit32([0, 0, 0, 1])).unwrap());
    }
}
