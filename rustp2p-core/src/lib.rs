/// Primary API for P2P networking.
pub mod endpoint;

/// Transport layer (deprecated, use `endpoint` module instead).
#[deprecated(note = "Use `endpoint` module instead")]
pub mod transport;

pub mod idle;
pub mod nat;
pub mod punch;
pub mod route_table;
pub mod socket;
pub mod stun;
pub mod util;
