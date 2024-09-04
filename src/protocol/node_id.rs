#[non_exhaustive]
#[derive(Hash, Eq, PartialEq, Copy, Clone)]
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
impl NodeID {
    pub fn new(buf: &[u8]) -> std::io::Result<Self> {
        match buf.len() {
            4 => Ok(NodeID::Bit32(buf.try_into().unwrap())),
            8 => Ok(NodeID::Bit32(buf.try_into().unwrap())),
            16 => Ok(NodeID::Bit32(buf.try_into().unwrap())),
            _ => Err(std::io::Error::from(std::io::ErrorKind::InvalidData)),
        }
    }
    pub fn is_unspecified(&self) -> bool {
        let buf = self.as_ref();
        buf.iter().all(|v| *v == 0)
    }
    pub fn is_broadcast(&self) -> bool {
        let buf = self.as_ref();
        buf.iter().all(|v| *v == 1)
    }
    pub fn len(&self) -> usize {
        self.as_ref().len()
    }
}

macro_rules! impl_from_integer {
	($p0:ident:$t0:ty)=>{
		impl From<$t0> for $crate::protocol::node_id::NodeID{
			fn from(value: $t0) -> Self {
				$crate::protocol::node_id::NodeID::$p0(value.to_be_bytes())
			}
		}
		impl TryFrom<$crate::protocol::node_id::NodeID> for $t0{
			type Error = std::io::Error;

			fn try_from(value: $crate::protocol::node_id::NodeID) -> Result<Self, Self::Error> {
				match value{
					$crate::protocol::node_id::NodeID::$p0(v) => Ok(<$t0>::from_be_bytes(v)),
					_=>Err(std::io::Error::other("invalid parse"))
				}
			}
		}
	};
	($p0:ident:$t0:ty, $($p:ident:$t:ty),*) => {
		impl From<$t0> for $crate::protocol::node_id::NodeID{
			fn from(value: $t0) -> Self {
				$crate::protocol::node_id::NodeID::$p0(value.to_be_bytes())
			}
		}
		impl TryFrom<$crate::protocol::node_id::NodeID> for $t0{
			type Error = std::io::Error;

			fn try_from(value: $crate::protocol::node_id::NodeID) -> Result<Self, Self::Error> {
				match value{
					$crate::protocol::node_id::NodeID::$p0(v) => Ok(<$t0>::from_be_bytes(v)),
					_=>Err(std::io::Error::other("invalid parse"))
				}
			}
		}
		impl_from_integer!{$($p:$t),*}
	};
}

impl_from_integer! {
    Bit32:i32,
    Bit32:u32,
    Bit64:i64,
    Bit64:u64,
    Bit128:i128,
    Bit128:u128
}
