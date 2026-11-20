//! Compiles the wire protocol schema in `proto/` into Rust types.
//! Uses protox (a pure-Rust protobuf compiler), so building needs no system `protoc`.

const SCHEMA_ROOT: &str = "../../proto";
const SCHEMA: &str = "../../proto/factory/v1/factory.proto";

#[derive(Debug, thiserror::Error)]
enum BuildError {
    #[error("invalid protocol schema: {0}")]
    Schema(#[from] protox::Error),
    #[error("could not generate Rust types: {0}")]
    Codegen(#[from] std::io::Error),
}

fn main() -> Result<(), BuildError> {
    println!("cargo:rerun-if-changed={SCHEMA_ROOT}");
    let descriptors = protox::compile([SCHEMA], [SCHEMA_ROOT])?;
    // Maps as BTreeMap: the same state always encodes to the same bytes, so
    // a connection can tell what changed (WorkspacePatch).
    prost_build::Config::new().btree_map(["."]).compile_fds(descriptors)?;
    Ok(())
}
