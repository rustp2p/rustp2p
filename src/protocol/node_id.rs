use std::net::Ipv4Addr;

#[repr(transparent)]
#[derive(Hash, Eq, PartialEq, Ord, PartialOrd, Copy, Clone, Debug)]
pub struct NodeID([u8; 4]);
pub const ID_LEN: usize = 4;

impl AsRef<[u8]> for NodeID {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl NodeID {
    pub fn broadcast() -> NodeID {
        NodeID([255u8; 4])
    }
    pub fn unspecified() -> NodeID {
        NodeID([0u8; 4])
    }
    pub fn is_unspecified(&self) -> bool {
        let buf = self.as_ref();
        buf.iter().all(|v| *v == 0)
    }
    pub fn is_broadcast(&self) -> bool {
        let buf = self.as_ref();
        buf.iter().all(|v| *v == 255)
    }
}

impl From<[u8; 4]> for NodeID {
    fn from(value: [u8; 4]) -> Self {
        NodeID(value)
    }
}

impl From<Ipv4Addr> for NodeID {
    fn from(value: Ipv4Addr) -> Self {
        NodeID(value.octets())
    }
}
impl From<u32> for NodeID {
    fn from(value: u32) -> Self {
        NodeID(value.to_be_bytes())
    }
}
impl From<i32> for NodeID {
    fn from(value: i32) -> Self {
        NodeID(value.to_be_bytes())
    }
}
impl TryFrom<&[u8]> for NodeID {
    type Error = std::io::Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        match value.len() {
            4 => Ok(NodeID(value.try_into().unwrap())),
            _ => Err(std::io::Error::from(std::io::ErrorKind::InvalidData)),
        }
    }
}

#[repr(transparent)]
#[derive(Hash, Eq, PartialEq, Ord, PartialOrd, Copy, Clone, Debug)]
pub struct GroupCode([u8; 16]);

pub const GROUP_CODE_LEN: usize = 16;
impl GroupCode {
    pub fn unspecified() -> GroupCode {
        GroupCode([0u8; 16])
    }
    pub fn is_unspecified(&self) -> bool {
        let buf = self.as_ref();
        buf.iter().all(|v| *v == 0)
    }
}

impl AsRef<[u8]> for GroupCode {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl TryFrom<&[u8]> for GroupCode {
    type Error = std::io::Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        match value.len() {
            16 => Ok(GroupCode(value.try_into().unwrap())),
            _ => Err(std::io::Error::from(std::io::ErrorKind::InvalidData)),
        }
    }
}
impl From<u128> for GroupCode {
    fn from(value: u128) -> Self {
        GroupCode(value.to_be_bytes())
    }
}
impl From<i128> for GroupCode {
    fn from(value: i128) -> Self {
        GroupCode(value.to_be_bytes())
    }
}
impl From<[u8; 16]> for GroupCode {
    fn from(value: [u8; 16]) -> Self {
        GroupCode(value)
    }
}
