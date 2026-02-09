use std::net::Ipv4Addr;

/// Unique identifier for a peer in the P2P network.
///
/// `NodeID` is a 4-byte identifier typically based on an IPv4 address.
/// Each peer in the network must have a unique `NodeID` for routing purposes.
///
/// # Examples
///
/// ```rust
/// use rustp2p::NodeID;
/// use std::net::Ipv4Addr;
///
/// // Create from IPv4 address
/// let node_id: NodeID = Ipv4Addr::new(10, 0, 0, 1).into();
///
/// // Create from u32
/// let node_id: NodeID = 0x0A000001u32.into();
///
/// // Convert back to IPv4
/// let addr: Ipv4Addr = node_id.into();
/// assert_eq!(addr, Ipv4Addr::new(10, 0, 0, 1));
/// ```
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
    /// Returns a broadcast NodeID (255.255.255.255).
    ///
    /// Messages sent to this ID will be broadcast to all peers.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rustp2p::NodeID;
    ///
    /// let broadcast = NodeID::broadcast();
    /// assert!(broadcast.is_broadcast());
    /// ```
    pub fn broadcast() -> NodeID {
        NodeID([255u8; ID_LEN])
    }
    
    /// Returns an unspecified NodeID (0.0.0.0).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rustp2p::NodeID;
    ///
    /// let unspec = NodeID::unspecified();
    /// assert!(unspec.is_unspecified());
    /// ```
    pub fn unspecified() -> NodeID {
        NodeID([0u8; ID_LEN])
    }
    
    /// Checks if this NodeID is unspecified (all zeros).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rustp2p::NodeID;
    ///
    /// let unspec = NodeID::unspecified();
    /// assert!(unspec.is_unspecified());
    /// ```
    pub fn is_unspecified(&self) -> bool {
        let buf = self.as_ref();
        buf.iter().all(|v| *v == 0)
    }
    
    /// Checks if this NodeID is a broadcast address (all 255s).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rustp2p::NodeID;
    ///
    /// let broadcast = NodeID::broadcast();
    /// assert!(broadcast.is_broadcast());
    /// ```
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

/// Group identifier for isolating P2P networks.
///
/// `GroupCode` is a 16-byte identifier that creates isolated P2P networks.
/// Peers with different group codes cannot communicate with each other.
///
/// # Examples
///
/// ```rust
/// use rustp2p::GroupCode;
/// use std::convert::TryFrom;
///
/// // Create from string
/// let group = GroupCode::try_from("my-group").unwrap();
///
/// // Create from u128
/// let group: GroupCode = 12345u128.into();
///
/// // Create unspecified (default)
/// let unspec = GroupCode::unspecified();
/// assert!(unspec.is_unspecified());
/// ```
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
    /// Returns an unspecified GroupCode (all zeros).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rustp2p::GroupCode;
    ///
    /// let unspec = GroupCode::unspecified();
    /// assert!(unspec.is_unspecified());
    /// ```
    pub fn unspecified() -> GroupCode {
        GroupCode([0u8; GROUP_CODE_LEN])
    }
    
    /// Checks if this GroupCode is unspecified (all zeros).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rustp2p::GroupCode;
    ///
    /// let unspec = GroupCode::unspecified();
    /// assert!(unspec.is_unspecified());
    /// ```
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

impl TryFrom<&str> for GroupCode {
    type Error = std::io::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.len() > GROUP_CODE_LEN {
            return Err(std::io::Error::other(format!("The number of bytes in the string exceeds the limit of group code len {GROUP_CODE_LEN}")));
        }
        let mut array = [0u8; 16];
        let bytes = value.as_bytes();
        let len = bytes.len().min(16);
        array[..len].copy_from_slice(&bytes[..len]);
        Ok(array.into())
    }
}

impl TryFrom<String> for GroupCode {
    type Error = std::io::Error;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl TryFrom<&String> for GroupCode {
    type Error = std::io::Error;
    fn try_from(value: &String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
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
