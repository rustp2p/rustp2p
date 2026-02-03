# Using rustp2p C FFI with JavaScript/TypeScript

This document provides examples of how to use the rustp2p C FFI bindings from JavaScript/TypeScript.

## Bun FFI Example

[Bun](https://bun.sh/) provides native FFI support that's fast and easy to use.

### Installation

```bash
# Install Bun if you haven't already
curl -fsSL https://bun.sh/install | bash

# Build the FFI library
cd rustp2p-c-ffi
cargo build --release
```

### Example Usage

```typescript
import { dlopen, FFIType, ptr, CString, suffix } from "bun:ffi";

// Load the library
const lib = dlopen(`../target/release/librustp2p_c_ffi.${suffix}`, {
  rustp2p_builder_new: {
    returns: FFIType.ptr,
    args: [],
  },
  rustp2p_builder_node_id: {
    returns: FFIType.i32,
    args: [FFIType.ptr, FFIType.cstring],
  },
  rustp2p_builder_udp_port: {
    returns: FFIType.i32,
    args: [FFIType.ptr, FFIType.u16],
  },
  rustp2p_builder_tcp_port: {
    returns: FFIType.i32,
    args: [FFIType.ptr, FFIType.u16],
  },
  rustp2p_builder_group_code: {
    returns: FFIType.i32,
    args: [FFIType.ptr, FFIType.u32],
  },
  rustp2p_builder_encryption: {
    returns: FFIType.i32,
    args: [FFIType.ptr, FFIType.i32, FFIType.cstring],
  },
  rustp2p_builder_add_peer: {
    returns: FFIType.i32,
    args: [FFIType.ptr, FFIType.cstring],
  },
  rustp2p_builder_build: {
    returns: FFIType.ptr,
    args: [FFIType.ptr],
  },
  rustp2p_endpoint_send_to: {
    returns: FFIType.i32,
    args: [FFIType.ptr, FFIType.cstring, FFIType.ptr, FFIType.usize],
  },
  rustp2p_endpoint_recv_from: {
    returns: FFIType.ptr,
    args: [FFIType.ptr],
  },
  rustp2p_recv_data_get_payload: {
    returns: FFIType.i32,
    args: [FFIType.ptr, FFIType.ptr, FFIType.ptr],
  },
  rustp2p_recv_data_get_src_id: {
    returns: FFIType.i32,
    args: [FFIType.ptr, FFIType.ptr, FFIType.usize],
  },
  rustp2p_recv_data_free: {
    returns: FFIType.void,
    args: [FFIType.ptr],
  },
  rustp2p_endpoint_free: {
    returns: FFIType.void,
    args: [FFIType.ptr],
  },
});

const symbols = lib.symbols;

// Create and configure builder
const builder = symbols.rustp2p_builder_new();
if (!builder) {
  throw new Error("Failed to create builder");
}

let ret = symbols.rustp2p_builder_node_id(builder, ptr(new CString("10.0.0.1")));
if (ret !== 0) throw new Error(`Failed to set node_id: ${ret}`);

ret = symbols.rustp2p_builder_udp_port(builder, 8080);
if (ret !== 0) throw new Error(`Failed to set udp_port: ${ret}`);

ret = symbols.rustp2p_builder_tcp_port(builder, 8080);
if (ret !== 0) throw new Error(`Failed to set tcp_port: ${ret}`);

ret = symbols.rustp2p_builder_group_code(builder, 12345);
if (ret !== 0) throw new Error(`Failed to set group_code: ${ret}`);

ret = symbols.rustp2p_builder_encryption(builder, 0, ptr(new CString("password")));
if (ret !== 0) throw new Error(`Failed to set encryption: ${ret}`);

// Add a peer (optional)
// ret = symbols.rustp2p_builder_add_peer(builder, ptr(new CString("udp://127.0.0.1:9090")));
// if (ret !== 0) throw new Error(`Failed to add peer: ${ret}`);

// Build endpoint
const endpoint = symbols.rustp2p_builder_build(builder);
if (!endpoint) {
  throw new Error("Failed to build endpoint");
}

console.log("Endpoint created successfully!");

// Send a message
const message = new TextEncoder().encode("Hello from Bun FFI!");
ret = symbols.rustp2p_endpoint_send_to(
  endpoint,
  ptr(new CString("10.0.0.2")),
  ptr(message),
  message.length
);

if (ret === 0) {
  console.log("Message sent successfully!");
} else {
  console.error(`Failed to send message: ${ret}`);
}

// Receive loop (would typically run in async loop)
// const recvData = symbols.rustp2p_endpoint_recv_from(endpoint);
// if (recvData) {
//   // Process received data
//   symbols.rustp2p_recv_data_free(recvData);
// }

// Cleanup
symbols.rustp2p_endpoint_free(endpoint);
console.log("Cleanup complete!");
```

## Node.js FFI Example

Using [ffi-napi](https://github.com/node-ffi-napi/node-ffi-napi) for Node.js:

### Installation

```bash
npm install ffi-napi ref-napi
```

### Example Usage

```javascript
const ffi = require('ffi-napi');
const ref = require('ref-napi');

// Define types
const voidPtr = ref.refType(ref.types.void);

// Load the library
const lib = ffi.Library('../target/release/librustp2p_c_ffi', {
  'rustp2p_builder_new': [voidPtr, []],
  'rustp2p_builder_node_id': ['int', [voidPtr, 'string']],
  'rustp2p_builder_udp_port': ['int', [voidPtr, 'uint16']],
  'rustp2p_builder_tcp_port': ['int', [voidPtr, 'uint16']],
  'rustp2p_builder_group_code': ['int', [voidPtr, 'uint32']],
  'rustp2p_builder_encryption': ['int', [voidPtr, 'int', 'string']],
  'rustp2p_builder_add_peer': ['int', [voidPtr, 'string']],
  'rustp2p_builder_build': [voidPtr, [voidPtr]],
  'rustp2p_endpoint_send_to': ['int', [voidPtr, 'string', voidPtr, 'size_t']],
  'rustp2p_endpoint_free': ['void', [voidPtr]],
});

// Create builder
const builder = lib.rustp2p_builder_new();
if (builder.isNull()) {
  throw new Error('Failed to create builder');
}

// Configure builder
let ret = lib.rustp2p_builder_node_id(builder, '10.0.0.1');
if (ret !== 0) throw new Error(`Failed to set node_id: ${ret}`);

ret = lib.rustp2p_builder_udp_port(builder, 8080);
if (ret !== 0) throw new Error(`Failed to set udp_port: ${ret}`);

ret = lib.rustp2p_builder_tcp_port(builder, 8080);
if (ret !== 0) throw new Error(`Failed to set tcp_port: ${ret}`);

ret = lib.rustp2p_builder_group_code(builder, 12345);
if (ret !== 0) throw new Error(`Failed to set group_code: ${ret}`);

ret = lib.rustp2p_builder_encryption(builder, 0, 'password');
if (ret !== 0) throw new Error(`Failed to set encryption: ${ret}`);

// Build endpoint
const endpoint = lib.rustp2p_builder_build(builder);
if (endpoint.isNull()) {
  throw new Error('Failed to build endpoint');
}

console.log('Endpoint created successfully!');

// Send a message
const message = Buffer.from('Hello from Node.js FFI!');
ret = lib.rustp2p_endpoint_send_to(endpoint, '10.0.0.2', message, message.length);

if (ret === 0) {
  console.log('Message sent successfully!');
} else {
  console.error(`Failed to send message: ${ret}`);
}

// Cleanup
lib.rustp2p_endpoint_free(endpoint);
console.log('Cleanup complete!');
```

## Error Codes

```typescript
const RUSTP2P_OK = 0;
const RUSTP2P_ERROR = -1;
const RUSTP2P_ERROR_NULL_PTR = -2;
const RUSTP2P_ERROR_INVALID_STR = -3;
const RUSTP2P_ERROR_INVALID_IP = -4;
const RUSTP2P_ERROR_BUILD_FAILED = -5;
const RUSTP2P_ERROR_WOULD_BLOCK = -6;
const RUSTP2P_ERROR_EOF = -7;
```

## Notes

1. **Memory Management**: Always call the `_free` functions to avoid memory leaks
2. **Thread Safety**: Each endpoint manages its own Tokio runtime
3. **Error Handling**: Check return codes for all operations
4. **Platform Support**: Works on Linux, macOS, and Windows (adjust library extension accordingly)

## Building for Different Platforms

```bash
# Linux
cargo build --release
# Output: target/release/librustp2p_c_ffi.so

# macOS
cargo build --release
# Output: target/release/librustp2p_c_ffi.dylib

# Windows
cargo build --release
# Output: target/release/rustp2p_c_ffi.dll
```
