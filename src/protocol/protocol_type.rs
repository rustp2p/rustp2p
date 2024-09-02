#[derive(Eq, PartialEq, Copy, Clone, Debug)]
pub enum ProtocolType {
    Unknown(u8),
}

impl From<u8> for ProtocolType {
    fn from(value: u8) -> Self {
        match value {
            val => ProtocolType::Unknown(val),
        }
    }
}
impl Into<u8> for ProtocolType {
    fn into(self) -> u8 {
        match self {
            ProtocolType::Unknown(val) => val,
        }
    }
}
