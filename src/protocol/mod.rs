/*
   0                                            15                                              31
   0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |1|   protocol(7)       |                 data len(16)               |max ttl(4) |curr ttl(4) |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |e|                                        reserve(31)                                         |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                        group code(128)                                      |
  |                                                                                             |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                         src ID(32)                                          |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                         dest ID(32)                                         |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                         payload(n)                                          |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
*/

use std::fmt::Debug;
use std::io;

use node_id::NodeID;

use crate::protocol::node_id::GroupCode;
use crate::protocol::protocol_type::ProtocolType;

pub const HEAD_LEN: usize = 32;

pub mod broadcast;
pub mod echo;
pub mod id_route;
pub mod node_id;
pub mod protocol_type;
pub mod punch;
pub mod timestamp;

pub struct NetPacket<B> {
    buffer: B,
}

impl<B: AsRef<[u8]>> NetPacket<B> {
    /// # Safety
    /// Ensures that the given buffer has enough space
    pub unsafe fn new_unchecked(buffer: B) -> NetPacket<B> {
        Self { buffer }
    }
    pub fn new(buffer: B) -> io::Result<NetPacket<B>> {
        let len = buffer.as_ref().len();
        if len < HEAD_LEN {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "overflow"));
        }
        let packet = unsafe { Self::new_unchecked(buffer) };
        if packet.data_length() as usize != len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "packet len invalid",
            ));
        }
        Ok(packet)
    }
    pub fn protocol(&self) -> io::Result<ProtocolType> {
        (self.buffer.as_ref()[0] & 0x7F).try_into()
    }

    pub fn data_length(&self) -> u16 {
        ((self.buffer.as_ref()[1] as u16) << 8) | self.buffer.as_ref()[2] as u16
    }
    pub fn max_ttl(&self) -> u8 {
        self.buffer.as_ref()[3] >> 4
    }
    pub fn ttl(&self) -> u8 {
        self.buffer.as_ref()[3] & 0xF
    }

    pub fn is_encrypt(&self) -> bool {
        self.buffer.as_ref()[5] & 0x80 == 0x80
    }

    pub fn group_code(&self) -> &[u8] {
        &self.buffer.as_ref()[8..24]
    }
    pub fn src_id(&self) -> &[u8] {
        &self.buffer.as_ref()[24..28]
    }
    pub fn dest_id(&self) -> &[u8] {
        &self.buffer.as_ref()[28..32]
    }
    pub fn payload(&self) -> &[u8] {
        &self.buffer.as_ref()[32..]
    }
    pub fn buffer(&self) -> &[u8] {
        self.buffer.as_ref()
    }

    pub fn into_buffer(self) -> B {
        self.buffer
    }
}
impl<B: AsRef<[u8]> + AsMut<[u8]>> NetPacket<B> {
    pub(crate) fn set_high_flag(&mut self) {
        self.buffer.as_mut()[0] |= 0x80
    }
    pub(crate) fn incr_ttl(&mut self) -> bool {
        let ttl = self.ttl();
        if ttl <= 1 {
            return false;
        }
        self.buffer.as_mut()[3] &= 0xF0 | (ttl - 1);
        true
    }
    pub(crate) fn set_ttl(&mut self, ttl: u8) {
        self.buffer.as_mut()[3] = (ttl << 4) | (ttl & 0xF)
    }

    pub(crate) fn set_protocol(&mut self, protocol_type: ProtocolType) {
        self.buffer.as_mut()[0] = protocol_type.into();
        self.set_high_flag()
    }
    pub(crate) fn reset_data_len(&mut self) {
        let len = self.buffer().len();
        self.buffer.as_mut()[1] = (len >> 8) as u8;
        self.buffer.as_mut()[2] = len as u8;
    }
    #[allow(dead_code)]
    pub(crate) fn set_encrypt_flag(&mut self, is_encrypt: bool) {
        if is_encrypt {
            self.buffer.as_mut()[5] = self.buffer.as_ref()[5] | 0x80
        } else {
            self.buffer.as_mut()[5] = self.buffer.as_ref()[5] & 0x7F
        };
    }
    pub(crate) fn set_group_code(&mut self, group_code: &GroupCode) {
        self.buffer.as_mut()[8..24].copy_from_slice(group_code.as_ref());
    }
    pub(crate) fn set_src_id(&mut self, id: &NodeID) {
        self.buffer.as_mut()[24..28].copy_from_slice(id.as_ref());
    }
    pub(crate) fn set_dest_id(&mut self, id: &NodeID) {
        self.buffer.as_mut()[28..32].copy_from_slice(id.as_ref());
    }
    pub(crate) fn payload_mut(&mut self) -> &mut [u8] {
        &mut self.buffer.as_mut()[32..]
    }

    #[allow(dead_code)]
    pub(crate) fn buffer_mut(&mut self) -> &mut [u8] {
        self.buffer.as_mut()
    }
}

impl<B: AsRef<[u8]>> Debug for NetPacket<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let buf = self.buffer();
        if buf.len() < HEAD_LEN {
            return f.write_str("Invalid Protocol Buffer");
        }
        let data_len = self.data_length();
        let high = buf[0] >> 7;
        let ttl = buf[3];
        let group_code = format!("{:?}", self.group_code());
        let src_id = format!("{:?}", self.src_id());
        let dest_id = format!("{:?}", self.dest_id());
        let payload_size = self.payload().len();
        let protocol_type = match self.protocol() {
            Ok(protocol_type) => format!("{protocol_type:?}"),
            Err(_) => "Unknown".to_string(),
        };
        let s = format!(
            "
   0                                           15                                               31
   0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |{high}|   {0:^14}  |       {1:^30}      |   {2:^9} | {3:^9}   |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                       reserve(32)                                           |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  | {4:^91} |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  | {5:^91} |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  | {6:^91} |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  | {7:^91} |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
		",
            format!("{protocol_type}"),
            format!("{}bytes", data_len),
            format!("{:0b}", ttl >> 4),
            format!("{:0b}", ttl & 0b00001111),
            group_code,
            src_id,
            dest_id,
            format!("{}bytes", payload_size)
        );
        f.write_str(&s)
    }
}

#[cfg(test)]
mod test {
    use crate::protocol::protocol_type::ProtocolType;
    use crate::protocol::{NetPacket, HEAD_LEN};

    #[test]
    fn test_build() {
        let mut buf = [0u8; HEAD_LEN + 4];
        let mut packet = unsafe { NetPacket::new_unchecked(&mut buf) };
        packet.set_ttl(2);
        packet.reset_data_len();
        packet.set_group_code(&100000u128.into());
        packet.set_src_id(&3.into());
        packet.set_dest_id(&2.into());
        packet.set_protocol(ProtocolType::IDRouteQuery);
        println!("{packet:?}");
        assert_eq!(packet.max_ttl(), packet.ttl());
        assert_eq!(packet.max_ttl(), 2);
        assert_eq!(packet.dest_id(), &2_u32.to_be_bytes());
        assert_eq!(packet.src_id(), &3_u32.to_be_bytes());
        assert_eq!(packet.protocol().unwrap(), ProtocolType::IDRouteQuery);
    }
}
