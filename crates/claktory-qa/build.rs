//! The protocol's descriptors, so the driver can turn any request from JSON
//! into its wire form, and any answer back (`driver`), without code for each.

use prost::Message as _;

const SCHEMA_ROOT: &str = "../../proto";
const SCHEMA: &str = "../../proto/factory/v1/factory.proto";

fn main() {
    println!("cargo:rerun-if-changed={SCHEMA_ROOT}");
    let descriptors = protox::compile([SCHEMA], [SCHEMA_ROOT]).expect("the protocol schema compiles");
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(out.join("factory.bin"), descriptors.encode_to_vec()).expect("descriptors written");
}
