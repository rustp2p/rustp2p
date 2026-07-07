use crate::{GroupCode, PeerAddr, PeerId};
use serde::{Deserialize, Serialize};
use std::io;

pub const VERSION: u8 = 1;
pub const HEADER_LEN: usize = 104;

#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[repr(u8)]
pub enum ProtocolType {
    MessageData = 0,
    PunchConsultRequest = 1,
    PunchConsultReply = 2,
    PunchRequest = 3,
    PunchReply = 4,
    EchoRequest = 5,
    EchoReply = 6,
    TimestampRequest = 7,
    TimestampReply = 8,
    IDRouteQuery = 9,
    IDRouteReply = 10,
    RangeBroadcast = 11,
    QuicRelay = 12,
    ReliableRelayOpen = 13,
    ReliableRelayClose = 14,
}

impl TryFrom<u8> for ProtocolType {
    type Error = io::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::MessageData),
            1 => Ok(Self::PunchConsultRequest),
            2 => Ok(Self::PunchConsultReply),
            3 => Ok(Self::PunchRequest),
            4 => Ok(Self::PunchReply),
            5 => Ok(Self::EchoRequest),
            6 => Ok(Self::EchoReply),
            7 => Ok(Self::TimestampRequest),
            8 => Ok(Self::TimestampReply),
            9 => Ok(Self::IDRouteQuery),
            10 => Ok(Self::IDRouteReply),
            11 => Ok(Self::RangeBroadcast),
            12 => Ok(Self::QuicRelay),
            13 => Ok(Self::ReliableRelayOpen),
            14 => Ok(Self::ReliableRelayClose),
            val => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid protocol type: {val}"),
            )),
        }
    }
}

impl From<ProtocolType> for u8 {
    fn from(value: ProtocolType) -> Self {
        value as u8
    }
}

#[derive(Clone, Debug)]
pub struct Packet {
    buf: Vec<u8>,
}

impl Packet {
    pub fn build(
        protocol: ProtocolType,
        group: GroupCode,
        src: PeerId,
        dest: PeerId,
        max_ttl: u8,
        payload: &[u8],
    ) -> io::Result<Self> {
        let len = HEADER_LEN
            .checked_add(payload.len())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "packet too large"))?;
        if len > u32::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "packet too large",
            ));
        }
        let mut buf = vec![0u8; len];
        buf[0] = 0x80 | u8::from(protocol);
        buf[1] = VERSION;
        buf[2] = 0;
        buf[3] = max_ttl;
        buf[4] = max_ttl;
        buf[5..9].copy_from_slice(&(len as u32).to_be_bytes());
        buf[9..24].fill(0);
        buf[24..40].copy_from_slice(group.as_ref());
        buf[40..72].copy_from_slice(src.as_ref());
        buf[72..104].copy_from_slice(dest.as_ref());
        buf[HEADER_LEN..].copy_from_slice(payload);
        Ok(Self { buf })
    }

    pub fn parse(buf: Vec<u8>) -> io::Result<Self> {
        if buf.len() < HEADER_LEN {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "short packet"));
        }
        if buf[0] & 0x80 == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not a rustp2p packet",
            ));
        }
        if buf[1] != VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported packet version: {}", buf[1]),
            ));
        }
        let len = u32::from_be_bytes(buf[5..9].try_into().unwrap()) as usize;
        if len != buf.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "packet length mismatch",
            ));
        }
        Ok(Self { buf })
    }

    pub fn protocol(&self) -> io::Result<ProtocolType> {
        (self.buf[0] & 0x7f).try_into()
    }

    pub fn group(&self) -> GroupCode {
        let mut out = [0u8; 16];
        out.copy_from_slice(&self.buf[24..40]);
        GroupCode(out)
    }

    pub fn src(&self) -> PeerId {
        let mut out = [0u8; 32];
        out.copy_from_slice(&self.buf[40..72]);
        PeerId(out)
    }

    pub fn dest(&self) -> PeerId {
        let mut out = [0u8; 32];
        out.copy_from_slice(&self.buf[72..104]);
        PeerId(out)
    }

    pub fn ttl(&self) -> u8 {
        self.buf[4]
    }

    pub fn max_ttl(&self) -> u8 {
        self.buf[3]
    }

    pub fn decrement_ttl(&mut self) -> bool {
        if self.buf[4] <= 1 {
            return false;
        }
        self.buf[4] -= 1;
        true
    }

    pub fn payload(&self) -> &[u8] {
        &self.buf[HEADER_LEN..]
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    #[cfg(test)]
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RouteReplyPayload {
    pub peers: Vec<RouteEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RouteEntry {
    pub peer: PeerAddr,
    pub metric: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimestampPayload {
    pub millis: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PunchPayload {
    pub peer: PeerAddr,
    pub nat_info: Option<rust_p2p_core::nat::NatInfo>,
}

pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::{Packet, ProtocolType, HEADER_LEN};
    use crate::{GroupCode, PeerId};

    #[test]
    fn packet_round_trip() {
        let src = PeerId([1; 32]);
        let dest = PeerId([2; 32]);
        let group = GroupCode([3; 16]);
        let packet =
            Packet::build(ProtocolType::MessageData, group, src, dest, 8, b"hello").unwrap();
        assert_eq!(packet.as_bytes().len(), HEADER_LEN + 5);
        assert_eq!(packet.protocol().unwrap(), ProtocolType::MessageData);
        assert_eq!(packet.group(), group);
        assert_eq!(packet.src(), src);
        assert_eq!(packet.dest(), dest);
        assert_eq!(packet.payload(), b"hello");
        let parsed = Packet::parse(packet.into_bytes()).unwrap();
        assert_eq!(parsed.payload(), b"hello");
    }
}
