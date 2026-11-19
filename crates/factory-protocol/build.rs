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
    prost_build::Config::new().compile_fds(descriptors)?;
    Ok(())
}
