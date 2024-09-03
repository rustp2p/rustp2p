#[derive(Eq, PartialEq, Copy, Clone, Debug)]
pub enum ProtocolType {
    PunchConsult,
    PunchRequest,
    PunchReply,
    /// Maintain mapping
    EchoRequest,
    EchoReply,
    /// Detecting RTT
    TimestampRequest,
    TimestampReply,
    /// ID route query
    IDRouteQuery,
    IDRouteReply,
    UserData,
    Unknown(u8),
}

impl From<u8> for ProtocolType {
    fn from(value: u8) -> Self {
        match value {
            1 => ProtocolType::PunchConsult,
            2 => ProtocolType::PunchRequest,
            3 => ProtocolType::PunchReply,
            4 => ProtocolType::EchoRequest,
            5 => ProtocolType::EchoReply,
            6 => ProtocolType::TimestampRequest,
            7 => ProtocolType::TimestampReply,
            8 => ProtocolType::IDRouteQuery,
            9 => ProtocolType::IDRouteReply,
            10 => ProtocolType::UserData,
            val => ProtocolType::Unknown(val),
        }
    }
}
impl Into<u8> for ProtocolType {
    fn into(self) -> u8 {
        match self {
            ProtocolType::PunchConsult => 1,
            ProtocolType::PunchRequest => 2,
            ProtocolType::PunchReply => 3,
            ProtocolType::EchoRequest => 4,
            ProtocolType::EchoReply => 5,
            ProtocolType::TimestampRequest => 6,
            ProtocolType::TimestampReply => 7,
            ProtocolType::IDRouteQuery => 8,
            ProtocolType::IDRouteReply => 9,
            ProtocolType::UserData => 10,
            ProtocolType::Unknown(val) => val,
        }
    }
}
