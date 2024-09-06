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
    pub fn data(&self) -> &[u8] {
        &self.buf[self.head_reserve..]
    }
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.buf[self.head_reserve..]
    }
    pub fn set_payload_len(&mut self, payload_len: usize) {
        let len = self.head_reserve + payload_len;
        assert!(self.buf.len() <= len);
        self.len = len;
    }
    pub(crate) fn buf_mut(&mut self) -> &mut [u8] {
        &mut self.buf[..self.len]
    }
}
