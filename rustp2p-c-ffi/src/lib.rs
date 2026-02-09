//! # rustp2p-c-ffi - C FFI Bindings for rustp2p
//!
//! This crate provides C-compatible Foreign Function Interface (FFI) bindings for the `rustp2p`
//! library, enabling integration with C, C++, and other languages that support C FFI.
//!
//! ## Features
//!
//! - **C-Compatible API**: Simple, pointer-based API for C/C++ integration
//! - **Builder Pattern**: Fluent API for configuration
//! - **Error Handling**: Clear error codes for all operations
//! - **Memory Safety**: Proper ownership and cleanup functions
//! - **Cross-Platform**: Works on Windows, Linux, and macOS
//!
//! ## Building
//!
//! This crate produces both a static library and a dynamic library:
//!
//! ```bash
//! cargo build --release
//! ```
//!
//! Output files:
//! - `librustp2p_c_ffi.a` / `rustp2p_c_ffi.lib` (static)
//! - `librustp2p_c_ffi.so` / `rustp2p_c_ffi.dll` / `librustp2p_c_ffi.dylib` (dynamic)
//!
//! ## C Usage Example
//!
//! ### Basic Setup
//!
//! ```c
//! #include <stdio.h>
//! #include <stdlib.h>
//! #include <string.h>
//!
//! // Create and configure a node
//! Rustp2pBuilder* builder = rustp2p_builder_new();
//! rustp2p_builder_node_id(builder, "10.0.0.1");
//! rustp2p_builder_udp_port(builder, 8080);
//! rustp2p_builder_group_code(builder, "12345");
//!
//! // Build the endpoint
//! Rustp2pEndpoint* endpoint = rustp2p_builder_build(builder);
//! if (!endpoint) {
//!     fprintf(stderr, "Failed to build endpoint\n");
//!     return -1;
//! }
//! ```
//!
//! ### Sending Data
//!
//! ```c
//! const char* message = "Hello, peer!";
//! int result = rustp2p_endpoint_send_to(
//!     endpoint,
//!     "10.0.0.2",
//!     (const uint8_t*)message,
//!     strlen(message)
//! );
//!
//! if (result == RUSTP2P_OK) {
//!     printf("Message sent successfully\n");
//! }
//! ```
//!
//! ### Receiving Data
//!
//! ```c
//! // Blocking receive
//! Rustp2pRecvData* recv_data = rustp2p_endpoint_recv_from(endpoint);
//! if (recv_data) {
//!     const uint8_t* data;
//!     size_t len;
//!     rustp2p_recv_data_get_payload(recv_data, &data, &len);
//!     
//!     char src_id[16];
//!     rustp2p_recv_data_get_src_id(recv_data, src_id, sizeof(src_id));
//!     
//!     printf("Received %zu bytes from %s\n", len, src_id);
//!     
//!     rustp2p_recv_data_free(recv_data);
//! }
//! ```
//!
//! ### Cleanup
//!
//! ```c
//! rustp2p_endpoint_free(endpoint);
//! ```
//!
//! ## Error Codes
//!
//! The library uses the following error codes:
//!
//! - `RUSTP2P_OK` (0): Success
//! - `RUSTP2P_ERROR` (-1): General error
//! - `RUSTP2P_ERROR_NULL_PTR` (-2): NULL pointer provided
//! - `RUSTP2P_ERROR_INVALID_STR` (-3): Invalid string
//! - `RUSTP2P_ERROR_INVALID_IP` (-4): Invalid IP address
//! - `RUSTP2P_ERROR_BUILD_FAILED` (-5): Failed to build endpoint
//! - `RUSTP2P_ERROR_WOULD_BLOCK` (-6): Operation would block
//! - `RUSTP2P_ERROR_EOF` (-7): End of file / connection closed
//!
//! ## Complete C Example
//!
//! ```c
//! #include <stdio.h>
//! #include <stdlib.h>
//! #include <string.h>
//! #include <pthread.h>
//!
//! void* receive_thread(void* arg) {
//!     Rustp2pEndpoint* endpoint = (Rustp2pEndpoint*)arg;
//!     
//!     while (1) {
//!         Rustp2pRecvData* recv_data = rustp2p_endpoint_recv_from(endpoint);
//!         if (!recv_data) {
//!             break;
//!         }
//!         
//!         const uint8_t* data;
//!         size_t len;
//!         rustp2p_recv_data_get_payload(recv_data, &data, &len);
//!         
//!         char src_id[16];
//!         rustp2p_recv_data_get_src_id(recv_data, src_id, sizeof(src_id));
//!         
//!         printf("Received %zu bytes from %s: %.*s\n", len, src_id, (int)len, data);
//!         
//!         rustp2p_recv_data_free(recv_data);
//!     }
//!     
//!     return NULL;
//! }
//!
//! int main() {
//!     // Create builder
//!     Rustp2pBuilder* builder = rustp2p_builder_new();
//!     if (!builder) {
//!         return -1;
//!     }
//!     
//!     // Configure
//!     rustp2p_builder_node_id(builder, "10.0.0.1");
//!     rustp2p_builder_udp_port(builder, 8080);
//!     rustp2p_builder_tcp_port(builder, 8080);
//!     rustp2p_builder_group_code(builder, "12345");
//!     rustp2p_builder_add_peer(builder, "udp://192.168.1.100:9090");
//!     
//!     // Build endpoint
//!     Rustp2pEndpoint* endpoint = rustp2p_builder_build(builder);
//!     if (!endpoint) {
//!         fprintf(stderr, "Failed to build endpoint\n");
//!         return -1;
//!     }
//!     
//!     // Start receive thread
//!     pthread_t thread;
//!     pthread_create(&thread, NULL, receive_thread, endpoint);
//!     
//!     // Send messages
//!     const char* message = "Hello from C!";
//!     rustp2p_endpoint_send_to(endpoint, "10.0.0.2",
//!                              (const uint8_t*)message, strlen(message));
//!     
//!     // Wait for thread
//!     pthread_join(thread, NULL);
//!     
//!     // Cleanup
//!     rustp2p_endpoint_free(endpoint);
//!     
//!     return 0;
//! }
//! ```
//!
//! ## C++ Integration
//!
//! For C++ projects, wrap the C API in RAII classes:
//!
//! ```cpp
//! class Rustp2pEndpointWrapper {
//!     Rustp2pEndpoint* endpoint_;
//! public:
//!     Rustp2pEndpointWrapper(Rustp2pEndpoint* ep) : endpoint_(ep) {}
//!     ~Rustp2pEndpointWrapper() {
//!         if (endpoint_) {
//!             rustp2p_endpoint_free(endpoint_);
//!         }
//!     }
//!     
//!     // Prevent copying
//!     Rustp2pEndpointWrapper(const Rustp2pEndpointWrapper&) = delete;
//!     Rustp2pEndpointWrapper& operator=(const Rustp2pEndpointWrapper&) = delete;
//!     
//!     Rustp2pEndpoint* get() { return endpoint_; }
//! };
//! ```
//!
//! ## JavaScript/TypeScript Integration
//!
//! See [JAVASCRIPT.md](https://github.com/rustp2p/rustp2p/blob/master/rustp2p-c-ffi/JAVASCRIPT.md)
//! for details on using these bindings with Node.js via `node-ffi-napi`.
//!
//! ## Thread Safety
//!
//! - All functions are thread-safe
//! - Multiple threads can call send/receive operations simultaneously
//! - The Tokio runtime is managed internally
//!
//! ## Memory Management
//!
//! - Always call the corresponding `_free` function for allocated resources
//! - Do not use pointers after calling `_free`
//! - Received data pointers are valid only until `rustp2p_recv_data_free` is called
//!
//! ## See Also
//!
//! - [`rustp2p`](../rustp2p/index.html) - The main Rust library
//! - [GitHub Repository](https://github.com/rustp2p/rustp2p)

use std::ffi::{CStr, CString};
use std::net::Ipv4Addr;
use std::os::raw::{c_char, c_int, c_ushort};
use std::ptr;
use std::str::FromStr;
use std::sync::Arc;

use rustp2p::cipher::Algorithm;
use rustp2p::node_id::GroupCode;
use rustp2p::{Builder, EndPoint, PeerNodeAddress, RecvMetadata, RecvUserData};
use tokio::runtime::Runtime;

// Opaque handle types for C
/// Builder handle for configuring a rustp2p endpoint.
///
/// Use the `rustp2p_builder_*` functions to configure, then call
/// `rustp2p_builder_build` to create an endpoint.
pub struct Rustp2pBuilder {
    builder: Builder,
    runtime: Arc<Runtime>,
    peers: Vec<PeerNodeAddress>,
}

/// Endpoint handle for sending and receiving P2P data.
///
/// Create with `rustp2p_builder_build`, free with `rustp2p_endpoint_free`.
pub struct Rustp2pEndpoint {
    endpoint: Arc<EndPoint>,
    runtime: Arc<Runtime>,
}

/// Received data handle containing both payload and metadata.
///
/// Use `rustp2p_recv_data_get_*` functions to extract information,
/// then call `rustp2p_recv_data_free` to release.
pub struct Rustp2pRecvData {
    data: RecvUserData,
    metadata: RecvMetadata,
}

// Error codes
/// Operation completed successfully.
pub const RUSTP2P_OK: c_int = 0;
/// General error occurred.
pub const RUSTP2P_ERROR: c_int = -1;
/// NULL pointer was provided where a valid pointer was expected.
pub const RUSTP2P_ERROR_NULL_PTR: c_int = -2;
/// Invalid string format or encoding.
pub const RUSTP2P_ERROR_INVALID_STR: c_int = -3;
/// Invalid IP address format.
pub const RUSTP2P_ERROR_INVALID_IP: c_int = -4;
/// Failed to build endpoint (e.g., port in use, invalid config).
pub const RUSTP2P_ERROR_BUILD_FAILED: c_int = -5;
/// Operation would block (for non-blocking operations).
pub const RUSTP2P_ERROR_WOULD_BLOCK: c_int = -6;
/// End of file / connection closed.
pub const RUSTP2P_ERROR_EOF: c_int = -7;

/// Create a new builder
///
/// Returns a new builder handle that must be freed with `rustp2p_builder_free`
/// or consumed by `rustp2p_builder_build`.
///
/// # Returns
///
/// Returns a valid builder pointer on success, or NULL on failure (e.g., out of memory).
///
/// # Example
///
/// ```c
/// Rustp2pBuilder* builder = rustp2p_builder_new();
/// if (!builder) {
///     fprintf(stderr, "Failed to create builder\n");
///     return -1;
/// }
/// ```
#[no_mangle]
pub extern "C" fn rustp2p_builder_new() -> *mut Rustp2pBuilder {
    let runtime = match Runtime::new() {
        Ok(rt) => Arc::new(rt),
        Err(_) => return ptr::null_mut(),
    };

    let builder = Builder::new();
    let rust_builder = Rustp2pBuilder {
        builder,
        runtime,
        peers: Vec::new(),
    };
    Box::into_raw(Box::new(rust_builder))
}

/// Set UDP port
///
/// Configures the UDP port for this endpoint. Use 0 for a random port.
///
/// # Arguments
///
/// * `builder` - The builder handle
/// * `port` - The UDP port number (0-65535)
///
/// # Returns
///
/// * `RUSTP2P_OK` on success
/// * `RUSTP2P_ERROR_NULL_PTR` if builder is NULL
///
/// # Example
///
/// ```c
/// rustp2p_builder_udp_port(builder, 8080);
/// ```
#[no_mangle]
pub extern "C" fn rustp2p_builder_udp_port(builder: *mut Rustp2pBuilder, port: c_ushort) -> c_int {
    if builder.is_null() {
        return RUSTP2P_ERROR_NULL_PTR;
    }
    let builder = unsafe { &mut *builder };
    builder.builder = std::mem::take(&mut builder.builder).udp_port(port);
    RUSTP2P_OK
}

/// Set TCP port
///
/// Configures the TCP port for this endpoint. Use 0 for a random port.
///
/// # Arguments
///
/// * `builder` - The builder handle
/// * `port` - The TCP port number (0-65535)
///
/// # Returns
///
/// * `RUSTP2P_OK` on success
/// * `RUSTP2P_ERROR_NULL_PTR` if builder is NULL
///
/// # Example
///
/// ```c
/// rustp2p_builder_tcp_port(builder, 8080);
/// ```
#[no_mangle]
pub extern "C" fn rustp2p_builder_tcp_port(builder: *mut Rustp2pBuilder, port: c_ushort) -> c_int {
    if builder.is_null() {
        return RUSTP2P_ERROR_NULL_PTR;
    }
    let builder = unsafe { &mut *builder };
    builder.builder = std::mem::take(&mut builder.builder).tcp_port(port);
    RUSTP2P_OK
}

/// Set node ID from IPv4 address string (e.g., "10.0.0.1")
///
/// Each node in the P2P network must have a unique node ID.
///
/// # Arguments
///
/// * `builder` - The builder handle
/// * `node_id_str` - NULL-terminated IPv4 address string
///
/// # Returns
///
/// * `RUSTP2P_OK` on success
/// * `RUSTP2P_ERROR_NULL_PTR` if builder or node_id_str is NULL
/// * `RUSTP2P_ERROR_INVALID_STR` if the string is not valid UTF-8
/// * `RUSTP2P_ERROR_INVALID_IP` if the IP address format is invalid
///
/// # Example
///
/// ```c
/// int result = rustp2p_builder_node_id(builder, "10.0.0.1");
/// if (result != RUSTP2P_OK) {
///     fprintf(stderr, "Failed to set node ID\n");
/// }
/// ```
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
    builder.builder = std::mem::take(&mut builder.builder).node_id(ipv4.into());
    RUSTP2P_OK
}

/// Set group code from string (e.g., "mygroup" or "12345")
///
/// The group code creates isolated P2P networks. Nodes with different
/// group codes cannot communicate with each other.
///
/// The string will be converted to a 16-byte array (padded with zeros if shorter).
///
/// # Arguments
///
/// * `builder` - The builder handle
/// * `group_code` - NULL-terminated group code string
///
/// # Returns
///
/// * `RUSTP2P_OK` on success
/// * `RUSTP2P_ERROR_NULL_PTR` if builder or group_code is NULL
/// * `RUSTP2P_ERROR_INVALID_STR` if the string is not valid UTF-8
/// * `RUSTP2P_ERROR` if the group code format is invalid
///
/// # Example
///
/// ```c
/// rustp2p_builder_group_code(builder, "12345");
/// ```
#[no_mangle]
pub extern "C" fn rustp2p_builder_group_code(
    builder: *mut Rustp2pBuilder,
    group_code: *const c_char,
) -> c_int {
    if builder.is_null() || group_code.is_null() {
        return RUSTP2P_ERROR_NULL_PTR;
    }

    let c_str = unsafe { CStr::from_ptr(group_code) };
    let code_str = match c_str.to_str() {
        Ok(s) => s,
        Err(_) => return RUSTP2P_ERROR_INVALID_STR,
    };

    let group_code = match GroupCode::try_from(code_str) {
        Ok(gc) => gc,
        Err(_) => return RUSTP2P_ERROR,
    };

    let builder = unsafe { &mut *builder };
    builder.builder = std::mem::take(&mut builder.builder).group_code(group_code);
    RUSTP2P_OK
}

/// Add a peer address (e.g., "udp://127.0.0.1:9090" or "tcp://192.168.1.1:8080")
///
/// Adds an initial peer that this node will connect to. You can call this
/// multiple times to add multiple peers.
///
/// # Arguments
///
/// * `builder` - The builder handle
/// * `peer_addr` - NULL-terminated peer address string (format: "protocol://ip:port")
///
/// # Returns
///
/// * `RUSTP2P_OK` on success
/// * `RUSTP2P_ERROR_NULL_PTR` if builder or peer_addr is NULL
/// * `RUSTP2P_ERROR_INVALID_STR` if the string is not valid UTF-8
/// * `RUSTP2P_ERROR` if the address format is invalid
///
/// # Example
///
/// ```c
/// rustp2p_builder_add_peer(builder, "udp://192.168.1.100:9090");
/// rustp2p_builder_add_peer(builder, "tcp://10.0.0.1:8080");
/// ```
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
    builder.peers.push(peer);
    RUSTP2P_OK
}

/// Set encryption algorithm
/// algorithm: 0 = AesGcm, 1 = ChaCha20Poly1305 (if available)
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

    // Try to create the algorithm based on what's available
    let algo = match algorithm {
        0 => {
            // AesGcm is available if either openssl or ring feature is enabled in rustp2p
            #[cfg(any(feature = "aes-gcm-openssl", feature = "aes-gcm-ring"))]
            {
                Algorithm::AesGcm(pwd)
            }
            #[cfg(not(any(feature = "aes-gcm-openssl", feature = "aes-gcm-ring")))]
            {
                let _ = pwd;
                return RUSTP2P_ERROR;
            }
        }
        1 => {
            // ChaCha20Poly1305 is available if either openssl or ring feature is enabled in rustp2p
            #[cfg(any(
                feature = "chacha20-poly1305-openssl",
                feature = "chacha20-poly1305-ring"
            ))]
            {
                Algorithm::ChaCha20Poly1305(pwd)
            }
            #[cfg(not(any(
                feature = "chacha20-poly1305-openssl",
                feature = "chacha20-poly1305-ring"
            )))]
            {
                let _ = pwd;
                return RUSTP2P_ERROR;
            }
        }
        _ => return RUSTP2P_ERROR,
    };

    let builder = unsafe { &mut *builder };
    builder.builder = std::mem::take(&mut builder.builder).encryption(algo);
    RUSTP2P_OK
}

/// Build the endpoint
///
/// Consumes the builder and creates an endpoint. The builder is freed automatically
/// and must not be used after this call, regardless of success or failure.
///
/// # Arguments
///
/// * `builder` - The builder handle (will be freed)
///
/// # Returns
///
/// Returns a valid endpoint pointer on success, or NULL on failure.
/// Common failure reasons include:
/// - Port already in use
/// - Missing required configuration (e.g., node_id)
/// - Network initialization errors
///
/// # Example
///
/// ```c
/// Rustp2pEndpoint* endpoint = rustp2p_builder_build(builder);
/// if (!endpoint) {
///     fprintf(stderr, "Failed to build endpoint\n");
///     return -1;
/// }
/// // Use endpoint...
/// rustp2p_endpoint_free(endpoint);
/// ```
#[no_mangle]
pub extern "C" fn rustp2p_builder_build(builder: *mut Rustp2pBuilder) -> *mut Rustp2pEndpoint {
    if builder.is_null() {
        return ptr::null_mut();
    }

    let builder = unsafe { Box::from_raw(builder) };
    let runtime = builder.runtime.clone();
    let mut builder_inner = builder.builder;

    // Add peers if any were configured
    if !builder.peers.is_empty() {
        builder_inner = builder_inner.peers(builder.peers);
    }

    let endpoint = match runtime.block_on(builder_inner.build()) {
        Ok(ep) => Arc::new(ep),
        Err(_) => return ptr::null_mut(),
    };

    let rust_endpoint = Rustp2pEndpoint { endpoint, runtime };
    Box::into_raw(Box::new(rust_endpoint))
}

/// Free/destroy the builder
///
/// Releases all resources associated with the builder. Only use this if you
/// did NOT call `rustp2p_builder_build` (which frees the builder automatically).
///
/// # Safety
///
/// The builder pointer must not be used after this call.
///
/// # Arguments
///
/// * `builder` - The builder handle to free (can be NULL, in which case this is a no-op)
#[no_mangle]
pub extern "C" fn rustp2p_builder_free(builder: *mut Rustp2pBuilder) {
    if !builder.is_null() {
        unsafe {
            let _ = Box::from_raw(builder);
        }
    }
}

/// Send data to a peer
///
/// Sends data to a specific peer by their node ID. This operation blocks until
/// the data is queued for sending (but not necessarily delivered).
///
/// # Arguments
///
/// * `endpoint` - The endpoint handle
/// * `dest_ip` - NULL-terminated destination node IP address string (e.g., "10.0.0.2")
/// * `data` - Pointer to data buffer
/// * `len` - Length of data in bytes
///
/// # Returns
///
/// * `RUSTP2P_OK` on success
/// * `RUSTP2P_ERROR_NULL_PTR` if any pointer is NULL
/// * `RUSTP2P_ERROR_INVALID_STR` if dest_ip is not valid UTF-8
/// * `RUSTP2P_ERROR_INVALID_IP` if dest_ip format is invalid
/// * `RUSTP2P_ERROR` on other errors (e.g., network failure)
///
/// # Example
///
/// ```c
/// const char* message = "Hello, peer!";
/// int result = rustp2p_endpoint_send_to(
///     endpoint,
///     "10.0.0.2",
///     (const uint8_t*)message,
///     strlen(message)
/// );
///
/// if (result == RUSTP2P_OK) {
///     printf("Message sent\n");
/// }
/// ```
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

    match endpoint
        .runtime
        .block_on(endpoint.endpoint.send_to(buf, ipv4))
    {
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
///
/// Blocks until data is received from any peer or an error occurs.
/// The returned structure must be freed with `rustp2p_recv_data_free`.
///
/// # Arguments
///
/// * `endpoint` - The endpoint handle
///
/// # Returns
///
/// Returns pointer to received data structure, or NULL on error or connection closed.
/// Caller must free the returned pointer with `rustp2p_recv_data_free`.
///
/// # Example
///
/// ```c
/// Rustp2pRecvData* recv_data = rustp2p_endpoint_recv_from(endpoint);
/// if (recv_data) {
///     const uint8_t* data;
///     size_t len;
///     rustp2p_recv_data_get_payload(recv_data, &data, &len);
///     printf("Received %zu bytes\n", len);
///     rustp2p_recv_data_free(recv_data);
/// }
/// ```
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
///
/// Releases all resources associated with the endpoint and closes all connections.
///
/// # Safety
///
/// The endpoint pointer must not be used after this call.
///
/// # Arguments
///
/// * `endpoint` - The endpoint handle to free (can be NULL, in which case this is a no-op)
///
/// # Example
///
/// ```c
/// rustp2p_endpoint_free(endpoint);
/// endpoint = NULL;  // Good practice to avoid use-after-free
/// ```
#[no_mangle]
pub extern "C" fn rustp2p_endpoint_free(endpoint: *mut Rustp2pEndpoint) {
    if !endpoint.is_null() {
        unsafe {
            let _ = Box::from_raw(endpoint);
        }
    }
}
