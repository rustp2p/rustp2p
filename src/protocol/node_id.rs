use std::net::Ipv4Addr;

#[repr(transparent)]
#[derive(Hash, Eq, PartialEq, Ord, PartialOrd, Copy, Clone, Debug)]
pub struct NodeID([u8; ID_LEN]);
pub const ID_LEN: usize = 4;

impl AsRef<[u8]> for NodeID {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl NodeID {
    pub fn broadcast() -> NodeID {
        NodeID([255u8; ID_LEN])
    }
    pub fn unspecified() -> NodeID {
        NodeID([0u8; ID_LEN])
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

impl From<NodeID> for [u8; ID_LEN] {
    fn from(value: NodeID) -> Self {
        value.0
    }
}
impl From<NodeID> for u32 {
    fn from(value: NodeID) -> Self {
        u32::from_be_bytes(value.0)
    }
}
impl From<NodeID> for i32 {
    fn from(value: NodeID) -> Self {
        i32::from_be_bytes(value.0)
    }
}
impl From<NodeID> for Ipv4Addr {
    fn from(value: NodeID) -> Self {
        Ipv4Addr::from(value.0)
    }
}

impl From<[u8; ID_LEN]> for NodeID {
    fn from(value: [u8; ID_LEN]) -> Self {
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
            ID_LEN => Ok(NodeID(value.try_into().unwrap())),
            _ => Err(std::io::Error::from(std::io::ErrorKind::InvalidData)),
        }
    }
}

#[repr(transparent)]
#[derive(Hash, Eq, PartialEq, Ord, PartialOrd, Copy, Clone, Debug)]
pub struct GroupCode([u8; GROUP_CODE_LEN]);

impl Default for GroupCode {
    fn default() -> Self {
        GroupCode::unspecified()
    }
}
pub const GROUP_CODE_LEN: usize = 16;
impl GroupCode {
    pub fn unspecified() -> GroupCode {
        GroupCode([0u8; GROUP_CODE_LEN])
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
            GROUP_CODE_LEN => Ok(GroupCode(value.try_into().unwrap())),
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
impl From<[u8; GROUP_CODE_LEN]> for GroupCode {
    fn from(value: [u8; GROUP_CODE_LEN]) -> Self {
        GroupCode(value)
    }
}
