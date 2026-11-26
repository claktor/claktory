//! Landlock around Chrome (agent-browser.md R2, R5; Fugo 30773): the exact
//! folders it may read and write, and TCP connections only to ports 80 and
//! 443. It fails closed: below ABI 4 (no network rules) the runner refuses
//! to run. From ABI 6 it also scopes abstract unix sockets and signals.
//!
//! The rules are made in the runner, then applied in Chrome's child just
//! before exec (`super::fence_self`), never to the runner itself: with the
//! signal scope, a fenced runner couldn't stop the screen it started
//! (spike finding 3).

use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};

use landlock::{ABI, Access, AccessFs, AccessNet, NetPort, Ruleset, RulesetAttr, RulesetCreatedAttr, Scope, path_beneath_rules};

/// The least ABI the runner takes: network rules came with 4 (Linux 6.7).
pub(crate) const LEAST: u32 = 4;

/// The kernel's Landlock ABI; 0 when it has none (or it's off).
pub(crate) fn abi() -> u32 {
    // SAFETY: LANDLOCK_CREATE_RULESET_VERSION with no attributes only asks.
    let version = unsafe { libc::syscall(libc::SYS_landlock_create_ruleset, std::ptr::null::<u8>(), 0usize, 1u32) };
    u32::try_from(version).unwrap_or(0)
}

/// Where Chrome may only read: the system, Chrome itself, NixOS's store,
/// random numbers, the CPUs' description and cgroups, the PCI devices'
/// descriptions, plus the real folders that /etc's resolver, hosts and name
/// service files link into (Ubuntu's resolv.conf is in /run/systemd/resolve:
/// without it Chrome resolves nothing; spike finding 6).
///
/// The PCI devices: Chrome's GPU process reads each one's vendor and device
/// (pcilib, through /sys/bus/pci/devices, whose entries link into
/// /sys/devices/pci*) and exits when it can't, three times, after which
/// Chrome turns off its GPU process and everything it does (spike box:
/// "GPU process was unable to boot"). Read only, as any page could learn
/// the same of the GPU from WebGL.
pub(crate) fn read_only() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = [
        "/usr",
        "/lib",
        "/lib64",
        "/bin",
        "/sbin",
        "/etc",
        "/opt/google/chrome",
        "/nix/store",
        "/run/current-system",
        "/dev/urandom",
        "/dev/zero",
        "/sys/devices/system/cpu",
        "/sys/fs/cgroup",
        "/sys/bus/pci",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    if let Ok(devices) = std::fs::read_dir("/sys/devices") {
        let mut buses: Vec<PathBuf> =
            devices.flatten().filter(|entry| entry.file_name().to_string_lossy().starts_with("pci")).map(|entry| entry.path()).collect();
        buses.sort();
        paths.extend(buses);
    }
    for link in ["/etc/resolv.conf", "/etc/hosts", "/etc/nsswitch.conf"] {
        if let Some(folder) = std::fs::canonicalize(link).ok().and_then(|real| real.parent().map(Path::to_path_buf))
            && !folder.starts_with("/etc")
            && !paths.contains(&folder)
        {
            paths.push(folder);
        }
    }
    paths.retain(|path| path.exists());
    paths
}

/// Where Chrome may also write: the kept profile, the runner's private
/// folder, shared memory, /dev/null (Chrome opens it read-write; spike
/// finding 1) and /proc, which is the runner's own (`namespace`): Chrome's
/// sandbox writes its children's uid_map, gid_map, setgroups and
/// oom_score_adj there (spike finding 2).
pub(crate) fn read_write(profile: &Path, tmp: &Path) -> Vec<PathBuf> {
    let mut paths = vec![profile.to_path_buf(), tmp.to_path_buf(), PathBuf::from("/dev/shm"), PathBuf::from("/dev/null"), PathBuf::from("/proc")];
    paths.retain(|path| path.exists());
    paths
}

/// The rules, made: `read_write` and `read_only` folders, TCP to 80 and
/// 443 only, scopes from ABI 6. Err (with why) below ABI 4: never less.
pub(crate) fn rules(read_write: &[PathBuf], read_only: &[PathBuf]) -> Result<(OwnedFd, u32), String> {
    let version = abi();
    if version < LEAST {
        return Err(format!(
            "this computer's Linux has Landlock {version}, and Claktory's browser needs 4 or later (Linux 6.7) to keep Chrome to the web's ports"
        ));
    }
    let level = ABI::from(i32::try_from(version).unwrap_or(i32::MAX));
    let fail = |error: &dyn std::fmt::Display| format!("couldn't make the browser's fence: {error}");
    let mut ruleset = Ruleset::default().handle_access(AccessFs::from_all(level)).map_err(|e| fail(&e))?;
    ruleset = ruleset.handle_access(AccessNet::from_all(level)).map_err(|e| fail(&e))?;
    if version >= 6 {
        ruleset = ruleset.scope(Scope::from_all(level)).map_err(|e| fail(&e))?;
    }
    let created = ruleset
        .create()
        .map_err(|e| fail(&e))?
        .add_rules(path_beneath_rules(read_write, AccessFs::from_all(level)))
        .map_err(|e| fail(&e))?
        .add_rules(path_beneath_rules(read_only, AccessFs::from_read(level)))
        .map_err(|e| fail(&e))?
        .add_rule(NetPort::new(80, AccessNet::ConnectTcp))
        .map_err(|e| fail(&e))?
        .add_rule(NetPort::new(443, AccessNet::ConnectTcp))
        .map_err(|e| fail(&e))?;
    let fd: Option<OwnedFd> = created.into();
    let fd = fd.ok_or_else(|| "couldn't make the browser's fence: Landlock gave no ruleset".to_owned())?;
    Ok((fd, version))
}
