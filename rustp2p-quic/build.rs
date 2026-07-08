use std::io;

fn main() -> io::Result<()> {
    println!("cargo:rerun-if-changed=proto/rustp2p_quic.proto");

    let protoc = protoc_bin_vendored::protoc_bin_path()
        .map_err(|e| io::Error::other(format!("vendored protoc: {e}")))?;
    std::env::set_var("PROTOC", protoc);

    prost_build::Config::new()
        .compile_protos(&["proto/rustp2p_quic.proto"], &["proto"])
        .map_err(|e| io::Error::other(format!("compile protobuf: {e}")))
}
