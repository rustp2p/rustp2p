/*
   0                                            15                                              31
   0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |    unused(8)        | ID length(8)          |   protocol (8)       |max ttl(4) |curr ttl(4) |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                           src ID                                            |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                          dest ID                                            |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                         payload(n)                                          |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
*/

use std::fmt::Debug;

use node_id::NodeID;

use crate::error::{Error, Result};
use crate::protocol::protocol_type::ProtocolType;

pub mod echo;
pub mod id_route;
pub mod node_id;
pub mod proto;
pub mod protocol_type;
pub mod punch;
pub mod timestamp;

pub struct NetPacket<B> {
    buffer: B,
}

impl<B: AsRef<[u8]>> NetPacket<B> {
    pub fn new(buffer: B) -> Result<NetPacket<B>> {
        let len = buffer.as_ref().len();
        if len < 4 {
            return Err(Error::Overflow {
                cap: len,
                required: 4,
            });
        }
        let packet = Self { buffer };
        let head_len = 4 + 2 * packet.id_length() as usize;
        if len < head_len {
            return Err(Error::Overflow {
                cap: len,
                required: head_len,
            });
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
    pub fn new(buf: &'a mut B, id_len: u8) -> Result<Self> {
        let slice = buf.as_mut();
        let head_len = 4 + 2 * id_len as usize;
        if slice.len() < head_len {
            return Err(Error::Overflow {
                cap: slice.len(),
                required: head_len,
            });
        }
        if id_len % 2 != 0 {
            return Err(Error::InvalidArgument(format!(
                "node_id len must be a non-zero integer power of two, the checked one is {id_len}"
            )));
        }
        slice[1] = id_len;
        Ok(Builder(buf, id_len as usize))
    }
    pub fn protocol(&mut self, p: ProtocolType) -> Result<&mut Self> {
        let slice = self.0.as_mut();
        slice[2] = p.into();
        Ok(self)
    }
    pub fn ttl(&mut self, t: u8) -> Result<&mut Self> {
        let slice = self.0.as_mut();
        if t > 15 {
            return Err(Error::InvalidArgument(
                "ttl must be less or equal than 15".to_string(),
            ));
        }
        let t_4 = (t << 4) | t;
        slice[3] = t_4;
        Ok(self)
    }
    pub fn src_id<T: Into<NodeID>>(&mut self, src: T) -> Result<&mut Self> {
        let src = src.into();
        let slice = self.0.as_mut();
        let bytes = src.as_ref();
        if bytes.len() != self.1 {
            return Err(Error::InvalidArgument(format!(
                "the length of source node_id must equal to {}",
                self.1
            )));
        }
        slice[4..4 + self.1].copy_from_slice(bytes);
        Ok(self)
    }
    pub fn dest_id<T: Into<NodeID>>(&mut self, dest: T) -> Result<&mut Self> {
        let dest = dest.into();
        let slice = self.0.as_mut();
        let bytes = dest.as_ref();
        if bytes.len() != self.1 {
            return Err(Error::InvalidArgument(format!(
                "the length of destination node_id must equal to {}",
                self.1
            )));
        }
        slice[4 + self.1..4 + 2 * self.1].copy_from_slice(bytes);
        Ok(self)
    }
    pub fn payload(&mut self, data: &[u8]) -> Result<&mut Self> {
        let slice = self.0.as_mut();
        let total = slice.len();
        let head_len = 4 + 2 * self.1 as usize;
        let need_size = head_len + data.len();
        if need_size > total {
            return Err(Error::Overflow {
                cap: total,
                required: need_size,
            });
        }
        slice[head_len..need_size].copy_from_slice(data);
        Ok(self)
    }
}

impl<'a, B: AsRef<[u8]>> Debug for Builder<'a, B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let buf = self.0.as_ref();
        if buf.len() < 4 {
            return f.write_str("");
        }
        let ttl = buf[3];
        let id_len = self.1;
        let src_id = if buf.len() < (4 + id_len) {
            "".to_string()
        } else {
            format!("{:?}", &buf[4..4 + id_len])
        };
        let dest_id = if buf.len() < (4 + 2 * id_len) {
            "".to_string()
        } else {
            format!("{:?}", &buf[4 + id_len..4 + 2 * id_len])
        };
        let payload_size = if buf.len() > 4 + 2 * id_len {
            buf.len() - (4 + 2 * id_len)
        } else {
            0
        };
        let s = format!(
            "
   0                                           15                                               31
   0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |    unused(8)        | {0:^20}| {1:^22}| {2:^12} | {3:^6}   |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  | {4:^91} |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  | {5:^91} |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  | {6:^91} |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
		",
            format!("{}bytes", buf[1]),
            format!("{:?}", ProtocolType::from(buf[2])),
            format!("{:0b}", ttl >> 4),
            format!("{:0b}", ttl & 0b00001111),
            src_id,
            dest_id,
            format!("{}bytes", payload_size)
        );
        f.write_str(&s)
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
            .protocol(ProtocolType::TimestampRequest)
            .unwrap()
            .src_id(1i32)
            .unwrap()
            .dest_id(2i32)
            .unwrap();
        println!("{:?}", build);
        assert_eq!(
            buf,
            [
                0u8,
                4u8,
                ProtocolType::TimestampRequest as u8,
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
