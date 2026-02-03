# C FFI Bindings for rustp2p

This directory contains C FFI bindings for the rustp2p library, enabling the use of rustp2p from C, C++, and other languages that can call C functions (including JavaScript/TypeScript via FFI mechanisms like Bun FFI, Node-FFI, etc.).

## Building

To build the FFI library:

```bash
cargo build --release
```

This will generate:
- A shared library: `target/release/librustp2p_c_ffi.so` (Linux), `librustp2p_c_ffi.dylib` (macOS), or `rustp2p_c_ffi.dll` (Windows)
- A static library: `target/release/librustp2p_c_ffi.a` (Unix) or `rustp2p_c_ffi.lib` (Windows)

## C Header File

The C header file `rustp2p.h` is included in this directory and provides the API declarations.

## API Overview

### Builder API

1. Create a new builder:
   ```c
   Rustp2pBuilder* builder = rustp2p_builder_new();
   ```

2. Configure the builder:
   ```c
   rustp2p_builder_node_id(builder, "10.0.0.1");
   rustp2p_builder_udp_port(builder, 8080);
   rustp2p_builder_tcp_port(builder, 8080);
   rustp2p_builder_group_code(builder, "mygroup");
   rustp2p_builder_add_peer(builder, "udp://127.0.0.1:9090");
   rustp2p_builder_encryption(builder, RUSTP2P_ALGO_AES_GCM, "password");
   ```

3. Build the endpoint:
   ```c
   Rustp2pEndpoint* endpoint = rustp2p_builder_build(builder);
   ```

### Endpoint Operations

1. Send data:
   ```c
   const char* message = "Hello, peer!";
   rustp2p_endpoint_send_to(endpoint, "10.0.0.2", 
                            (const uint8_t*)message, strlen(message));
   ```

2. Receive data:
   ```c
   Rustp2pRecvData* recv_data = rustp2p_endpoint_recv_from(endpoint);
   if (recv_data) {
       const uint8_t* data;
       size_t len;
       rustp2p_recv_data_get_payload(recv_data, &data, &len);
       // Process data...
       rustp2p_recv_data_free(recv_data);
   }
   ```

3. Cleanup:
   ```c
   rustp2p_endpoint_free(endpoint);
   ```

## Example

See `examples/simple_example.c` for a complete working example.

To build and run the example:

```bash
# Build the FFI library
cargo build --release

# Compile the C example
cd examples
gcc simple_example.c -o simple_example \
    -L../../target/release -lrustp2p_c_ffi \
    -I.. -lpthread -ldl -lm

# Run the example
LD_LIBRARY_PATH=../../target/release ./simple_example 10.0.0.1 8080
```

## Using with JavaScript/TypeScript

The FFI bindings can be used with JavaScript/TypeScript through various FFI mechanisms:

### Bun FFI

```typescript
import { dlopen, FFIType, suffix } from "bun:ffi";

const lib = dlopen(`librustp2p_c_ffi.${suffix}`, {
  rustp2p_builder_new: {
    returns: FFIType.ptr,
    args: [],
  },
  rustp2p_builder_node_id: {
    returns: FFIType.i32,
    args: [FFIType.ptr, FFIType.cstring],
  },
  // ... other functions
});

// Use the library
const builder = lib.symbols.rustp2p_builder_new();
lib.symbols.rustp2p_builder_node_id(builder, "10.0.0.1");
// ...
```

### Node.js FFI

```javascript
const ffi = require('ffi-napi');
const ref = require('ref-napi');

const lib = ffi.Library('librustp2p_c_ffi', {
  'rustp2p_builder_new': ['pointer', []],
  'rustp2p_builder_node_id': ['int', ['pointer', 'string']],
  // ... other functions
});

const builder = lib.rustp2p_builder_new();
lib.rustp2p_builder_node_id(builder, "10.0.0.1");
// ...
```

## Error Codes

- `RUSTP2P_OK` (0): Success
- `RUSTP2P_ERROR` (-1): Generic error
- `RUSTP2P_ERROR_NULL_PTR` (-2): NULL pointer provided
- `RUSTP2P_ERROR_INVALID_STR` (-3): Invalid string
- `RUSTP2P_ERROR_INVALID_IP` (-4): Invalid IP address
- `RUSTP2P_ERROR_BUILD_FAILED` (-5): Builder build failed
- `RUSTP2P_ERROR_WOULD_BLOCK` (-6): Operation would block
- `RUSTP2P_ERROR_EOF` (-7): End of file/stream

## Thread Safety

The FFI bindings use Tokio runtime internally. Each endpoint has its own runtime instance. The functions are designed to be safe to call from multiple threads, but it's recommended to use one endpoint per thread or use proper synchronization.

## License

Apache-2.0 (same as rustp2p)
