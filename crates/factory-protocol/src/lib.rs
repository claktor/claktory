//! Factory wire protocol types, generated from `proto/factory/v1/factory.proto`.

/// Protocol version this build speaks. Bump only for changes that old peers can't
/// tolerate; additive schema changes keep the same version.
pub const PROTOCOL_VERSION: u32 = 1;

// Generated code. Messages are sent once and dropped, so a large variant costs nothing worth boxing for.
#[allow(clippy::large_enum_variant)]
pub mod v1 {
    include!(concat!(env!("OUT_DIR"), "/factory.v1.rs"));
}

/// factoryd ↔ claktory-apps, the host that runs apps: internal, tied to
/// factoryd's version (`proto/factory/apps/v1/host.proto`).
pub mod apps {
    include!(concat!(env!("OUT_DIR"), "/factory.apps.v1.rs"));
}
