/*
  Broadcast to the designated range
   0                                            15                                              31
   0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |    unused(8)        | ID length(8)          |   protocol (8)       |max ttl(4) | cur ttl(4) |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                           src ID                                            |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                          dest ID                                            |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  | Range ID num(8)     |               broadcast ID...                                         |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                      broadcast ID                                           |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                          payload                                            |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  protocol = ProtocolType::RangeBroadcast
*/

use crate::error::*;
use crate::protocol::node_id::NodeID;
use crate::protocol::protocol_type::ProtocolType;
use crate::protocol::NetPacket;

pub struct RangeBroadcastPacket<B> {
    id_len: u8,
    buffer: B,
}

impl<B: AsRef<[u8]>> RangeBroadcastPacket<B> {
    pub fn unchecked(buffer: B, id_len: u8) -> RangeBroadcastPacket<B> {
        Self { id_len, buffer }
    }
    pub fn new(buffer: B, id_len: u8) -> Result<RangeBroadcastPacket<B>> {
        if !NodeID::complies_with_length(id_len) {
            return Err(Error::InvalidArgument("id_len".to_string()))?;
        }
        let len = buffer.as_ref().len();
        if len == 0 {
            return Err(Error::Overflow {
                cap: 0,
                required: 4,
            });
        }
        let packet = Self::unchecked(buffer, id_len);
        let head_len = packet.head_len();
        if len < head_len {
            return Err(Error::Overflow {
                cap: len,
                required: head_len,
            });
        }
        Ok(packet)
    }
    pub fn head_len(&self) -> usize {
        1 + self.range_id_num() as usize * self.id_len as usize
    }
    pub fn range_id_num(&self) -> u8 {
        self.buffer.as_ref()[0]
    }
    pub fn payload(&self) -> &[u8] {
        &self.buffer.as_ref()[self.head_len()..]
    }
    pub fn buffer(&self) -> &[u8] {
        self.buffer.as_ref()
    }
    pub fn iter(&self) -> RangeBroadcastIter<B> {
        RangeBroadcastIter {
            packet: self,
            index: 0,
        }
    }
}
impl<B: AsRef<[u8]> + AsMut<[u8]>> RangeBroadcastPacket<B> {
    pub fn payload_mut(&mut self) -> &mut [u8] {
        let head_len = self.head_len();
        &mut self.buffer.as_mut()[head_len..]
    }
    pub fn buffer_mut(&mut self) -> &mut [u8] {
        self.buffer.as_mut()
    }
}

pub struct RangeBroadcastIter<'a, B> {
    packet: &'a RangeBroadcastPacket<B>,
    index: usize,
}

impl<B: AsRef<[u8]>> Iterator for RangeBroadcastIter<'_, B> {
    type Item = NodeID;

    fn next(&mut self) -> Option<Self::Item> {
        let range_id_num = self.packet.range_id_num() as usize;
        if range_id_num == self.index {
            return None;
        }
        let id_len = self.packet.id_len as usize;
        let start = 1 + self.index * id_len;
        let end = start + id_len;
        self.index += 1;
        Some(NodeID::new(&self.packet.buffer()[start..end]).unwrap())
    }
}
pub struct Builder;
impl Builder {
    pub fn calculate_len(list: &[NodeID], payload_len: usize) -> Result<(usize, usize, usize)> {
        if list.is_empty() {
            return Err(Error::InvalidArgument("".into()));
        }
        if list.len() > 255 {
            return Err(Error::InvalidArgument("".into()));
        }
        let id_len = list[0].len();
        if list.iter().any(|node_id| node_id.len() != id_len) {
            return Err(Error::InvalidArgument("".into()));
        }
        let id_num = list.len();
        let out_packet_head = 4 + id_len * 2;
        let broadcast_packet_head = 1 + id_len * id_num;
        let len = out_packet_head + broadcast_packet_head + payload_len;
        Ok((id_len, broadcast_packet_head, len))
    }
    pub fn build_range_broadcast(
        list: &[NodeID],
        broadcast_payload: &[u8],
    ) -> Result<NetPacket<Vec<u8>>> {
        let (id_len, broadcast_packet_head, len) =
            Self::calculate_len(list, broadcast_payload.len())?;
        let mut packet = NetPacket::unchecked(vec![0; len]);
        packet.set_protocol(ProtocolType::RangeBroadcast);
        packet.set_id_length(id_len as _);
        packet.set_ttl(1);
        let packet_payload = packet.payload_mut();
        packet_payload[0] = list.len() as u8;
        for (index, id) in list.iter().enumerate() {
            let start = 1 + index * id_len;
            let end = start + id_len;
            packet_payload[start..end].copy_from_slice(id.as_ref());
        }
        packet_payload[broadcast_packet_head..].copy_from_slice(broadcast_payload);
        Ok(packet)
    }
}
#[cfg(test)]
mod test {
    use crate::protocol::broadcast::{Builder, RangeBroadcastPacket};
    use crate::protocol::node_id::NodeID;

    #[test]
    fn test_build() {
        let list = vec![NodeID::from(1), NodeID::from(2), NodeID::from(3)];
        let payload = [1; 100];
        test_build0(list, &payload);
    }
    fn test_build0(list: Vec<NodeID>, payload: &[u8]) {
        let packet = Builder::build_range_broadcast(&list, payload).unwrap();
        let packet = RangeBroadcastPacket::new(packet.payload(), packet.id_length()).unwrap();
        assert_eq!(packet.iter().count(), list.len());
        for (index, node_id) in packet.iter().enumerate() {
            assert_eq!(node_id, list[index]);
        }
    }
}
