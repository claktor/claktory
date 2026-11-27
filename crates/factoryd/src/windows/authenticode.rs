//! A release's own programs are Claktory's own: before a downloaded release is offered, and before the
//! supervisor's copy is written from it, every program of Claktory's in it
//! must carry an Authenticode signature that Windows trusts, with an
//! RFC 3161 timestamp, from the same certificate profile as this backend's
//! own signature.
//!
//! The pin is the leaf certificate's profile EKU
//! (`1.3.6.1.4.1.311.97.<profile>`), which Artifact Signing puts in every
//! certificate it issues under one certificate profile and keeps across the
//! daily renewals. Not the subject or the chain: Artifact Signing's
//! intermediates sign every subscriber's certificates. Never the shared
//! `1.3.6.1.4.1.311.97.1.0`, which every profile's certificates carry. The
//! organization in the subject must match as well, never instead.
//!
//! A backend that isn't signed (a dev build) checks nothing, as an ad hoc
//! Mac build checks no team. One whose own signature can't be read, or
//! names no single profile, refuses every release: it never falls back to
//! checking nothing.
//!
//! Moving to another certificate profile takes a bridge release signed
//! under the old one; deleting or recreating the profile would leave every
//! installed backend refusing the new one.

use std::path::{Path, PathBuf};

/// Code signing (`id-kp-codeSigning`).
const CODE_SIGNING: &str = "1.3.6.1.5.5.7.3.3";
/// What every Artifact Signing profile's EKU starts with.
const PROFILE_PREFIX: &str = "1.3.6.1.4.1.311.97.";
/// The EKU every Artifact Signing certificate carries, whatever its
/// profile: never a pin.
const SHARED_PROFILE: &str = "1.3.6.1.4.1.311.97.1.0";
/// An RFC 3161 timestamp, as an unsigned attribute of the signature.
const RFC3161_TIMESTAMP: &str = "1.3.6.1.4.1.311.3.3.1";

/// What a trusted signature's signer says.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Signer {
    /// The leaf certificate's own EKUs (its extension, nothing inherited).
    pub ekus: Vec<String>,
    /// The leaf subject's O, if it has one.
    pub organization: Option<String>,
    /// The signature carries an RFC 3161 timestamp Windows verified.
    pub timestamped: bool,
}

/// A program's Authenticode signature.
#[derive(Debug, Clone, PartialEq)]
pub enum Signature {
    /// It has none (TRUST_E_NOSIGNATURE).
    Unsigned,
    /// Windows trusts it, chain and all.
    Trusted(Signer),
}

/// The signer's profile EKU: the only one under PROFILE_PREFIX that isn't
/// the shared one, made of numbers only. None when there's none, or more
/// than one.
fn profile(signer: &Signer) -> Option<&str> {
    let mut profiles = signer.ekus.iter().map(String::as_str).filter(|eku| {
        eku.strip_prefix(PROFILE_PREFIX).is_some_and(|rest| {
            *eku != SHARED_PROFILE
                && !rest.is_empty()
                && rest.split('.').all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        })
    });
    let first = profiles.next()?;
    profiles.all(|other| other == first).then_some(first)
}

/// What this backend pins, from its own signature: None when it's unsigned
/// (nothing is checked), an error when it's signed but names no single
/// profile (everything is refused).
pub fn own_pin(own: &Result<Signature, String>) -> Result<Option<Signer>, String> {
    match own {
        Ok(Signature::Unsigned) => Ok(None),
        Ok(Signature::Trusted(signer)) if signer.ekus.iter().any(|eku| eku == CODE_SIGNING) && profile(signer).is_some() => {
            Ok(Some(signer.clone()))
        }
        Ok(Signature::Trusted(_)) => Err("this backend's own signature names no single certificate profile".into()),
        Err(error) => Err(format!("this backend's own signature can't be read: {error}")),
    }
}

/// Whether `theirs` is signed as `own` (a pin from `own_pin`) requires.
pub fn pinned(own: &Signer, theirs: &Result<Signature, String>) -> Result<(), String> {
    let signer = match theirs {
        Err(error) => return Err(format!("its signature isn't trusted: {error}")),
        Ok(Signature::Unsigned) => return Err("it isn't signed".into()),
        Ok(Signature::Trusted(signer)) => signer,
    };
    if !signer.timestamped {
        return Err("its signature has no timestamp".into());
    }
    if !signer.ekus.iter().any(|eku| eku == CODE_SIGNING) {
        return Err("its certificate isn't for code signing".into());
    }
    let (Some(wanted), Some(found)) = (profile(own), profile(signer)) else {
        return Err("its certificate names no single certificate profile".into());
    };
    if wanted != found {
        return Err(format!("it's signed under another certificate profile ({found})"));
    }
    if own.organization.is_some() && signer.organization != own.organization {
        return Err("it's signed by another organization".into());
    }
    Ok(())
}

/// Other files Windows runs by their extension. Until a signed catalog
/// covers every file of a release, none may be in one outside
/// `tools\`: nothing unsigned that runs gets in by its name.
const RUNNABLE: [&str; 10] = ["com", "bat", "cmd", "ps1", "vbs", "js", "msi", "scr", "cpl", "sys"];

/// What an unpacked release holds, outside `tools\` (other projects'
/// programs, pinned by the tarball's hash only). Links were refused when
/// it was unpacked.
#[derive(Debug, Default, PartialEq)]
pub struct Contents {
    /// Every .exe and .dll: Claktory's programs, each signed.
    pub programs: Vec<PathBuf>,
    /// Every other file Windows runs (RUNNABLE): never in a release.
    pub runnable: Vec<PathBuf>,
}

/// Lists `release` (`Contents`).
pub fn contents(release: &Path) -> std::io::Result<Contents> {
    let mut found = Contents::default();
    let mut folders = vec![release.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            let extension = path.extension().map(|extension| extension.to_string_lossy().to_ascii_lowercase());
            if kind.is_dir() {
                if !(folder == release && entry.file_name().eq_ignore_ascii_case("tools")) {
                    folders.push(path);
                }
            } else if extension.as_deref().is_some_and(|extension| extension == "exe" || extension == "dll") {
                found.programs.push(path);
            } else if extension.as_deref().is_some_and(|extension| RUNNABLE.contains(&extension)) {
                found.runnable.push(path);
            }
        }
    }
    found.programs.sort();
    found.runnable.sort();
    Ok(found)
}

/// Checks `release`: no other file Windows runs, and every program of
/// Claktory's signed as this backend is (Ok when this backend is
/// unsigned). Blocking: Windows may fetch revocation lists.
pub fn check_release(release: &Path) -> Result<(), String> {
    let contents = contents(release).map_err(|error| format!("can't list {}: {error}", release.display()))?;
    let name = |path: &Path| path.strip_prefix(release).unwrap_or(path).display().to_string();
    if let Some(runnable) = contents.runnable.first() {
        return Err(format!("{}: a file Windows runs, which a release never holds", name(runnable)));
    }
    let exe = std::env::current_exe().map_err(|error| format!("this backend can't find itself: {error}"))?;
    let Some(own) = own_pin(&signature(&exe, Revocation::None))? else { return Ok(()) };
    if !contents.programs.iter().any(|program| program.parent() == Some(release)) {
        return Err("it holds no program of Claktory's".into());
    }
    for program in &contents.programs {
        pinned(&own, &signature(program, Revocation::WholeChain)).map_err(|error| format!("{}: {error}", name(program)))?;
    }
    Ok(())
}

/// Checks one program, such as the supervisor's copy just before it's put
/// in, against this backend's own signature. Ok when this backend is
/// unsigned.
pub fn check_program(program: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|error| format!("this backend can't find itself: {error}"))?;
    let Some(own) = own_pin(&signature(&exe, Revocation::None))? else { return Ok(()) };
    pinned(&own, &signature(program, Revocation::WholeChain))
}

/// Whether WinVerifyTrust checks revocation. This backend's own signature
/// is read only for its pin, so a machine offline still reads it; a
/// release's are checked whole, as they were just downloaded.
#[derive(Clone, Copy, PartialEq)]
pub enum Revocation {
    None,
    WholeChain,
}

/// The Authenticode signature embedded in `path`, verified by
/// WinVerifyTrust (WINTRUST_ACTION_GENERIC_VERIFY_V2, the whole chain).
pub fn signature(path: &Path, revocation: Revocation) -> Result<Signature, String> {
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::WinTrust::{
        WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO, WTD_CHOICE_FILE,
        WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT, WTD_REVOKE_NONE, WTD_REVOKE_WHOLECHAIN, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY,
        WTD_UI_NONE, WinVerifyTrust,
    };

    /// TRUST_E_NOSIGNATURE: the file has no signature at all.
    const NO_SIGNATURE: i32 = 0x800B_0100_u32 as i32;

    let wide = factory_agent::windows::wide::wide(path.as_os_str()).map_err(|error| error.to_string())?;
    let mut file = WINTRUST_FILE_INFO {
        cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: wide.as_ptr(),
        hFile: std::ptr::null_mut(),
        pgKnownSubject: std::ptr::null_mut(),
    };
    let mut data = WINTRUST_DATA {
        cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: if revocation == Revocation::WholeChain { WTD_REVOKE_WHOLECHAIN } else { WTD_REVOKE_NONE },
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 { pFile: &mut file },
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: if revocation == Revocation::WholeChain { WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT } else { 0 },
        ..Default::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // SAFETY: `data` and `file` point at locals (and `wide`) that outlive
    // both calls; INVALID_HANDLE_VALUE asks for no UI.
    let status = unsafe { WinVerifyTrust(INVALID_HANDLE_VALUE as _, &mut action, (&raw mut data).cast()) };
    let result = match status {
        0 => signer(data.hWVTStateData),
        NO_SIGNATURE => Ok(Signature::Unsigned),
        other => Err(format!("{} (0x{:08X})", std::io::Error::from_raw_os_error(other), other as u32)),
    };
    let state: HANDLE = data.hWVTStateData;
    if !state.is_null() {
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        // SAFETY: closes the state the verify above opened.
        unsafe { WinVerifyTrust(INVALID_HANDLE_VALUE as _, &mut action, (&raw mut data).cast()) };
    }
    result
}

/// The first signer of a verified file's state: its leaf certificate's
/// EKUs and organization, and whether an RFC 3161 timestamp was verified.
fn signer(state: windows_sys::Win32::Foundation::HANDLE) -> Result<Signature, String> {
    use windows_sys::Win32::Security::WinTrust::{WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData};

    // SAFETY: `state` is WinVerifyTrust's, still open.
    let data = unsafe { WTHelperProvDataFromStateData(state) };
    if data.is_null() {
        return Err("Windows kept no state for the signature".into());
    }
    // SAFETY: `data` is the open state's; signer 0 is the primary one.
    let sgnr = unsafe { WTHelperGetProvSignerFromChain(data, 0, 0, 0) };
    if sgnr.is_null() {
        return Err("the signature has no signer".into());
    }
    // SAFETY: a non-null signer of the open state, read before it's closed.
    let sgnr = unsafe { &*sgnr };
    if sgnr.csCertChain == 0 || sgnr.pasCertChain.is_null() || sgnr.psSigner.is_null() {
        return Err("the signature has no certificate".into());
    }
    // SAFETY: csCertChain > 0, so element 0 (the leaf) is there.
    let leaf = unsafe { (*sgnr.pasCertChain).pCert };
    if leaf.is_null() {
        return Err("the signature has no certificate".into());
    }
    // SAFETY: psSigner is the open state's CMSG_SIGNER_INFO.
    let unauthenticated = unsafe { &(*sgnr.psSigner).UnauthAttrs };
    let stamped = (0..unauthenticated.cAttr as usize).any(|i| {
        // SAFETY: cAttr attributes at rgAttr; each pszObjId a C string.
        let attribute = unsafe { &*unauthenticated.rgAttr.add(i) };
        !attribute.pszObjId.is_null()
            && unsafe { std::ffi::CStr::from_ptr(attribute.pszObjId.cast()) }.to_bytes() == RFC3161_TIMESTAMP.as_bytes()
    });
    Ok(Signature::Trusted(Signer {
        ekus: ekus(leaf)?,
        organization: organization(leaf),
        timestamped: stamped && sgnr.csCounterSigners > 0,
    }))
}

/// The certificate's own EKU extension.
fn ekus(cert: *const windows_sys::Win32::Security::Cryptography::CERT_CONTEXT) -> Result<Vec<String>, String> {
    use windows_sys::Win32::Security::Cryptography::{CERT_FIND_EXT_ONLY_ENHKEY_USAGE_FLAG, CTL_USAGE, CertGetEnhancedKeyUsage};

    let mut size = 0u32;
    // SAFETY: a size query on a live certificate.
    if unsafe { CertGetEnhancedKeyUsage(cert, CERT_FIND_EXT_ONLY_ENHKEY_USAGE_FLAG, std::ptr::null_mut(), &mut size) } == 0 {
        // No EKU extension at all.
        return Ok(Vec::new());
    }
    // u64s, so the CTL_USAGE at its start is aligned.
    let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
    let usage = buffer.as_mut_ptr().cast::<CTL_USAGE>();
    // SAFETY: `buffer` holds `size` bytes, aligned for CTL_USAGE.
    if unsafe { CertGetEnhancedKeyUsage(cert, CERT_FIND_EXT_ONLY_ENHKEY_USAGE_FLAG, usage, &mut size) } == 0 {
        return Err(format!("its certificate's uses can't be read: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: filled in above, its strings inside `buffer`.
    let usage = unsafe { &*usage };
    Ok((0..usage.cUsageIdentifier as usize)
        .filter_map(|i| {
            // SAFETY: cUsageIdentifier C strings at rgpszUsageIdentifier.
            let oid = unsafe { *usage.rgpszUsageIdentifier.add(i) };
            (!oid.is_null()).then(|| unsafe { std::ffi::CStr::from_ptr(oid.cast()) }.to_string_lossy().into_owned())
        })
        .collect())
}

/// The O of the certificate's subject.
fn organization(cert: *const windows_sys::Win32::Security::Cryptography::CERT_CONTEXT) -> Option<String> {
    use windows_sys::Win32::Security::Cryptography::{CERT_NAME_ATTR_TYPE, CertGetNameStringW, szOID_ORGANIZATION_NAME};

    let mut buffer = [0u16; 512];
    // SAFETY: a live certificate and a buffer of that many units.
    let length = unsafe {
        CertGetNameStringW(cert, CERT_NAME_ATTR_TYPE, 0, szOID_ORGANIZATION_NAME.cast(), buffer.as_mut_ptr(), buffer.len() as u32)
    } as usize;
    // One unit is the terminator alone: no O.
    (length > 1).then(|| String::from_utf16_lossy(&buffer[..length - 1]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROFILE: &str = "1.3.6.1.4.1.311.97.990309390.766961637.194916062.941502583";

    fn signed(ekus: &[&str], organization: Option<&str>, timestamped: bool) -> Signer {
        Signer { ekus: ekus.iter().map(|eku| (*eku).to_owned()).collect(), organization: organization.map(str::to_owned), timestamped }
    }

    fn ours() -> Signer {
        signed(&[CODE_SIGNING, SHARED_PROFILE, PROFILE], Some("Claktory"), true)
    }

    #[test]
    fn a_release_signed_under_this_backends_profile_passes() {
        let own = own_pin(&Ok(Signature::Trusted(ours()))).expect("pin").expect("signed");
        assert_eq!(pinned(&own, &Ok(Signature::Trusted(ours()))), Ok(()));
        // A renewed certificate under the same profile: the same EKUs.
        let renewed = signed(&[SHARED_PROFILE, PROFILE, CODE_SIGNING], Some("Claktory"), true);
        assert_eq!(pinned(&own, &Ok(Signature::Trusted(renewed))), Ok(()));
    }

    #[test]
    fn another_profile_or_only_the_shared_one_is_refused() {
        let own = ours();
        let other = signed(&[CODE_SIGNING, SHARED_PROFILE, "1.3.6.1.4.1.311.97.12345.678"], Some("Claktory"), true);
        assert!(pinned(&own, &Ok(Signature::Trusted(other))).unwrap_err().contains("another certificate profile"));
        let shared = signed(&[CODE_SIGNING, SHARED_PROFILE], Some("Claktory"), true);
        assert!(pinned(&own, &Ok(Signature::Trusted(shared))).unwrap_err().contains("no single certificate profile"));
        // Ours and another's in one certificate names no single profile.
        let both = signed(&[CODE_SIGNING, PROFILE, "1.3.6.1.4.1.311.97.12345.678"], Some("Claktory"), true);
        assert!(pinned(&own, &Ok(Signature::Trusted(both))).is_err());
        // Something that only starts like ours.
        let longer = signed(&[CODE_SIGNING, &format!("{PROFILE}.1")], Some("Claktory"), true);
        assert!(pinned(&own, &Ok(Signature::Trusted(longer))).is_err());
        let odd = signed(&[CODE_SIGNING, "1.3.6.1.4.1.311.97.x"], Some("Claktory"), true);
        assert!(pinned(&own, &Ok(Signature::Trusted(odd))).is_err());
    }

    #[test]
    fn no_timestamp_no_code_signing_or_another_organization_is_refused() {
        let own = ours();
        let unstamped = signed(&[CODE_SIGNING, PROFILE], Some("Claktory"), false);
        assert!(pinned(&own, &Ok(Signature::Trusted(unstamped))).unwrap_err().contains("timestamp"));
        let not_code = signed(&[PROFILE], Some("Claktory"), true);
        assert!(pinned(&own, &Ok(Signature::Trusted(not_code))).unwrap_err().contains("code signing"));
        let elsewhere = signed(&[CODE_SIGNING, PROFILE], Some("Someone Else"), true);
        assert!(pinned(&own, &Ok(Signature::Trusted(elsewhere))).unwrap_err().contains("organization"));
        let none = signed(&[CODE_SIGNING, PROFILE], None, true);
        assert!(pinned(&own, &Ok(Signature::Trusted(none))).is_err());
    }

    #[test]
    fn an_unsigned_or_untrusted_program_is_refused() {
        let own = ours();
        assert!(pinned(&own, &Ok(Signature::Unsigned)).unwrap_err().contains("isn't signed"));
        assert!(pinned(&own, &Err("revoked".into())).unwrap_err().contains("isn't trusted"));
    }

    #[test]
    fn an_unsigned_backend_checks_nothing_and_a_badly_signed_one_refuses_everything() {
        assert_eq!(own_pin(&Ok(Signature::Unsigned)), Ok(None));
        assert!(own_pin(&Err("unreadable".into())).is_err());
        assert!(own_pin(&Ok(Signature::Trusted(signed(&[CODE_SIGNING, SHARED_PROFILE], None, true)))).is_err());
        assert!(own_pin(&Ok(Signature::Trusted(signed(&[PROFILE], None, true)))).is_err());
    }

    #[test]
    fn the_programs_checked_are_every_exe_and_dll_but_tools_and_nothing_else_runnable_gets_in() {
        let dir = std::env::temp_dir().join(format!("claktory-programs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = dir.as_path();
        let write = |file: &str| {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("folder");
            std::fs::write(&path, b"x").expect("file");
        };
        for file in [
            "factoryd.exe",
            "backend-version",
            "app/1.2.3/Claktory.exe",
            "app/1.2.3/flutter_windows.DLL",
            "app/1.2.3/data/app.so",
            "tools/bin/rg.exe",
            "tools/etc/profile.cmd",
            "app/tools/x.exe",
        ] {
            write(file);
        }
        let relative = |paths: &[PathBuf]| -> Vec<String> {
            paths.iter().map(|path| path.strip_prefix(root).expect("inside").to_string_lossy().replace('\\', "/")).collect()
        };
        let found = contents(root).expect("list");
        assert_eq!(
            relative(&found.programs),
            ["app/1.2.3/Claktory.exe", "app/1.2.3/flutter_windows.DLL", "app/tools/x.exe", "factoryd.exe"]
        );
        assert!(found.runnable.is_empty(), "{found:?}");
        // This test binary is unsigned, so only the runnable rule applies.
        assert_eq!(check_release(root), Ok(()));

        for runnable in ["app/1.2.3/data/run.BAT", "setup.ps1", "app/x.js", "a.cmd", "b.vbs", "c.com", "d.msi", "e.scr", "f.cpl", "g.sys"] {
            write(runnable);
        }
        let found = contents(root).expect("list");
        assert_eq!(found.runnable.len(), 10, "{found:?}");
        assert!(check_release(root).unwrap_err().contains("a file Windows runs"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unsigned_file_reads_as_unsigned_and_a_signed_one_as_its_signer() {
        // This test binary isn't signed.
        let exe = std::env::current_exe().expect("exe");
        assert_eq!(signature(&exe, Revocation::None), Ok(Signature::Unsigned));
        // Not a program at all: never read as trusted.
        let text = std::env::temp_dir().join(format!("claktory-not-a-program-{}.exe", std::process::id()));
        std::fs::write(&text, b"not a program").expect("file");
        let read = signature(&text, Revocation::None);
        let _ = std::fs::remove_file(&text);
        assert!(!matches!(read, Ok(Signature::Trusted(_))), "{read:?}");
        // Git for Windows, where the laptop has it: signed through Artifact
        // Signing, as Claktory's releases are, so a real signature of the
        // kind the pin reads. Its profile pins its own programs and
        // refuses ours, and ours refuses it.
        let git = Path::new(r"C:\Program Files\Git\cmd\git.exe");
        if git.is_file() {
            let read = signature(git, Revocation::WholeChain);
            let Ok(Signature::Trusted(signer)) = &read else { panic!("git.exe: {read:?}") };
            assert!(signer.ekus.iter().any(|eku| eku == CODE_SIGNING), "{signer:?}");
            assert!(signer.ekus.iter().any(|eku| eku == SHARED_PROFILE), "{signer:?}");
            assert!(signer.timestamped, "{signer:?}");
            let profile = profile(signer).expect("its profile");
            assert!(profile.starts_with(PROFILE_PREFIX) && profile != SHARED_PROFILE, "{signer:?}");
            let pin = own_pin(&read).expect("pin").expect("signed");
            assert_eq!(pinned(&pin, &read), Ok(()));
            assert!(pinned(&ours(), &read).is_err());
            assert!(pinned(&pin, &Ok(Signature::Trusted(ours()))).is_err());
            // One byte changed in its code: no longer trusted.
            let mut bytes = std::fs::read(git).expect("git.exe");
            let middle = bytes.len() / 3;
            bytes[middle] ^= 0xff;
            let tampered = std::env::temp_dir().join(format!("claktory-tampered-{}.exe", std::process::id()));
            std::fs::write(&tampered, bytes).expect("copy");
            let read = signature(&tampered, Revocation::None);
            let _ = std::fs::remove_file(&tampered);
            assert!(read.is_err(), "{read:?}");
            assert!(pinned(&pin, &read).unwrap_err().contains("isn't trusted"));
        }
    }
}
