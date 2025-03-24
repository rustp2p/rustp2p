# rust-p2p

NAT traversal for p2p communication, this is implemented in terms of a hole-punching technique.

[![Crates.io](https://img.shields.io/crates/v/rust-p2p-core.svg)](https://crates.io/crates/rust-p2p-core)
![rust-p2p-core](https://docs.rs/rust-p2p-core/badge.svg)

This crate provides a convenient way to create connections between multiple remote peers that may be behind Nats, these pipelines that are spawned from the pipe can be used to read/write bytes from/to a peer to another.

The underlying transport protocols are `TCP`, `UDP` in the pipelines, users can even extend the protocol for the pipeline by using the powerful trait.

This crate is built on the async ecosystem tokio

# Supported Platforms

It's a cross-platform crate

# Usage

Add this dependency to your `cargo.toml`

```toml
rust-p2p-core = {version = "0.1"}
```

# Example


