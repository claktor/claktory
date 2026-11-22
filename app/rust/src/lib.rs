// Android only (its tests run everywhere): iroh's JNI context, before any
// tunnel opens (android.rs).
#[cfg(any(target_os = "android", test))]
mod android;
pub mod api;
pub mod forward;
mod frb_generated;
pub mod tunnel;
