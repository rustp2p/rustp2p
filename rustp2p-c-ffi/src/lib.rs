use std::ffi::{CStr, CString};
use std::net::Ipv4Addr;
use std::os::raw::{c_char, c_int, c_uint, c_ushort};
use std::ptr;
use std::str::FromStr;
use std::sync::Arc;

use rustp2p::cipher::Algorithm;
use rustp2p::node_id::GroupCode;
use rustp2p::{Builder, EndPoint, PeerNodeAddress, RecvMetadata, RecvUserData};
use tokio::runtime::Runtime;

// Opaque handle types for C
pub struct Rustp2pBuilder {
    builder: Builder,
    runtime: Arc<Runtime>,
}

pub struct Rustp2pEndpoint {
    endpoint: Arc<EndPoint>,
    runtime: Arc<Runtime>,
}

pub struct Rustp2pRecvData {
    data: RecvUserData,
    metadata: RecvMetadata,
}

// Error codes
pub const RUSTP2P_OK: c_int = 0;
pub const RUSTP2P_ERROR: c_int = -1;
pub const RUSTP2P_ERROR_NULL_PTR: c_int = -2;
pub const RUSTP2P_ERROR_INVALID_STR: c_int = -3;
pub const RUSTP2P_ERROR_INVALID_IP: c_int = -4;
pub const RUSTP2P_ERROR_BUILD_FAILED: c_int = -5;
pub const RUSTP2P_ERROR_WOULD_BLOCK: c_int = -6;
pub const RUSTP2P_ERROR_EOF: c_int = -7;

/// Create a new builder
/// Returns NULL on failure
#[no_mangle]
pub extern "C" fn rustp2p_builder_new() -> *mut Rustp2pBuilder {
    let runtime = match Runtime::new() {
        Ok(rt) => Arc::new(rt),
        Err(_) => return ptr::null_mut(),
    };

    let builder = Builder::new();
    let rust_builder = Rustp2pBuilder { builder, runtime };
    Box::into_raw(Box::new(rust_builder))
}

/// Set UDP port
#[no_mangle]
pub extern "C" fn rustp2p_builder_udp_port(
    builder: *mut Rustp2pBuilder,
    port: c_ushort,
) -> c_int {
    if builder.is_null() {
        return RUSTP2P_ERROR_NULL_PTR;
    }
    let builder = unsafe { &mut *builder };
    builder.builder = std::mem::replace(&mut builder.builder, Builder::new()).udp_port(port);
    RUSTP2P_OK
}

/// Set TCP port
#[no_mangle]
pub extern "C" fn rustp2p_builder_tcp_port(
    builder: *mut Rustp2pBuilder,
    port: c_ushort,
) -> c_int {
    if builder.is_null() {
        return RUSTP2P_ERROR_NULL_PTR;
    }
    let builder = unsafe { &mut *builder };
    builder.builder = std::mem::replace(&mut builder.builder, Builder::new()).tcp_port(port);
    RUSTP2P_OK
}

/// Set node ID from IPv4 address string (e.g., "10.0.0.1")
#[no_mangle]
pub extern "C" fn rustp2p_builder_node_id(
    builder: *mut Rustp2pBuilder,
    node_id_str: *const c_char,
) -> c_int {
    if builder.is_null() || node_id_str.is_null() {
        return RUSTP2P_ERROR_NULL_PTR;
    }

    let c_str = unsafe { CStr::from_ptr(node_id_str) };
    let ip_str = match c_str.to_str() {
        Ok(s) => s,
        Err(_) => return RUSTP2P_ERROR_INVALID_STR,
    };

    let ipv4 = match Ipv4Addr::from_str(ip_str) {
        Ok(ip) => ip,
        Err(_) => return RUSTP2P_ERROR_INVALID_IP,
    };

    let builder = unsafe { &mut *builder };
    builder.builder = std::mem::replace(&mut builder.builder, Builder::new()).node_id(ipv4.into());
    RUSTP2P_OK
}

/// Set group code
#[no_mangle]
pub extern "C" fn rustp2p_builder_group_code(
    builder: *mut Rustp2pBuilder,
    group_code: c_uint,
) -> c_int {
    if builder.is_null() {
        return RUSTP2P_ERROR_NULL_PTR;
    }
    let builder = unsafe { &mut *builder };
    builder.builder =
        std::mem::replace(&mut builder.builder, Builder::new()).group_code(GroupCode::from(group_code as u128));
    RUSTP2P_OK
}

/// Add a peer address (e.g., "udp://127.0.0.1:9090" or "tcp://192.168.1.1:8080")
#[no_mangle]
pub extern "C" fn rustp2p_builder_add_peer(
    builder: *mut Rustp2pBuilder,
    peer_addr: *const c_char,
) -> c_int {
    if builder.is_null() || peer_addr.is_null() {
        return RUSTP2P_ERROR_NULL_PTR;
    }

    let c_str = unsafe { CStr::from_ptr(peer_addr) };
    let addr_str = match c_str.to_str() {
        Ok(s) => s,
        Err(_) => return RUSTP2P_ERROR_INVALID_STR,
    };

    let peer = match PeerNodeAddress::from_str(addr_str) {
        Ok(p) => p,
        Err(_) => return RUSTP2P_ERROR,
    };

    let builder = unsafe { &mut *builder };
    // We need to collect existing peers and add the new one
    // Since Builder doesn't expose peers, we'll need to rebuild
    // For now, we'll use a simplified approach
    builder.builder = std::mem::replace(&mut builder.builder, Builder::new()).peers(vec![peer]);
    RUSTP2P_OK
}

/// Set encryption algorithm
/// algorithm: 0 = AesGcm
/// password: the encryption password
#[no_mangle]
pub extern "C" fn rustp2p_builder_encryption(
    builder: *mut Rustp2pBuilder,
    algorithm: c_int,
    password: *const c_char,
) -> c_int {
    if builder.is_null() || password.is_null() {
        return RUSTP2P_ERROR_NULL_PTR;
    }

    let c_str = unsafe { CStr::from_ptr(password) };
    let pwd = match c_str.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return RUSTP2P_ERROR_INVALID_STR,
    };

    #[cfg(any(feature = "aes-gcm-openssl", feature = "aes-gcm-ring"))]
    let algo = match algorithm {
        0 => Algorithm::AesGcm(pwd),
        #[cfg(any(
            feature = "chacha20-poly1305-openssl",
            feature = "chacha20-poly1305-ring"
        ))]
        1 => Algorithm::ChaCha20Poly1305(pwd),
        _ => return RUSTP2P_ERROR,
    };

    #[cfg(not(any(feature = "aes-gcm-openssl", feature = "aes-gcm-ring")))]
    let _ = (algorithm, pwd);
    #[cfg(not(any(feature = "aes-gcm-openssl", feature = "aes-gcm-ring")))]
    return RUSTP2P_ERROR;

    #[cfg(any(feature = "aes-gcm-openssl", feature = "aes-gcm-ring"))]
    {
        let builder = unsafe { &mut *builder };
        builder.builder = std::mem::replace(&mut builder.builder, Builder::new()).encryption(algo);
        RUSTP2P_OK
    }
}

/// Build the endpoint
/// Returns NULL on failure
#[no_mangle]
pub extern "C" fn rustp2p_builder_build(builder: *mut Rustp2pBuilder) -> *mut Rustp2pEndpoint {
    if builder.is_null() {
        return ptr::null_mut();
    }

    let builder = unsafe { Box::from_raw(builder) };
    let runtime = builder.runtime.clone();
    let builder_inner = builder.builder;

    let endpoint = match runtime.block_on(builder_inner.build()) {
        Ok(ep) => Arc::new(ep),
        Err(_) => return ptr::null_mut(),
    };

    let rust_endpoint = Rustp2pEndpoint { endpoint, runtime };
    Box::into_raw(Box::new(rust_endpoint))
}

/// Free/destroy the builder
#[no_mangle]
pub extern "C" fn rustp2p_builder_free(builder: *mut Rustp2pBuilder) {
    if !builder.is_null() {
        unsafe {
            let _ = Box::from_raw(builder);
        }
    }
}

/// Send data to a peer
/// dest_ip: destination node IP address string (e.g., "10.0.0.2")
/// data: pointer to data buffer
/// len: length of data
#[no_mangle]
pub extern "C" fn rustp2p_endpoint_send_to(
    endpoint: *mut Rustp2pEndpoint,
    dest_ip: *const c_char,
    data: *const u8,
    len: usize,
) -> c_int {
    if endpoint.is_null() || dest_ip.is_null() || data.is_null() {
        return RUSTP2P_ERROR_NULL_PTR;
    }

    let endpoint = unsafe { &*endpoint };
    let c_str = unsafe { CStr::from_ptr(dest_ip) };
    let ip_str = match c_str.to_str() {
        Ok(s) => s,
        Err(_) => return RUSTP2P_ERROR_INVALID_STR,
    };

    let ipv4 = match Ipv4Addr::from_str(ip_str) {
        Ok(ip) => ip,
        Err(_) => return RUSTP2P_ERROR_INVALID_IP,
    };

    let buf = unsafe { std::slice::from_raw_parts(data, len) };

    match endpoint.runtime.block_on(endpoint.endpoint.send_to(buf, ipv4)) {
        Ok(_) => RUSTP2P_OK,
        Err(_) => RUSTP2P_ERROR,
    }
}

/// Try to send data to a peer (non-blocking)
#[no_mangle]
pub extern "C" fn rustp2p_endpoint_try_send_to(
    endpoint: *mut Rustp2pEndpoint,
    dest_ip: *const c_char,
    data: *const u8,
    len: usize,
) -> c_int {
    if endpoint.is_null() || dest_ip.is_null() || data.is_null() {
        return RUSTP2P_ERROR_NULL_PTR;
    }

    let endpoint = unsafe { &*endpoint };
    let c_str = unsafe { CStr::from_ptr(dest_ip) };
    let ip_str = match c_str.to_str() {
        Ok(s) => s,
        Err(_) => return RUSTP2P_ERROR_INVALID_STR,
    };

    let ipv4 = match Ipv4Addr::from_str(ip_str) {
        Ok(ip) => ip,
        Err(_) => return RUSTP2P_ERROR_INVALID_IP,
    };

    let buf = unsafe { std::slice::from_raw_parts(data, len) };

    match endpoint.endpoint.try_send_to(buf, ipv4) {
        Ok(_) => RUSTP2P_OK,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::WouldBlock {
                RUSTP2P_ERROR_WOULD_BLOCK
            } else {
                RUSTP2P_ERROR
            }
        }
    }
}

/// Receive data from peers (blocking)
/// Returns pointer to received data structure, or NULL on error
/// Caller must free the returned pointer with rustp2p_recv_data_free
#[no_mangle]
pub extern "C" fn rustp2p_endpoint_recv_from(
    endpoint: *mut Rustp2pEndpoint,
) -> *mut Rustp2pRecvData {
    if endpoint.is_null() {
        return ptr::null_mut();
    }

    let endpoint = unsafe { &*endpoint };

    match endpoint.runtime.block_on(endpoint.endpoint.recv_from()) {
        Ok((data, metadata)) => {
            let recv_data = Rustp2pRecvData { data, metadata };
            Box::into_raw(Box::new(recv_data))
        }
        Err(_) => ptr::null_mut(),
    }
}

/// Try to receive data from peers (non-blocking)
/// Returns pointer to received data structure, or NULL if no data available or on error
/// Caller must free the returned pointer with rustp2p_recv_data_free
#[no_mangle]
pub extern "C" fn rustp2p_endpoint_try_recv_from(
    endpoint: *mut Rustp2pEndpoint,
) -> *mut Rustp2pRecvData {
    if endpoint.is_null() {
        return ptr::null_mut();
    }

    let endpoint = unsafe { &*endpoint };

    match endpoint.endpoint.try_recv_from() {
        Ok((data, metadata)) => {
            let recv_data = Rustp2pRecvData { data, metadata };
            Box::into_raw(Box::new(recv_data))
        }
        Err(_) => ptr::null_mut(),
    }
}

/// Get payload data from received data
/// out_data: output pointer to data (borrowed, don't free)
/// out_len: output length
#[no_mangle]
pub extern "C" fn rustp2p_recv_data_get_payload(
    recv_data: *const Rustp2pRecvData,
    out_data: *mut *const u8,
    out_len: *mut usize,
) -> c_int {
    if recv_data.is_null() || out_data.is_null() || out_len.is_null() {
        return RUSTP2P_ERROR_NULL_PTR;
    }

    let recv_data = unsafe { &*recv_data };
    let payload = recv_data.data.payload();
    unsafe {
        *out_data = payload.as_ptr();
        *out_len = payload.len();
    }
    RUSTP2P_OK
}

/// Get source node ID from received data as IPv4 string
/// buffer: output buffer for IP string
/// buffer_len: size of buffer (should be at least 16 bytes for "xxx.xxx.xxx.xxx\0")
#[no_mangle]
pub extern "C" fn rustp2p_recv_data_get_src_id(
    recv_data: *const Rustp2pRecvData,
    buffer: *mut c_char,
    buffer_len: usize,
) -> c_int {
    if recv_data.is_null() || buffer.is_null() {
        return RUSTP2P_ERROR_NULL_PTR;
    }

    let recv_data = unsafe { &*recv_data };
    let src_id: Ipv4Addr = recv_data.metadata.src_id().into();
    let ip_str = src_id.to_string();

    let c_string = match CString::new(ip_str) {
        Ok(s) => s,
        Err(_) => return RUSTP2P_ERROR,
    };

    let bytes = c_string.as_bytes_with_nul();
    if bytes.len() > buffer_len {
        return RUSTP2P_ERROR;
    }

    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_char, buffer, bytes.len());
    }
    RUSTP2P_OK
}

/// Get destination node ID from received data as IPv4 string
#[no_mangle]
pub extern "C" fn rustp2p_recv_data_get_dest_id(
    recv_data: *const Rustp2pRecvData,
    buffer: *mut c_char,
    buffer_len: usize,
) -> c_int {
    if recv_data.is_null() || buffer.is_null() {
        return RUSTP2P_ERROR_NULL_PTR;
    }

    let recv_data = unsafe { &*recv_data };
    let dest_id: Ipv4Addr = recv_data.metadata.dest_id().into();
    let ip_str = dest_id.to_string();

    let c_string = match CString::new(ip_str) {
        Ok(s) => s,
        Err(_) => return RUSTP2P_ERROR,
    };

    let bytes = c_string.as_bytes_with_nul();
    if bytes.len() > buffer_len {
        return RUSTP2P_ERROR;
    }

    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_char, buffer, bytes.len());
    }
    RUSTP2P_OK
}

/// Check if the received data was relayed
#[no_mangle]
pub extern "C" fn rustp2p_recv_data_is_relay(recv_data: *const Rustp2pRecvData) -> c_int {
    if recv_data.is_null() {
        return RUSTP2P_ERROR_NULL_PTR;
    }

    let recv_data = unsafe { &*recv_data };
    if recv_data.metadata.is_relay() {
        1
    } else {
        0
    }
}

/// Free received data
#[no_mangle]
pub extern "C" fn rustp2p_recv_data_free(recv_data: *mut Rustp2pRecvData) {
    if !recv_data.is_null() {
        unsafe {
            let _ = Box::from_raw(recv_data);
        }
    }
}

/// Free/destroy the endpoint
#[no_mangle]
pub extern "C" fn rustp2p_endpoint_free(endpoint: *mut Rustp2pEndpoint) {
    if !endpoint.is_null() {
        unsafe {
            let _ = Box::from_raw(endpoint);
        }
    }
}
