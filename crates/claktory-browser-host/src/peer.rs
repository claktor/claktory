//! Who's on the other end of the app's socket, by code signature: the
//! socket's audit token (LOCAL_PEERTOKEN, naming that exact process, never
//! a pid that could be reused) gives the peer's SecCode, checked against a
//! code requirement. And this helper's own signature, which says what the
//! requirement is: the same team, or, in a Dev build signed ad hoc, the
//! cdhash of the app it sits in.

use std::os::fd::AsRawFd;
use std::path::Path;

/// A signature, as Security reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub identifier: String,
    /// The Developer ID team; None for ad hoc or unsigned code.
    pub team: Option<String>,
    /// Signed ad hoc: no certificate at all.
    pub adhoc: bool,
    /// The code directory's hash, as a requirement's `cdhash H"…"` names it.
    pub cdhash: Vec<u8>,
    /// Signed with the hardened runtime (kSecCodeSignatureRuntime).
    pub runtime: bool,
}

impl Signature {
    pub fn cdhash_hex(&self) -> String {
        self.cdhash.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}

/// A requirement's quoted string: refused when it could break out of its
/// quotes, which identifiers and team ids never need.
pub fn quoted(text: &str) -> Result<String, String> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_') {
        return Err(format!("{text:?} can't go in a code requirement"));
    }
    Ok(format!("\"{text}\""))
}

#[cfg(target_os = "macos")]
pub use mac::{check, of_path, own};

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;

    use core_foundation::base::{CFType, TCFType};
    use core_foundation::data::CFData;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_foundation::url::CFURL;
    use core_foundation_sys::base::OSStatus;
    use core_foundation_sys::dictionary::CFDictionaryRef;
    use core_foundation_sys::string::CFStringRef;
    use core_foundation_sys::url::CFURLRef;

    use super::*;

    type SecCodeRef = *const c_void;
    type SecStaticCodeRef = *const c_void;
    type SecRequirementRef = *const c_void;

    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        static kSecGuestAttributeAudit: CFStringRef;
        static kSecCodeInfoIdentifier: CFStringRef;
        static kSecCodeInfoTeamIdentifier: CFStringRef;
        static kSecCodeInfoUnique: CFStringRef;
        static kSecCodeInfoFlags: CFStringRef;
        static kSecCodeInfoCertificates: CFStringRef;
        fn SecCodeCopySelf(flags: u32, code: *mut SecCodeRef) -> OSStatus;
        fn SecCodeCopyGuestWithAttributes(host: SecCodeRef, attributes: CFDictionaryRef, flags: u32, guest: *mut SecCodeRef) -> OSStatus;
        fn SecStaticCodeCreateWithPath(path: CFURLRef, flags: u32, code: *mut SecStaticCodeRef) -> OSStatus;
        fn SecRequirementCreateWithString(text: CFStringRef, flags: u32, requirement: *mut SecRequirementRef) -> OSStatus;
        fn SecCodeCheckValidity(code: SecCodeRef, flags: u32, requirement: SecRequirementRef) -> OSStatus;
        fn SecStaticCodeCheckValidity(code: SecStaticCodeRef, flags: u32, requirement: SecRequirementRef) -> OSStatus;
        fn SecCodeCopySigningInformation(code: SecStaticCodeRef, flags: u32, information: *mut CFDictionaryRef) -> OSStatus;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(cf: *const c_void);
    }

    const SOL_LOCAL: libc::c_int = 0;
    const LOCAL_PEERTOKEN: libc::c_int = 6;
    const SIGNING_INFORMATION: u32 = 1 << 1;
    /// kSecCSStrictValidate | kSecCSCheckAllArchitectures | kSecCSCheckNestedCode
    const STATIC_CHECK: u32 = (1 << 4) | (1 << 0) | (1 << 3);
    const ADHOC: u32 = 0x0002;
    const RUNTIME: u32 = 0x0001_0000;

    /// Releases a Security object when it goes.
    struct Owned(*const c_void);

    impl Drop for Owned {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0) };
            }
        }
    }

    /// The peer's audit token: 8 words, exactly.
    fn audit_token(socket: &impl AsRawFd) -> Result<[u8; 32], String> {
        let mut token = [0u8; 32];
        let mut len = token.len() as libc::socklen_t;
        let status = unsafe { libc::getsockopt(socket.as_raw_fd(), SOL_LOCAL, LOCAL_PEERTOKEN, token.as_mut_ptr().cast(), &mut len) };
        if status != 0 {
            return Err(format!("no audit token for the peer ({})", std::io::Error::last_os_error()));
        }
        if len as usize != token.len() {
            return Err(format!("an audit token of {len} bytes"));
        }
        Ok(token)
    }

    fn requirement(text: &str) -> Result<Owned, String> {
        let text = CFString::new(text);
        let mut requirement: SecRequirementRef = std::ptr::null();
        let status = unsafe { SecRequirementCreateWithString(text.as_concrete_TypeRef(), 0, &mut requirement) };
        if status != 0 || requirement.is_null() {
            return Err(format!("a code requirement Security can't read (OSStatus {status})"));
        }
        Ok(Owned(requirement))
    }

    /// Checks the process on the other end of `socket` against
    /// `requirement`; its signature when it passes.
    pub fn check(socket: &impl AsRawFd, requirement_text: &str) -> Result<Signature, String> {
        let requirement = requirement(requirement_text)?;
        let token = audit_token(socket)?;
        let key = unsafe { CFString::wrap_under_get_rule(kSecGuestAttributeAudit) };
        let attributes = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), CFData::from_buffer(&token).as_CFType())]);
        let mut code: SecCodeRef = std::ptr::null();
        let status = unsafe { SecCodeCopyGuestWithAttributes(std::ptr::null(), attributes.as_concrete_TypeRef(), 0, &mut code) };
        if status != 0 || code.is_null() {
            // It exited, say: never accepted for want of a check.
            return Err(format!("no code for the peer (OSStatus {status})"));
        }
        let code = Owned(code);
        let status = unsafe { SecCodeCheckValidity(code.0, 0, requirement.0) };
        let signature = information(code.0);
        if status != 0 {
            let who = signature.map_or_else(|_| "unsigned code".to_owned(), |signature| signature.identifier);
            return Err(format!("{who} isn't the app this helper works with (OSStatus {status})"));
        }
        signature
    }

    /// This process's own signature.
    pub fn own() -> Result<Signature, String> {
        let mut code: SecCodeRef = std::ptr::null();
        let status = unsafe { SecCodeCopySelf(0, &mut code) };
        if status != 0 || code.is_null() {
            return Err(format!("no signature of its own (OSStatus {status})"));
        }
        let code = Owned(code);
        information(code.0)
    }

    /// The signature of the code at `path` (an app bundle), checked whole
    /// first: a bundle whose files don't match its signature has none.
    pub fn of_path(path: &Path) -> Result<Signature, String> {
        let url = CFURL::from_path(path, path.is_dir()).ok_or_else(|| format!("{} isn't a path", path.display()))?;
        let mut code: SecStaticCodeRef = std::ptr::null();
        let status = unsafe { SecStaticCodeCreateWithPath(url.as_concrete_TypeRef(), 0, &mut code) };
        if status != 0 || code.is_null() {
            return Err(format!("no code at {} (OSStatus {status})", path.display()));
        }
        let code = Owned(code);
        let status = unsafe { SecStaticCodeCheckValidity(code.0, STATIC_CHECK, std::ptr::null()) };
        if status != 0 {
            return Err(format!("{} isn't validly signed (OSStatus {status})", path.display()));
        }
        information(code.0)
    }

    fn information(code: *const c_void) -> Result<Signature, String> {
        let mut info: CFDictionaryRef = std::ptr::null();
        let status = unsafe { SecCodeCopySigningInformation(code, SIGNING_INFORMATION, &mut info) };
        if status != 0 || info.is_null() {
            return Err(format!("no signing information (OSStatus {status})"));
        }
        let info: CFDictionary = unsafe { CFDictionary::wrap_under_create_rule(info) };
        let get =
            |key: CFStringRef| -> Option<CFType> { info.find(key.cast()).map(|value| unsafe { CFType::wrap_under_get_rule(*value) }) };
        let string = |key: CFStringRef| get(key).and_then(|value| value.downcast::<CFString>()).map(|value| value.to_string());
        let identifier = unsafe { string(kSecCodeInfoIdentifier) }.ok_or("unsigned code")?;
        let team = unsafe { string(kSecCodeInfoTeamIdentifier) }.filter(|team| !team.is_empty());
        let flags = unsafe { get(kSecCodeInfoFlags) }.and_then(|value| value.downcast::<CFNumber>()).and_then(|value| value.to_i64());
        let certificates = unsafe { get(kSecCodeInfoCertificates) }.is_some();
        let adhoc = flags.map_or(!certificates, |flags| flags as u32 & ADHOC != 0);
        let runtime = flags.is_some_and(|flags| flags as u32 & RUNTIME != 0);
        let cdhash = unsafe { get(kSecCodeInfoUnique) }
            .and_then(|value| value.downcast::<CFData>())
            .map(|data| data.bytes().to_vec())
            .unwrap_or_default();
        if cdhash.is_empty() {
            return Err(format!("{identifier} has no code directory hash"));
        }
        Ok(Signature { identifier, team, adhoc, cdhash, runtime })
    }
}

/// Elsewhere there's no app to check: every peer is refused.
#[cfg(not(target_os = "macos"))]
pub fn check(_socket: &impl AsRawFd, _requirement: &str) -> Result<Signature, String> {
    Err("the browser helper works only on macOS".into())
}

#[cfg(not(target_os = "macos"))]
pub fn own() -> Result<Signature, String> {
    Err("the browser helper works only on macOS".into())
}

#[cfg(not(target_os = "macos"))]
pub fn of_path(_path: &Path) -> Result<Signature, String> {
    Err("the browser helper works only on macOS".into())
}
