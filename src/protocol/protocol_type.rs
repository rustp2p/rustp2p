#[derive(Eq, PartialEq, Copy, Clone, Debug)]
#[repr(u8)]
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
    Unknown = 255,
}

impl From<u8> for ProtocolType {
    fn from(value: u8) -> Self {
        const MAX: u8 = ProtocolType::UserData as u8;
        let mut ret = ProtocolType::Unknown;
        match value {
            0..=MAX => unsafe {
                let ptr = &mut ret as *mut _ as *mut u8;
                *ptr = value;
            },
            _ => {}
        }
        ret
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
        assert_eq!(ProtocolType::from(2), ProtocolType::PunchReply);
        assert_eq!(ProtocolType::from(128), ProtocolType::Unknown);
    }
}
