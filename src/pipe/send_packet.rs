use std::ops::{Deref, DerefMut};

use bytes::BytesMut;

use crate::protocol::node_id::{GroupCode, NodeID};
use crate::protocol::protocol_type::ProtocolType;
use crate::protocol::{NetPacket, HEAD_LEN};

#[derive(Clone)]
pub struct SendPacket {
    buf: BytesMut,
}
impl From<&[u8]> for SendPacket {
    fn from(value: &[u8]) -> Self {
        let mut packet = SendPacket::with_capacity(value.len());
        packet.set_payload(value);
        packet
    }
}

impl SendPacket {
    pub fn with_capacity(capacity: usize) -> Self {
        assert!(capacity < u16::MAX as _);
        let buf = BytesMut::with_capacity(HEAD_LEN + capacity);
        Self::with_bytes_mut(buf)
    }
    pub fn with_bytes_mut(mut buf: BytesMut) -> Self {
        buf.resize(HEAD_LEN, 0);
        let mut send_packet = Self { buf };
        let mut packet = NetPacket::unchecked(send_packet.buf_mut());
        packet.set_protocol(ProtocolType::UserData);
        packet.set_ttl(15);
        send_packet
    }
    pub fn reserve(&mut self, additional: usize) {
        self.buf.reserve(additional);
    }
    /// Copies all elements from src into self, using a memcpy.
    /// The length of src must be the same as self.
    /// # Panics
    /// This function will panic if the two slices have different lengths.
    pub fn set_payload(&mut self, src: &[u8]) {
        assert!(src.len() < u16::MAX as _);
        self.buf.truncate(HEAD_LEN);
        self.buf.extend_from_slice(src);
        let mut packet = NetPacket::unchecked(self.buf_mut());
        packet.reset_data_len();
    }
    /// # Safety
    /// Sets the length of the buffer.
    /// This will explicitly set the size of the buffer without actually modifying the data,
    /// so it is up to the caller to ensure that the data has been initialized.
    pub unsafe fn set_payload_len(&mut self, payload_len: usize) {
        assert!(payload_len < u16::MAX as _);
        self.buf.set_len(HEAD_LEN + payload_len);
        let mut packet = NetPacket::unchecked(self.buf_mut());
        packet.reset_data_len();
    }
    pub unsafe fn set_payload_len_raw(&mut self, payload_len: usize) {
        self.buf.set_len(HEAD_LEN + payload_len);
    }
    pub fn clear(&mut self) {
        unsafe {
            self.set_payload_len(0);
            self.buf.fill(0);

            let mut packet = NetPacket::unchecked(self.buf_mut());
            packet.set_protocol(ProtocolType::UserData);
            packet.set_ttl(15);
        }
    }
    pub fn set_ttl(&mut self, ttl: u8) {
        let ttl = ttl & 0xF;
        self.buf[3] = (ttl << 4) | ttl
    }
    pub fn set_group_code(&mut self, code: &GroupCode) {
        let mut packet = NetPacket::unchecked(self.buf_mut());
        packet.set_group_code(code);
    }
    pub fn set_src_id(&mut self, id: &NodeID) {
        let mut packet = NetPacket::unchecked(self.buf_mut());
        packet.set_src_id(id);
    }
    pub fn set_dest_id(&mut self, id: &NodeID) {
        let mut packet = NetPacket::unchecked(self.buf_mut());
        packet.set_dest_id(id);
    }
}

impl Deref for SendPacket {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.buf[HEAD_LEN..]
    }
}
impl DerefMut for SendPacket {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.buf[HEAD_LEN..]
    }
}
impl SendPacket {
    pub(crate) fn buf(&self) -> &[u8] {
        &self.buf
    }
    pub(crate) fn buf_mut(&mut self) -> &mut [u8] {
        &mut self.buf
    }
    pub(crate) fn into_buf(self) -> BytesMut {
        self.buf
    }
}
