use crate::nat::NatInfo;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::ops;
use std::str::FromStr;

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Serialize, Deserialize)]
pub enum PunchPolicy {
    IPv4Tcp,
    IPv4Udp,
    IPv6Tcp,
    IPv6Udp,
}

impl ops::BitOr<PunchPolicy> for PunchPolicy {
    type Output = PunchPolicySet;

    fn bitor(self, rhs: PunchPolicy) -> Self::Output {
        let mut model = PunchPolicySet::empty();
        model.or(self);
        model.or(rhs);
        model
    }
}
/// This is middle representation for inner
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PunchPolicySet {
    models: HashSet<PunchPolicy>,
}

impl Default for PunchPolicySet {
    fn default() -> Self {
        PunchPolicySet::all()
    }
}

impl ops::BitOr<PunchPolicy> for PunchPolicySet {
    type Output = PunchPolicySet;

    fn bitor(mut self, rhs: PunchPolicy) -> Self::Output {
        self.or(rhs);
        self
    }
}

impl PunchPolicySet {
    pub fn all() -> Self {
        PunchPolicy::IPv4Tcp | PunchPolicy::IPv4Udp | PunchPolicy::IPv6Tcp | PunchPolicy::IPv6Udp
    }
    pub fn ipv4() -> Self {
        PunchPolicy::IPv4Tcp | PunchPolicy::IPv4Udp
    }
    pub fn ipv6() -> Self {
        PunchPolicy::IPv6Tcp | PunchPolicy::IPv6Udp
    }
    pub fn empty() -> Self {
        Self {
            models: Default::default(),
        }
    }
    pub fn or(&mut self, punch_model: PunchPolicy) {
        self.models.insert(punch_model);
    }
    pub fn is_match(&self, punch_model: PunchPolicy) -> bool {
        self.models.contains(&punch_model)
    }
}

#[derive(Clone, Debug)]
pub struct PunchModel {
    set: Vec<PunchPolicySet>,
}

impl ops::BitAnd<PunchPolicySet> for PunchPolicySet {
    type Output = PunchModel;

    fn bitand(self, rhs: PunchPolicySet) -> Self::Output {
        let mut boxes = PunchModel::empty();
        boxes.and(self);
        boxes.and(rhs);
        boxes
    }
}

impl PunchModel {
    pub fn all() -> Self {
        Self {
            set: vec![PunchPolicySet::all()],
        }
    }
    pub fn empty() -> Self {
        Self { set: Vec::new() }
    }
    pub fn and(&mut self, punch_model_box: PunchPolicySet) {
        self.set.push(punch_model_box)
    }
    pub fn is_match(&self, punch_model: PunchPolicy) -> bool {
        if self.set.is_empty() {
            return false;
        }
        for x in &self.set {
            if !x.is_match(punch_model) {
                return false;
            }
        }
        true
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PunchConsultInfo {
    pub peer_punch_model: PunchPolicySet,
    pub peer_nat_info: NatInfo,
}

impl PunchConsultInfo {
    pub fn new(peer_punch_model: PunchPolicySet, peer_nat_info: NatInfo) -> Self {
        Self {
            peer_punch_model,
            peer_nat_info,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PunchInfo {
    pub(crate) punch_model: PunchModel,
    pub(crate) peer_nat_info: NatInfo,
}

impl PunchInfo {
    pub fn new(punch_model: PunchModel, peer_nat_info: NatInfo) -> Self {
        Self {
            punch_model,
            peer_nat_info,
        }
    }
}

impl FromStr for PunchPolicy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().trim() {
            "ipv4-tcp" => Ok(PunchPolicy::IPv4Tcp),
            "ipv4-udp" => Ok(PunchPolicy::IPv4Udp),
            "ipv6-tcp" => Ok(PunchPolicy::IPv6Tcp),
            "ipv6-udp" => Ok(PunchPolicy::IPv6Udp),
            _ => Err(format!(
                "not match '{}', enum: ipv4-tcp/ipv4-udp/ipv6-tcp/ipv6-udp",
                s
            )),
        }
    }
}
