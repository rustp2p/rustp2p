use std::ops::{Deref, DerefMut};

use crate::protocol::{NetPacket, HEAD_LEN};

pub struct SendPacket {
    buf: Vec<u8>,
    len: usize,
}

impl SendPacket {
    pub(crate) fn new_capacity(capacity: usize) -> Self {
        let mut buf = Vec::with_capacity(HEAD_LEN + capacity);
        unsafe {
            buf.set_len(HEAD_LEN + capacity);
        }
        Self { buf, len: HEAD_LEN }
    }
    pub fn set_ttl(&mut self, ttl: u8) {
        let ttl = ttl & 0xF;
        self.buf[3] = (ttl << 4) | ttl
    }
    pub fn data(&self) -> &[u8] {
        &self.buf[HEAD_LEN..]
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
