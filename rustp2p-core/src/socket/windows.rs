use std::io;
use std::os::windows::io::AsRawSocket;

use windows_sys::core::PCSTR;
use windows_sys::Win32::Networking::WinSock::{
    htonl, setsockopt, WSAIoctl, IPPROTO_IP, IP_UNICAST_IF, SIO_UDP_CONNRESET, SOCKET_ERROR,
};

use crate::socket::{LocalInterface, VntSocketTrait};

impl VntSocketTrait for socket2::Socket {
    fn set_ip_unicast_if(&self, interface: &LocalInterface) -> io::Result<()> {
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
