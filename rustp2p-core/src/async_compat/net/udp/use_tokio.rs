use std::io;
use tokio::io::Interest;
pub use tokio::net::UdpSocket;

pub async fn read_with<R>(udp: &UdpSocket, op: impl FnMut() -> io::Result<R>) -> io::Result<R> {
    udp.async_io(Interest::READABLE, op).await
}

pub async fn write_with<R>(udp: &UdpSocket, op: impl FnMut() -> io::Result<R>) -> io::Result<R> {
    udp.async_io(Interest::WRITABLE, op).await
}
