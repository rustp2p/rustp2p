#[non_exhaustive]
pub enum NodeID {
    Bit32([u8; 4]),
    Bit64([u8; 8]),
    Bit128([u8; 16]),
}

impl AsRef<[u8]> for NodeID {
    fn as_ref(&self) -> &[u8] {
        match self {
            NodeID::Bit32(v) => &v[..],
            NodeID::Bit64(v) => &v[..],
            NodeID::Bit128(v) => &v[..],
        }
    }
}
