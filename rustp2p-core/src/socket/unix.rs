use crate::socket::{LocalInterface, VntSocketTrait};

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
