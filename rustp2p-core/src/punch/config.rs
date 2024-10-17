use crate::nat::NatInfo;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::ops;
use std::str::FromStr;

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Serialize, Deserialize)]
pub enum PunchModel {
    IPv4Tcp,
    IPv4Udp,
    IPv6Tcp,
    IPv6Udp,
}

impl ops::BitOr<PunchModel> for PunchModel {
    type Output = PunchModelBox;

    fn bitor(self, rhs: PunchModel) -> Self::Output {
        let mut model = PunchModelBox::empty();
        model.or(self);
        model.or(rhs);
        model
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PunchModelBox {
    models: HashSet<PunchModel>,
}

impl Default for PunchModelBox {
    fn default() -> Self {
        PunchModelBox::all()
    }
}

impl ops::BitOr<PunchModel> for PunchModelBox {
    type Output = PunchModelBox;

    fn bitor(mut self, rhs: PunchModel) -> Self::Output {
        self.or(rhs);
        self
    }
}

impl PunchModelBox {
    pub fn all() -> Self {
        PunchModel::IPv4Tcp | PunchModel::IPv4Udp | PunchModel::IPv6Tcp | PunchModel::IPv6Udp
    }
    pub fn ipv4() -> Self {
        PunchModel::IPv4Tcp | PunchModel::IPv4Udp
    }
    pub fn ipv6() -> Self {
        PunchModel::IPv6Tcp | PunchModel::IPv6Udp
    }
    pub fn empty() -> Self {
        Self {
            models: Default::default(),
        }
    }
    pub fn or(&mut self, punch_model: PunchModel) {
        self.models.insert(punch_model);
    }
    pub fn is_match(&self, punch_model: PunchModel) -> bool {
        self.models.contains(&punch_model)
    }
}

#[derive(Clone, Debug)]
pub struct PunchModelBoxes {
    boxes: Vec<PunchModelBox>,
}

impl ops::BitAnd<PunchModelBox> for PunchModelBox {
    type Output = PunchModelBoxes;

    fn bitand(self, rhs: PunchModelBox) -> Self::Output {
        let mut boxes = PunchModelBoxes::empty();
        boxes.and(rhs);
        boxes
    }
}

impl PunchModelBoxes {
    pub fn all() -> Self {
        Self {
            boxes: vec![PunchModelBox::all()],
        }
    }
    pub fn empty() -> Self {
        Self { boxes: Vec::new() }
    }
    pub fn and(&mut self, punch_model_box: PunchModelBox) {
        self.boxes.push(punch_model_box)
    }
    pub fn is_match(&self, punch_model: PunchModel) -> bool {
        if self.boxes.is_empty() {
            return false;
        }
        for x in &self.boxes {
            if !x.is_match(punch_model) {
                return false;
            }
        }
        true
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PunchConsultInfo {
    pub peer_punch_model: PunchModelBox,
    pub peer_nat_info: NatInfo,
}

impl PunchConsultInfo {
    pub fn new(peer_punch_model: PunchModelBox, peer_nat_info: NatInfo) -> Self {
        Self {
            peer_punch_model,
            peer_nat_info,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PunchInfo {
    pub(crate) initiate_by_oneself: bool,
    pub(crate) punch_model: PunchModelBoxes,
    pub(crate) peer_nat_info: NatInfo,
}

impl PunchInfo {
    pub fn new(
        initiate_by_oneself: bool,
        punch_model: PunchModelBoxes,
        peer_nat_info: NatInfo,
    ) -> Self {
        Self {
            initiate_by_oneself,
            punch_model,
            peer_nat_info,
        }
    }
    pub fn new_by_oneself(punch_model: PunchModelBoxes, peer_nat_info: NatInfo) -> Self {
        Self {
            initiate_by_oneself: true,
            punch_model,
            peer_nat_info,
        }
    }
    pub fn new_by_other(punch_model: PunchModelBoxes, peer_nat_info: NatInfo) -> Self {
        Self {
            initiate_by_oneself: false,
            punch_model,
            peer_nat_info,
        }
    }
    pub(crate) fn use_ttl(&self) -> bool {
        self.initiate_by_oneself ^ (self.peer_nat_info.seq % 2 == 0)
    }
}

impl FromStr for PunchModel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().trim() {
            "ipv4-tcp" => Ok(PunchModel::IPv4Tcp),
            "ipv4-udp" => Ok(PunchModel::IPv4Udp),
            "ipv6-tcp" => Ok(PunchModel::IPv6Tcp),
            "ipv6-udp" => Ok(PunchModel::IPv6Udp),
            _ => Err(format!(
                "not match '{}', enum: ipv4-tcp/ipv4-udp/ipv6-tcp/ipv6-udp",
                s
            )),
        }
    }
}
