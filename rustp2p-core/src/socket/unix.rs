use crate::socket::{LocalInterface, SocketTrait};

#[cfg(target_os = "freebsd")]
impl SocketTrait for socket2::Socket {
    fn set_ip_unicast_if(&self, _interface: &LocalInterface) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
impl SocketTrait for socket2::Socket {
    fn set_ip_unicast_if(&self, interface: &LocalInterface) -> std::io::Result<()> {
        self.bind_device(Some(interface.name.as_bytes()))?;
        Ok(())
    }
}

#[cfg(any(target_os = "macos", target_os = "ios",))]
impl SocketTrait for socket2::Socket {
    fn set_ip_unicast_if(&self, interface: &LocalInterface) -> std::io::Result<()> {
        self.bind_device_by_index_v4(std::num::NonZeroU32::new(interface.index))?;
        Ok(())
    }
}
