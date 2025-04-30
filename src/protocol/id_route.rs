/*
  Query all reachable IDs of the other party

   0                                            15                                              31
   0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                  offset(16)                 |               query id(16)                    |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  protocol = ProtocolType::IDRouteQuery
  dest ID = 0
  ttl = 1
*/

/*
   0                                            15                                              31
   0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5  6  7  8  9  0  1
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |                                     group code(128)                                         |
  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
  |   current id num(8) |                            query id(16)      |            all id num(16)                     |
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
use crate::protocol::node_id::{GroupCode, NodeID, ID_LEN};
use crate::protocol::protocol_type::ProtocolType;
use crate::protocol::{NetPacket, HEAD_LEN};
use std::io;

pub struct IDRouteReplyPacket<B> {
    buffer: B,
}
impl<B: AsRef<[u8]>> IDRouteReplyPacket<B> {
    #[allow(dead_code)]
    pub(crate) fn unchecked(buffer: B) -> IDRouteReplyPacket<B> {
        Self { buffer }
    }
    pub(crate) fn new(buffer: B) -> io::Result<IDRouteReplyPacket<B>> {
        let len = buffer.as_ref().len();
        if len < 16 + 5 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "buf len error"));
        }
        let packet = Self { buffer };
        let calculate_size =
            packet.metric_len() as usize + packet.current_id_num() as usize * ID_LEN + 16 + 5;
        if calculate_size != len {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "buf len error"));
        }
        Ok(packet)
    }
    pub(crate) fn group_code(&self) -> &[u8] {
        &self.buffer.as_ref()[..16]
    }
    pub(crate) fn current_id_num(&self) -> u8 {
        self.buffer.as_ref()[16]
    }
    pub(crate) fn query_id(&self) -> u16 {
        u16::from_be_bytes(self.buffer.as_ref()[17..19].try_into().unwrap())
    }
    #[allow(dead_code)]
    pub(crate) fn all_id_num(&self) -> u16 {
        u16::from_be_bytes(self.buffer.as_ref()[19..21].try_into().unwrap())
    }
    pub(crate) fn metric_len(&self) -> u8 {
        let id_num = self.current_id_num();
        id_num / 2 + if id_num & 0b1 == 0b1 { 1 } else { 0 }
    }
    pub(crate) fn buffer(&self) -> &[u8] {
        self.buffer.as_ref()
    }
    pub(crate) fn iter(&self) -> IDRouteReplyIter<B> {
        IDRouteReplyIter {
            packet: self,
            index: 0,
        }
    }
}

pub(crate) struct IDRouteReplyIter<'a, B> {
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
        let metric_index = 16 + 5 + self.index / 2;
        let metric_offset = if self.index & 0b1 == 0b0 { 4 } else { 0 };
        let metric = (self.packet.buffer()[metric_index] >> metric_offset) & 0xF;
        let node_index_start = 16 + 5 + self.packet.metric_len() as usize + self.index * ID_LEN;
        let node_index_end = node_index_start + ID_LEN;
        let node_id =
            NodeID::try_from(&self.packet.buffer()[node_index_start..node_index_end]).unwrap();
        self.index += 1;
        Some((node_id, metric))
    }
}

pub(crate) struct Builder;
impl Builder {
    pub(crate) fn calculate_len(list: &[(NodeID, u8)]) -> io::Result<usize> {
        if list.len() > 255 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "list len error"));
        }

        let id_num = list.len();
        let metric_len = id_num / 2 + if id_num & 0b1 == 0b1 { 1 } else { 0 };

        let len = HEAD_LEN + 16 + 5 + metric_len + ID_LEN * id_num;
        Ok(len)
    }
    pub(crate) fn build_reply(
        group_code: &GroupCode,
        list: &[(NodeID, u8)],
        query_id: u16,
        all_id_num: u16,
    ) -> io::Result<NetPacket<Vec<u8>>> {
        let len = Self::calculate_len(list)?;
        let mut packet = unsafe { NetPacket::new_unchecked(vec![0; len]) };

        packet.set_protocol(ProtocolType::IDRouteReply);
        packet.set_ttl(15);
        packet.reset_data_len();
        let payload = packet.payload_mut();
        let id_num = list.len();
        let metric_len = id_num / 2 + if id_num & 0b1 == 0b1 { 1 } else { 0 };
        payload[..16].copy_from_slice(group_code.as_ref());
        payload[16] = id_num as _;
        payload[17..19].copy_from_slice(&query_id.to_be_bytes());
        payload[19..21].copy_from_slice(&all_id_num.to_be_bytes());
        for (index, (node_id, metric)) in list.iter().enumerate() {
            let metric_index = 16 + 5 + index / 2;
            let metric_offset = if index & 0b1 == 0b0 { 4 } else { 0 };
            payload[metric_index] |= (*metric) << metric_offset;
            let node_index_start = 16 + 5 + metric_len + index * ID_LEN;
            let node_index_end = node_index_start + ID_LEN;
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
        let packet = Builder::build_reply(&1u128.into(), &list, 16, 20).unwrap();
        let packet = IDRouteReplyPacket::new(packet.payload()).unwrap();
        assert_eq!(packet.iter().count(), list.len());
        assert_eq!(packet.query_id(), 16);
        assert_eq!(packet.all_id_num(), 20);
        for (index, (node_id, metric)) in packet.iter().enumerate() {
            assert_eq!(node_id, list[index].0);
            assert_eq!(metric, list[index].1);
        }
    }
}
