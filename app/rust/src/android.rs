//! Android only: hands iroh the JavaVM and the app's Application, which its
//! DNS resolver needs before any tunnel opens.
//!
//! iroh reads the phone's DNS servers through JNI (`ConnectivityManager`,
//! `LinkProperties.getDnsServers`), from a VM and a Context published to
//! `ndk_context`. Nothing publishes them for us: Dart's FFI `dlopen` never
//! calls `JNI_OnLoad`, and Flutter knows nothing of `ndk_context`. Without
//! them iroh quietly falls back to public nameservers, ignoring the phone's
//! own and its VPN's. So `ClaktoryApplication.onCreate` loads this library
//! (`System.loadLibrary`, which lets the VM bind the native method below)
//! and calls `TunnelBootstrap.install(this)` before any Dart runs; Dart's
//! later `dlopen` of the same soname gets this same loaded library, and
//! [ready] lets `open_tunnel` refuse to start without it, which also proves
//! the two are one copy (app/android/DESIGN.md §4).

use std::ffi::c_void;
use std::sync::OnceLock;

/// What the bootstrap installed: raw pointers that live until the process
/// exits (a global reference is never deleted), as `ndk_context` requires.
#[derive(Clone, Copy, Debug)]
struct Installed;

/// The bootstrap's state; one per process ([BOOTSTRAP]), its own in tests.
pub(crate) struct Bootstrap {
    installed: OnceLock<Installed>,
}

/// Why [Bootstrap::install] didn't install.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum InstallError {
    /// The VM or the Application reference was null.
    Missing,
}

impl Bootstrap {
    pub(crate) const fn new() -> Self {
        Self { installed: OnceLock::new() }
    }

    /// Publishes [vm] and [application] (a global reference that's never
    /// deleted) through [publish], once: later calls change nothing and
    /// succeed. Null pointers install nothing.
    pub(crate) fn install(
        &self,
        vm: *mut c_void,
        application: *mut c_void,
        publish: impl FnOnce(*mut c_void, *mut c_void),
    ) -> Result<(), InstallError> {
        if self.installed.get().is_some() {
            return Ok(());
        }
        if vm.is_null() || application.is_null() {
            return Err(InstallError::Missing);
        }
        self.installed.get_or_init(|| {
            publish(vm, application);
            Installed
        });
        Ok(())
    }

    pub(crate) fn is_installed(&self) -> bool {
        self.installed.get().is_some()
    }
}

/// This process's bootstrap.
pub(crate) static BOOTSTRAP: Bootstrap = Bootstrap::new();

/// Whether iroh can read this phone's DNS: an error, for the app to show,
/// when the host never installed the bootstrap.
pub fn ready() -> anyhow::Result<()> {
    anyhow::ensure!(BOOTSTRAP.is_installed(), "The private connection isn't set up on this device.");
    Ok(())
}

/// `TunnelBootstrap.nativeInstall(Application)`: true once installed
/// (also when it already was), false when it couldn't be.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_claktory_app_TunnelBootstrap_nativeInstall(
    env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    application: jni_sys::jobject,
) -> jni_sys::jboolean {
    if BOOTSTRAP.is_installed() {
        return true;
    }
    if env.is_null() || application.is_null() {
        return false;
    }
    // SAFETY: the VM calls this native method with its own valid JNIEnv
    // on an attached thread, and `application` is a live local reference
    // for the call's duration.
    unsafe {
        // jni-sys groups the table by the JNI version that added each
        // call; both of these are 1.1.
        let functions = &**env;
        let mut vm: *mut jni_sys::JavaVM = std::ptr::null_mut();
        if (functions.v1_1.GetJavaVM)(env, &mut vm) != jni_sys::JNI_OK || vm.is_null() {
            return false;
        }
        // A global reference, never deleted: the local one dies when this
        // call returns, and ndk_context needs it until the process exits.
        let global = (functions.v1_1.NewGlobalRef)(env, application);
        if global.is_null() {
            return false;
        }
        // A panic must not unwind into the VM: caught, it's a bootstrap
        // that couldn't be installed, which the host logs.
        let (vm, global) = (vm.cast(), global.cast());
        std::panic::catch_unwind(|| {
            logcat::install();
            BOOTSTRAP.install(vm, global, |vm, context| {
                // SAFETY: both live until the process exits (see above).
                iroh::dns::install_android_jni_context(vm, context)
            })
        })
        .is_ok_and(|result| result.is_ok())
    }
}

/// `TunnelBootstrap.nativeSystemDnsServers()`: how many DNS servers iroh's
/// resolver reads from this phone through the installed context, with no
/// fallback (0: it can't read them, so iroh would fall back to public
/// ones), or -1 before the bootstrap is installed. It reads the network's
/// configuration and sends nothing. The count, never the addresses, is what
/// the host logs: the proof that the context works where iroh uses it.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_claktory_app_TunnelBootstrap_nativeSystemDnsServers(
    _env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
) -> jni_sys::jint {
    if !BOOTSTRAP.is_installed() {
        return -1;
    }
    // A panic must not cross into the VM.
    std::panic::catch_unwind(|| {
        // Its builder starts with no nameservers at all: only the system's.
        let resolver = n0_dns_resolver::DnsResolver::builder().use_system_config().build();
        resolver.configured_nameservers().len()
    })
    .map_or(0, |count| count.try_into().unwrap_or(jni_sys::jint::MAX))
}

/// The tunnel's and iroh's logs, in logcat under the tag `claktory-tunnel`:
/// warnings and errors always, and more when the phone's system property
/// `debug.claktory.tunnel` says so (`adb shell setprop
/// debug.claktory.tunnel debug`, read when the app starts), so a slow or
/// failing connection can be looked into on any phone without a special
/// build. Nothing the tunnel logs holds its capability path or a pairing
/// code (app/android/DESIGN.md §5); iroh's say endpoint ids and addresses.
mod logcat {
    use tracing_subscriber::filter::{LevelFilter, Targets};

    /// What [property] asks for: iroh's and the tunnel's at that level,
    /// everything else's warnings; warnings only when it's unset or not a
    /// level.
    pub(super) fn filter(property: &str) -> Targets {
        let level = match property.trim().to_ascii_lowercase().as_str() {
            "trace" => LevelFilter::TRACE,
            "debug" => LevelFilter::DEBUG,
            "info" => LevelFilter::INFO,
            _ => LevelFilter::WARN,
        };
        Targets::new()
            .with_default(LevelFilter::WARN)
            .with_target("iroh", level)
            .with_target("iroh_relay", level)
            .with_target("netwatch", level)
            .with_target("claktory_tunnel", level)
    }

    #[cfg(target_os = "android")]
    pub(super) use device::install;

    #[cfg(target_os = "android")]
    mod device {
        use std::ffi::{CStr, CString, c_char, c_int};
        use std::io::Write;

        use tracing::{Level, Metadata};
        use tracing_subscriber::fmt::MakeWriter;
        use tracing_subscriber::prelude::*;

        const TAG: &CStr = c"claktory-tunnel";
        const PROPERTY: &CStr = c"debug.claktory.tunnel";
        // <sys/system_properties.h>
        const PROP_VALUE_MAX: usize = 92;
        // <android/log.h>
        const ANDROID_LOG_VERBOSE: c_int = 2;
        const ANDROID_LOG_DEBUG: c_int = 3;
        const ANDROID_LOG_INFO: c_int = 4;
        const ANDROID_LOG_WARN: c_int = 5;
        const ANDROID_LOG_ERROR: c_int = 6;

        unsafe extern "C" {
            fn __android_log_write(priority: c_int, tag: *const c_char, text: *const c_char) -> c_int;
            fn __system_property_get(name: *const c_char, value: *mut c_char) -> c_int;
        }

        /// Sends tracing to logcat, once per process; a second call (or
        /// another subscriber already set) changes nothing.
        pub(in super::super) fn install() {
            let mut value = [0 as c_char; PROP_VALUE_MAX];
            // SAFETY: the buffer is PROP_VALUE_MAX long, as the call requires.
            unsafe { __system_property_get(PROPERTY.as_ptr(), value.as_mut_ptr()) };
            // SAFETY: the call leaves it NUL-terminated (empty when unset).
            let property = unsafe { CStr::from_ptr(value.as_ptr()) }.to_string_lossy().into_owned();
            let layer = tracing_subscriber::fmt::layer()
                .with_writer(Logcat)
                .with_ansi(false)
                .without_time()
                .with_level(false)
                .with_target(true);
            let _ = tracing_subscriber::registry().with(layer.with_filter(super::filter(&property))).try_init();
        }

        /// One logcat line per event, at the event's priority.
        struct Logcat;

        impl<'a> MakeWriter<'a> for Logcat {
            type Writer = Line;

            fn make_writer(&'a self) -> Line {
                Line { priority: ANDROID_LOG_INFO, text: Vec::new() }
            }

            fn make_writer_for(&'a self, meta: &Metadata<'_>) -> Line {
                let priority = match *meta.level() {
                    Level::ERROR => ANDROID_LOG_ERROR,
                    Level::WARN => ANDROID_LOG_WARN,
                    Level::INFO => ANDROID_LOG_INFO,
                    Level::DEBUG => ANDROID_LOG_DEBUG,
                    // So `logcat *:D` leaves it out.
                    Level::TRACE => ANDROID_LOG_VERBOSE,
                };
                Line { priority, text: Vec::new() }
            }
        }

        struct Line {
            priority: c_int,
            text: Vec<u8>,
        }

        impl Write for Line {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.text.extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        impl Drop for Line {
            fn drop(&mut self) {
                while self.text.last().is_some_and(|byte| *byte == b'\n') {
                    self.text.pop();
                }
                self.text.retain(|byte| *byte != 0);
                if let Ok(text) = CString::new(std::mem::take(&mut self.text)) {
                    // SAFETY: both are NUL-terminated and outlive the call.
                    unsafe { __android_log_write(self.priority, TAG.as_ptr(), text.as_ptr()) };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn some() -> *mut c_void {
        std::ptr::NonNull::<u8>::dangling().as_ptr().cast()
    }

    #[test]
    fn not_ready_until_installed() {
        let bootstrap = Bootstrap::new();
        assert!(!bootstrap.is_installed());
        let calls = Cell::new(0);
        bootstrap.install(some(), some(), |_, _| calls.set(calls.get() + 1)).unwrap();
        assert!(bootstrap.is_installed());
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn installs_once() {
        let bootstrap = Bootstrap::new();
        let calls = Cell::new(0);
        for _ in 0..3 {
            bootstrap.install(some(), some(), |_, _| calls.set(calls.get() + 1)).unwrap();
        }
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn nulls_install_nothing() {
        let bootstrap = Bootstrap::new();
        let null = std::ptr::null_mut();
        let calls = Cell::new(0);
        assert_eq!(bootstrap.install(null, some(), |_, _| calls.set(1)), Err(InstallError::Missing));
        assert_eq!(bootstrap.install(some(), null, |_, _| calls.set(1)), Err(InstallError::Missing));
        assert_eq!(calls.get(), 0);
        assert!(!bootstrap.is_installed());
        // A later good call still installs.
        bootstrap.install(some(), some(), |_, _| calls.set(1)).unwrap();
        assert!(bootstrap.is_installed());
    }

    #[test]
    fn publishes_what_it_was_given() {
        let bootstrap = Bootstrap::new();
        let (vm, app) = (some(), std::ptr::NonNull::<u64>::dangling().as_ptr().cast::<c_void>());
        let seen = Cell::new((std::ptr::null_mut(), std::ptr::null_mut()));
        bootstrap.install(vm, app, |v, a| seen.set((v, a))).unwrap();
        assert_eq!(seen.get(), (vm, app));
    }

    #[test]
    fn logcat_says_warnings_unless_asked_for_more() {
        use tracing::Level;
        let enabled = |property: &str, target: &str, level: Level| logcat::filter(property).would_enable(target, &level);
        for property in ["", "nonsense", "warn"] {
            assert!(enabled(property, "iroh::socket", Level::WARN));
            assert!(!enabled(property, "iroh::socket", Level::INFO));
        }
        assert!(enabled(" Debug\n", "iroh::socket", Level::DEBUG));
        assert!(!enabled("debug", "iroh::socket", Level::TRACE));
        assert!(enabled("debug", "claktory_tunnel::tunnel", Level::DEBUG));
        assert!(enabled("info", "iroh_relay::client", Level::INFO));
        // Others stay at warnings whatever's asked.
        assert!(!enabled("trace", "hickory_proto", Level::INFO));
        assert!(enabled("trace", "hickory_proto", Level::ERROR));
    }

    #[test]
    fn the_process_bootstrap_says_why_its_not_ready() {
        // Nothing installs BOOTSTRAP in host tests.
        let error = ready().unwrap_err().to_string();
        assert_eq!(error, "The private connection isn't set up on this device.");
    }
}
