use crate::socket::{get_interface, LocalInterface, VntSocketTrait};
use std::net::Ipv4Addr;

#[cfg(target_os = "freebsd")]
impl VntSocketTrait for socket2::Socket {
    fn set_ip_unicast_if(&self, _interface: &LocalInterface) -> crate::error::Result<()> {
        Ok(())
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
impl VntSocketTrait for socket2::Socket {
    fn set_ip_unicast_if(&self, interface: &LocalInterface) -> crate::error::Result<()> {
        self.bind_device(Some(interface.name.as_bytes()))?;
        Ok(())
    }
}

#[cfg(any(target_os = "macos", target_os = "ios",))]
impl VntSocketTrait for socket2::Socket {
    fn set_ip_unicast_if(&self, interface: &LocalInterface) -> crate::error::Result<()> {
        self.bind_device_by_index_v4(std::num::NonZeroU32::new(interface.index))?;
        Ok(())
    }
}

/// Obtain the optimal interface with the specified IP address
pub fn get_best_interface(dest_ip: Ipv4Addr) -> anyhow::Result<LocalInterface> {
    match get_interface(dest_ip) {
        Ok(iface) => return Ok(iface),
        Err(e) => {
            log::warn!("not find interface e={:?},ip={}", e, dest_ip);
        }
    }
    // 应该再查路由表找到默认路由的
    Ok(LocalInterface::default())
}
