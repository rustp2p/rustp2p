/*
  Query all reachable IDs of the other party

   0                                            15                                              31
   0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |    unused(8)        | ID length(8)          |   protocol (8)       |max ttl(4) | cur ttl(4) |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                           src ID                                            |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                          dest ID                                            |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                  offset(16)                 |                 unused(16)                    |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  protocol = ProtocolType::IDRouteQuery
  dest ID = 0
  ttl = 1
*/

/*
   0                                            15                                              31
   0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |    unused(8)        | ID length(8)          |   protocol (8)       |max ttl(4) | cur ttl(4) |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                           src ID                                            |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                          dest ID                                            |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |   current id num(8) |        unused(8)      |            all id num(16)                     |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                          ID metric                                          |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                     Reachable ID 1                                          |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                     Reachable ID 2                                          |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                     Reachable ID ...                                        |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  protocol = ProtocolType::IDRouteReply
*/
use crate::error::*;
use crate::protocol::node_id::NodeID;
use crate::protocol::protocol_type::ProtocolType;
use crate::protocol::NetPacket;

pub struct IDRouteReplyPacket<B> {
    id_len: u8,
    buffer: B,
}
impl<B: AsRef<[u8]>> IDRouteReplyPacket<B> {
    pub fn unchecked(buffer: B, id_len: u8) -> IDRouteReplyPacket<B> {
        Self { id_len, buffer }
    }
    pub fn new(buffer: B, id_len: u8) -> Result<IDRouteReplyPacket<B>> {
        if !NodeID::complies_with_length(id_len) {
            return Err(Error::InvalidArgument("id_len".to_string()))?;
        }
        let len = buffer.as_ref().len();
        if len < 4 {
            return Err(Error::Overflow {
                cap: len,
                required: 4,
            });
        }
        let packet = Self { id_len, buffer };
        let calculate_size =
            packet.metric_len() as usize + packet.current_id_num() as usize * id_len as usize + 4;
        if calculate_size != len {
            return Err(Error::Overflow {
                cap: len,
                required: calculate_size,
            });
        }
        Ok(packet)
    }
    pub fn id_len(&self) -> u8 {
        self.id_len
    }
    pub fn current_id_num(&self) -> u8 {
        self.buffer.as_ref()[0]
    }
    pub fn all_id_num(&self) -> u16 {
        u16::from_be_bytes(self.buffer.as_ref()[2..4].try_into().unwrap())
    }
    pub fn metric_len(&self) -> u8 {
        let id_num = self.current_id_num();
        id_num / 2 + if id_num & 0b1 == 0b1 { 1 } else { 0 }
    }
    pub fn buffer(&self) -> &[u8] {
        self.buffer.as_ref()
    }
    pub fn iter(&self) -> IDRouteReplyIter<B> {
        IDRouteReplyIter {
            packet: self,
            index: 0,
        }
    }
}

pub struct IDRouteReplyIter<'a, B> {
    packet: &'a IDRouteReplyPacket<B>,
    index: usize,
}
impl<B: AsRef<[u8]>> Iterator for IDRouteReplyIter<'_, B> {
    type Item = (NodeID, u8);

    fn next(&mut self) -> Option<Self::Item> {
        let id_num = self.packet.current_id_num() as usize;
        if id_num == self.index {
            return None;
        }
        let metric_index = 4 + self.index / 2;
        let metric_offset = if self.index & 0b1 == 0b0 { 4 } else { 0 };
        let metric = (self.packet.buffer()[metric_index] >> metric_offset) & 0xF;
        let node_index_start =
            4 + self.packet.metric_len() as usize + self.index * self.packet.id_len() as usize;
        let node_index_end = node_index_start + self.packet.id_len() as usize;
        let node_id = NodeID::new(&self.packet.buffer()[node_index_start..node_index_end]).unwrap();
        self.index += 1;
        Some((node_id, metric))
    }
}

pub struct Builder;
impl Builder {
    pub fn calculate_len(list: &[(NodeID, u8)]) -> Result<usize> {
        if list.is_empty() {
            return Err(Error::InvalidArgument("".into()));
        }
        if list.len() > 255 {
            return Err(Error::InvalidArgument("".into()));
        }
        let id_len = list[0].0.len();
        if list.iter().any(|(node_id, _)| node_id.len() != id_len) {
            return Err(Error::InvalidArgument("".into()));
        }
        let id_num = list.len();
        let out_packet_head = 4 + id_len * 2;
        let metric_len = id_num / 2 + if id_num & 0b1 == 0b1 { 1 } else { 0 };

        let len = out_packet_head + 4 + metric_len + id_len * id_num;
        Ok(len)
    }
    pub fn build_reply(list: &[(NodeID, u8)], all_id_num: u16) -> Result<NetPacket<Vec<u8>>> {
        let len = Self::calculate_len(list)?;
        let id_len = list[0].0.len();
        let mut packet = NetPacket::unchecked(vec![0; len]);

        packet.set_protocol(ProtocolType::IDRouteReply);
        packet.set_id_length(id_len as _);
        packet.set_ttl(15);
        let payload = packet.payload_mut();
        let id_num = list.len();
        let metric_len = id_num / 2 + if id_num & 0b1 == 0b1 { 1 } else { 0 };
        payload[0] = id_num as _;
        payload[2..4].copy_from_slice(&all_id_num.to_be_bytes());
        for (index, (node_id, metric)) in list.iter().enumerate() {
            let metric_index = 4 + index / 2;
            let metric_offset = if index & 0b1 == 0b0 { 4 } else { 0 };
            payload[metric_index] |= (*metric) << metric_offset;
            let node_index_start = 4 + metric_len + index * id_len;
            let node_index_end = node_index_start + id_len;
            payload[node_index_start..node_index_end].copy_from_slice(node_id.as_ref());
        }

        Ok(packet)
    }
}

#[cfg(test)]
mod test {
    use crate::protocol::id_route::{Builder, IDRouteReplyPacket};
    use crate::protocol::node_id::NodeID;

    #[test]
    fn test_build() {
        let list = vec![
            (NodeID::from(1), 1),
            (NodeID::from(2), 1),
            (NodeID::from(1), 3),
        ];
        test_build0(list);
        let list = vec![
            (NodeID::from(1), 1),
            (NodeID::from(2), 1),
            (NodeID::from(1), 3),
            (NodeID::from(4), 2),
        ];
        test_build0(list);
    }
    fn test_build0(list: Vec<(NodeID, u8)>) {
        let packet = Builder::build_reply(&list, 20).unwrap();
        let packet = IDRouteReplyPacket::new(packet.payload(), packet.id_length()).unwrap();
        assert_eq!(packet.iter().count(), list.len());
        for (index, (node_id, metric)) in packet.iter().enumerate() {
            assert_eq!(node_id, list[index].0);
            assert_eq!(metric, list[index].1);
        }
    }
}
