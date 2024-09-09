#[derive(Eq, PartialEq, Copy, Clone, Debug)]
#[repr(u8)]
pub enum ProtocolType {
    UserData,
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
    /// Broadcast to the designated range
    RangeBroadcast,
}
impl TryFrom<u8> for ProtocolType {
    type Error = crate::error::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        const MAX: u8 = ProtocolType::RangeBroadcast as u8;
        match value {
            0..=MAX => unsafe { Ok(std::mem::transmute(value)) },
            val => Err(crate::error::Error::InvalidArgument(format!(
                "Invalid protocol {val}"
            ))),
        }
    }
}

impl Into<u8> for ProtocolType {
    fn into(self) -> u8 {
        self as u8
    }
}

#[cfg(test)]
mod test {
    use super::ProtocolType;

    #[test]
    fn test_new_protocol() {
        assert_eq!(ProtocolType::try_from(3).unwrap(), ProtocolType::PunchReply);
        assert!(ProtocolType::try_from(128).is_err());
    }
}
