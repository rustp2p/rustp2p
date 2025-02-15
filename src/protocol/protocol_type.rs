use std::io;

#[derive(Eq, PartialEq, Copy, Clone, Debug)]
#[repr(u8)]
pub enum ProtocolType {
    UserData = 0,
    PunchConsultRequest = 1,
    PunchConsultReply = 2,
    PunchRequest = 3,
    PunchReply = 4,
    /// Maintain mapping
    EchoRequest = 5,
    EchoReply = 6,
    /// Detecting RTT
    TimestampRequest = 7,
    TimestampReply = 8,
    /// ID route query
    IDRouteQuery = 9,
    IDRouteReply = 10,
    /// Broadcast to the designated range
    RangeBroadcast = 11,
    IDQuery = 12,
    IDReply = 13,
}

impl TryFrom<u8> for ProtocolType {
    type Error = io::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        const MAX: u8 = ProtocolType::IDReply as u8;
        match value {
            0..=MAX => unsafe { Ok(std::mem::transmute::<u8, ProtocolType>(value)) },
            val => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid protocol:{val}"),
            )),
        }
    }
}

impl From<ProtocolType> for u8 {
    fn from(val: ProtocolType) -> Self {
        val as u8
    }
}

#[cfg(test)]
mod test {
    use super::ProtocolType;

    #[test]
    fn test_new_protocol() {
        assert_eq!(ProtocolType::try_from(4).unwrap(), ProtocolType::PunchReply);
        assert!(ProtocolType::try_from(128).is_err());
    }
}
