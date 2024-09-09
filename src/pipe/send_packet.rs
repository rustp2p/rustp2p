use std::ops::{Deref, DerefMut};

pub struct SendPacket {
    buf: Vec<u8>,
    head_reserve: usize,
    len: usize,
}

impl SendPacket {
    pub(crate) fn new_capacity(head_reserve: usize, capacity: usize) -> Self {
        let buf = vec![0; capacity];
        Self {
            buf,
            head_reserve,
            len: head_reserve,
        }
    }
    pub fn set_ttl(&mut self, ttl: u8) {
        let ttl = ttl & 0xF;
        self.buf[3] = (ttl << 4) | ttl
    }
    pub fn data(&self) -> &[u8] {
        &self.buf[self.head_reserve..]
    }
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.buf[self.head_reserve..]
    }
    pub fn set_payload_len(&mut self, payload_len: usize) {
        let len = self.head_reserve + payload_len;
        assert!(self.buf.len() >= len);
        self.len = len;
    }
    pub(crate) fn buf_mut(&mut self) -> &mut [u8] {
        &mut self.buf[..self.len]
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
