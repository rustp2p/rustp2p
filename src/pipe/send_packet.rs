use std::ops::{Deref, DerefMut};

use crate::protocol::node_id::NodeID;
use crate::protocol::{NetPacket, HEAD_LEN};

pub struct SendPacket {
    buf: Vec<u8>,
    len: usize,
}

impl SendPacket {
    pub(crate) fn allocate(capacity: usize) -> Self {
        // This must be safe since `data` is marked with safe and returns a slice to the user to read from it.
        let mut buf = vec![0u8; HEAD_LEN + capacity];
        let mut packet = NetPacket::unchecked(&mut buf);
        packet.set_high_flag();
        packet.set_ttl(15);
        Self { buf, len: HEAD_LEN }
    }
    pub fn set_ttl(&mut self, ttl: u8) {
        let ttl = ttl & 0xF;
        self.buf[3] = (ttl << 4) | ttl
    }
    pub fn set_dest_id(&mut self, id: &NodeID) {
        let mut packet = NetPacket::unchecked(self.buf_mut());
        packet.set_dest_id(id);
    }
    pub fn set_src_id(&mut self, id: &NodeID) {
        let mut packet = NetPacket::unchecked(self.buf_mut());
        packet.set_src_id(id);
    }
    pub fn data(&self) -> &[u8] {
        &self.buf[HEAD_LEN..]
    }
    pub fn set_payload(&mut self, buf: &[u8]) -> Result<(), std::io::Error> {
        let payload_part = &mut self.buf[HEAD_LEN..];
        let data_len = buf.len();
        if data_len > payload_part.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::OutOfMemory,
                format!(
                    "The maximum capacity len:{} required len:{}",
                    payload_part.len(),
                    data_len
                ),
            ));
        }
        payload_part[0..data_len].copy_from_slice(buf);
        self.set_payload_len(data_len);
        Ok(())
    }
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.buf[HEAD_LEN..]
    }
    pub fn set_payload_len(&mut self, payload_len: usize) {
        let len = HEAD_LEN + payload_len;
        assert!(self.buf.len() >= len);
        self.len = len;
        let mut packet = NetPacket::unchecked(self.buf_mut());
        packet.reset_data_len();
    }
    pub(crate) fn buf_mut(&mut self) -> &mut [u8] {
        &mut self.buf[..self.len]
    }
    pub(crate) fn buf(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

impl Deref for SendPacket {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.data()
    }
}
impl DerefMut for SendPacket {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.data_mut()
    }
}
