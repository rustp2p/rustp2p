use crate::{PeerId, PeerInfo};
use serde::{Deserialize, Serialize};
use std::io;

pub const VERSION: u8 = 2;
const FIXED_HEADER_LEN: usize = 13;

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
    HelloRequest = 13,
    HelloReply = 14,
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
            13 => Ok(Self::HelloRequest),
            14 => Ok(Self::HelloReply),
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
    src: PeerId,
    dest: PeerId,
    payload_offset: usize,
}

impl Packet {
    pub fn build(
        protocol: ProtocolType,
        src: PeerId,
        dest: PeerId,
        max_ttl: u8,
        payload: &[u8],
    ) -> io::Result<Self> {
        let src_bytes = src.as_str().as_bytes();
        let dest_bytes = dest.as_str().as_bytes();
        if src_bytes.len() > u16::MAX as usize || dest_bytes.len() > u16::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "peer id exceeds 65535 bytes",
            ));
        }
        let len = FIXED_HEADER_LEN
            .checked_add(src_bytes.len())
            .and_then(|n| n.checked_add(dest_bytes.len()))
            .and_then(|n| n.checked_add(payload.len()))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "packet too large"))?;
        if len > u32::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "packet too large",
            ));
        }

        let mut buf = Vec::with_capacity(len);
        buf.push(0x80 | u8::from(protocol));
        buf.push(VERSION);
        buf.push(0);
        buf.push(max_ttl);
        buf.push(max_ttl);
        buf.extend_from_slice(&(len as u32).to_be_bytes());
        buf.extend_from_slice(&(src_bytes.len() as u16).to_be_bytes());
        buf.extend_from_slice(&(dest_bytes.len() as u16).to_be_bytes());
        buf.extend_from_slice(src_bytes);
        buf.extend_from_slice(dest_bytes);
        let payload_offset = buf.len();
        buf.extend_from_slice(payload);

        Ok(Self {
            buf,
            src,
            dest,
            payload_offset,
        })
    }

    pub fn parse(buf: Vec<u8>) -> io::Result<Self> {
        if buf.len() < FIXED_HEADER_LEN {
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
        let src_len = u16::from_be_bytes(buf[9..11].try_into().unwrap()) as usize;
        let dest_len = u16::from_be_bytes(buf[11..13].try_into().unwrap()) as usize;
        let src_start = FIXED_HEADER_LEN;
        let dest_start = src_start
            .checked_add(src_len)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid src length"))?;
        let payload_offset = dest_start
            .checked_add(dest_len)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid dest length"))?;
        if payload_offset > buf.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "peer id length exceeds packet",
            ));
        }
        let src = std::str::from_utf8(&buf[src_start..dest_start])
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("src peer id: {e}")))?
            .to_string();
        let dest = std::str::from_utf8(&buf[dest_start..payload_offset])
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("dest peer id: {e}")))?
            .to_string();
        Ok(Self {
            buf,
            src: PeerId::from(src),
            dest: PeerId::from(dest),
            payload_offset,
        })
    }

    pub fn protocol(&self) -> io::Result<ProtocolType> {
        (self.buf[0] & 0x7f).try_into()
    }

    pub fn src(&self) -> PeerId {
        self.src.clone()
    }

    pub fn dest(&self) -> PeerId {
        self.dest.clone()
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
        &self.buf[self.payload_offset..]
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
pub struct HelloPayload {
    pub peer: PeerInfo,
    pub peers: Vec<RouteEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RouteReplyPayload {
    pub peers: Vec<RouteEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RouteEntry {
    pub peer: PeerInfo,
    pub metric: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StreamHeader {
    pub src: PeerId,
    pub dest: PeerId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum StreamFrame {
    User(StreamHeader),
    HelloRequest { src: PeerId },
    HelloReply(HelloPayload),
    RouteQuery { src: PeerId },
    RouteReply(RouteReplyPayload),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DatagramFrame {
    User {
        id: u64,
        src: PeerId,
        dest: PeerId,
        payload: Vec<u8>,
    },
}

pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::{Packet, ProtocolType};
    use crate::PeerId;

    #[test]
    fn packet_round_trip() {
        let src = PeerId::from("node-a");
        let dest = PeerId::from("node-b");
        let packet = Packet::build(
            ProtocolType::MessageData,
            src.clone(),
            dest.clone(),
            8,
            b"hello",
        )
        .unwrap();
        assert_eq!(packet.protocol().unwrap(), ProtocolType::MessageData);
        assert_eq!(packet.src(), src);
        assert_eq!(packet.dest(), dest);
        assert_eq!(packet.payload(), b"hello");
        let parsed = Packet::parse(packet.into_bytes()).unwrap();
        assert_eq!(parsed.payload(), b"hello");
    }
}
