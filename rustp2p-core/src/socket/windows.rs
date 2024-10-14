use std::io;
use std::net::Ipv4Addr;
use std::os::windows::io::AsRawSocket;

use windows_sys::core::PCSTR;
use windows_sys::Win32::NetworkManagement::IpHelper::GetBestInterfaceEx;
use windows_sys::Win32::Networking::WinSock::{
    htonl, setsockopt, WSAIoctl, AF_INET, IPPROTO_IP, IP_UNICAST_IF, SIO_UDP_CONNRESET, SOCKADDR,
    SOCKADDR_IN, SOCKET_ERROR,
};

use crate::socket::{LocalInterface, VntSocketTrait};

impl VntSocketTrait for socket2::Socket {
    fn set_ip_unicast_if(&self, interface: &LocalInterface) -> crate::error::Result<()> {
        log::debug!("set_ip_unicast_if ={interface:?}");
        let index = interface.index;
        let raw_socket = self.as_raw_socket();
        let result = unsafe {
            let best_interface = htonl(index);
            setsockopt(
                raw_socket as usize,
                IPPROTO_IP,
                IP_UNICAST_IF,
                &best_interface as *const _ as PCSTR,
                std::mem::size_of_val(&best_interface) as i32,
            )
        };
        if result == SOCKET_ERROR {
            Err(io::Error::last_os_error())?;
        }
        Ok(())
    }
}

/// Obtain the optimal interface with the specified IP address
pub fn get_best_interface(dest_ip: Ipv4Addr) -> anyhow::Result<LocalInterface> {
    // 获取最佳接口
    let index = unsafe {
        let mut dest: SOCKADDR_IN = std::mem::zeroed();
        dest.sin_family = AF_INET;
        dest.sin_addr.S_un.S_addr = u32::from_ne_bytes(dest_ip.octets());

        let mut index: u32 = 0;
        if GetBestInterfaceEx(&dest as *const _ as *mut SOCKADDR, &mut index) != 0 {
            Err(anyhow::anyhow!(
                "Failed to GetBestInterfaceEx: {:?}",
                std::io::Error::last_os_error()
            ))?;
        }
        index
    };
    Ok(LocalInterface { index })
}
pub(crate) fn ignore_conn_reset(socket: &socket2::Socket) -> io::Result<()> {
    let socket_raw = socket.as_raw_socket() as usize;
    let mut bytes_returned: u32 = 0;
    let mut flag: u32 = 0;

    // SIO_UDP_CONNRESET 参数设置为 FALSE (0) 来忽略错误
    let result = unsafe {
        WSAIoctl(
            socket_raw,
            SIO_UDP_CONNRESET,
            &mut flag as *mut _ as *mut _,
            std::mem::size_of_val(&flag) as u32,
            std::ptr::null_mut(),
            0,
            &mut bytes_returned as *mut _,
            std::ptr::null_mut(),
            None,
        )
    };

    if result == SOCKET_ERROR {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
