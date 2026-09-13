//! Compile the wire schema.
//!
//! `protox` parses the `.proto` in Rust and hands `tonic-prost-build` the
//! descriptor set it would otherwise have got by running `protoc`. That keeps
//! the build hermetic: no protobuf compiler has to be installed, and the build
//! cannot fail differently depending on which version of one happens to be on
//! the machine.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    const SCHEMA: &str = "proto/slate/v1/records.proto";
    const STATUS: &str = "proto/google/rpc/status.proto";
    const DETAILS: &str = "proto/google/rpc/error_details.proto";

    let descriptors = protox::compile([SCHEMA, STATUS, DETAILS], ["proto"])?;
    tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        .compile_fds(descriptors)?;

    println!("cargo:rerun-if-changed={SCHEMA}");
    println!("cargo:rerun-if-changed={STATUS}");
    println!("cargo:rerun-if-changed={DETAILS}");
    Ok(())
}
