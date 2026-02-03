#!/bin/bash
set -e

# Build the C FFI library
echo "Building rustp2p C FFI library..."
cd "$(dirname "$0")"
cargo build --release

echo ""
echo "Library built successfully!"
echo "Shared library: target/release/librustp2p_c_ffi.so (Linux)"
echo "Static library: target/release/librustp2p_c_ffi.a"
echo ""
echo "To use in your C project:"
echo "  gcc your_program.c -o your_program -L./target/release -lrustp2p_c_ffi -I./rustp2p-c-ffi -lpthread -ldl -lm"
echo ""
echo "To run (Linux):"
echo "  LD_LIBRARY_PATH=./target/release ./your_program"
echo ""
echo "See rustp2p-c-ffi/README.md for more details and examples."
