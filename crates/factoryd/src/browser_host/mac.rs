//! The runner on a Mac (#435 P6, SPLIT.md): macOS has no Landlock and no
//! Xvfb, so Claktory's browser runs in a small Linux VM through Apple's
//! `container` (Apple silicon, macOS 26 or later), from Claktory's browser
//! image: an asset of each release, checked against the digest this build
//! carries before each run (`image`). The image's entrypoint loads
//! the browser's firewall as root (hence CAP_NET_ADMIN, for that step
//! only), then becomes the browser's own user and runs the runner:
//! `factoryd browser-runner run` built for Linux from the same release, so
//! its wire is this factoryd's. C1 runs over the container's stdin and
//! stdout as over sudo's on Linux (`linux::pipes`); the kept profile lives
//! in a named volume of each Claktory home's own (Fugo 50272).
//!
//! The VM ends with its runner (stdin closing ends the runner; SIGTERM to
//! `container run` stops the VM), and is killed once `container run` has
//! gone whatever ended it, since killing that client leaves its VM
//! running. A VM left over by a factoryd that didn't get to stop it
//! (killed, say) is stopped before the next starts: each carries this
//! Claktory's label.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use factory_protocol::v1;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::Io;
use crate::browser_runner::wire::Channel;
use crate::factory::Factory;

/// The oldest `container` its flags were tried with: --masked-path and
/// --read-only-path NONE are experimental there (Fugo 38436).
const CONTAINER_MIN: (u32, u32, u32) = (1, 7, 1);
/// Where `container` is installed: Homebrew's, or Apple's installer's.
const CONTAINER: [&str; 2] = ["/opt/homebrew/bin/container", "/usr/local/bin/container"];
/// The VM's memory: Chrome with a few tabs took ~900 MiB of the default
/// 1 GiB in the spike.
const MEMORY: &str = "2048M";
/// The VM's DNS server: `container`'s own forwarder doesn't resolve on a
/// Mac running Tailscale (spike); the firewall lets the browser reach only
/// this one on port 53.
const DNS: &str = "1.1.1.1";
/// The browser's folder in the VM: the volume's mount point (C4's HOME).
const MOUNT: &str = "/var/lib/claktory-browser";
/// The label each VM carries, its value the Claktory's kind.
const LABEL: &str = "claktory.browser";
/// Whether `home` is this user's own dev Claktory, which alone takes the
/// dev image's overrides (`super::dev_home`).
fn dev(home: &Path) -> bool {
    super::dev_home(home)
}

/// The VMs' name and their volume's, by Claktory home (Fugo 50272):
/// `claktory-browser-<home id>`, so no two Claktories under one user share
/// a profile or stop each other's VM; this user's own dev Claktory and
/// tests keep theirs.
fn name(home: &Path) -> String {
    if TEST_IMAGE.get().is_some() {
        "claktory-browser-test".to_owned()
    } else if dev(home) {
        "claktory-browser-dev".to_owned()
    } else {
        format!("claktory-browser-{}", super::home_id(home))
    }
}

/// The image to run, by the reference it's loaded under (`image`), once
/// it's loaded as this build says it must be; a test's, loaded on this
/// Mac, by its name. Runs `container`: never under the Factory's lock.
fn image(home: &Path, container: &Path) -> Result<String, String> {
    if let Some(image) = TEST_IMAGE.get() {
        return Ok(image.clone());
    }
    let built = super::image::Built::this(dev(home))?;
    super::image::ensure(home, container, &built)?;
    Ok(built.reference)
}

/// A test's options before the image and arguments after it (a stand-in
/// for the runner).
static TEST_RUN: std::sync::OnceLock<(Vec<String>, Vec<String>)> = std::sync::OnceLock::new();

/// Runs the test's VMs with `options` and `arguments`.
pub(crate) fn run_for_test(options: Vec<String>, arguments: Vec<String>) {
    let _ = TEST_RUN.set((options, arguments));
}

/// A test's image (its VMs and volume named apart from any Claktory's).
static TEST_IMAGE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Runs `image` for tests, whatever this build has, in VMs and a volume
/// named `claktory-browser-test`.
pub(crate) fn image_for_test(image: String) {
    let _ = TEST_IMAGE.set(image);
}

/// Whether `image` is `name@sha256:` and 64 lowercase hex characters, its
/// name a registry path with nothing a command line could take otherwise.
pub(crate) fn pinned(image: &str) -> bool {
    let Some((name, digest)) = image.split_once("@sha256:") else { return false };
    let name_ok = !name.is_empty()
        && !name.starts_with('-')
        && name.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"./_-:".contains(&byte));
    name_ok && digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// `container`, where it's installed.
fn container() -> Option<PathBuf> {
    CONTAINER.iter().map(PathBuf::from).find(|path| path.is_file())
}

/// Why the browser can't run on this Mac, with what fixes it; None when it
/// can. Runs `container system status`: never under the Factory's lock.
pub(crate) fn problem(home: &Path) -> Option<v1::AgentBrowserSetup> {
    let unsupported = |problem: &str| v1::AgentBrowserSetup { problem: problem.to_owned(), unsupported: true, ..Default::default() };
    if std::env::consts::ARCH != "aarch64" {
        return Some(unsupported("Claktory's browser needs a Mac with Apple silicon."));
    }
    if macos_major().is_some_and(|major| major < 26) {
        return Some(unsupported("Claktory's browser needs macOS 26 or later. Update macOS in System Settings, then try again."));
    }
    if TEST_IMAGE.get().is_none()
        && let Err(why) = super::image::Built::this(dev(home))
    {
        return Some(unsupported(&why));
    }
    let Some(container) = container() else {
        let brew = Path::new("/opt/homebrew/bin/brew").is_file();
        return Some(v1::AgentBrowserSetup {
            problem: if brew {
                "Claktory's browser runs in Apple's container tool, which isn't on this Mac. Install it, then try again.".to_owned()
            } else {
                "Claktory's browser runs in Apple's container tool, which isn't on this Mac. Install it from github.com/apple/container/releases, then try again."
                    .to_owned()
            },
            command: if brew { "brew install container".to_owned() } else { String::new() },
            ..Default::default()
        });
    };
    let said = std::process::Command::new(&container).arg("--version").stdin(Stdio::null()).stderr(Stdio::null()).output();
    let version = said.ok().and_then(|said| container_version(&String::from_utf8_lossy(&said.stdout)));
    if version.is_none_or(|version| version < CONTAINER_MIN) {
        let brew = Path::new("/opt/homebrew/bin/brew").is_file();
        return Some(v1::AgentBrowserSetup {
            problem: "Claktory's browser needs Apple's container tool 1.7.1 or later. Update it, then try again.".to_owned(),
            command: if brew { "brew upgrade container".to_owned() } else { String::new() },
            ..Default::default()
        });
    }
    let running = std::process::Command::new(&container)
        .args(["system", "status"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !running {
        return Some(v1::AgentBrowserSetup {
            problem: "Apple's container tool isn't running on this Mac. Start it, then try again.".to_owned(),
            command: format!("{} system start --enable-kernel-install", container.display()),
            ..Default::default()
        });
    }
    // Its image, got and loaded the first time (W11): what the card says
    // meanwhile.
    image(home, &container).err().map(|problem| v1::AgentBrowserSetup { problem, ..Default::default() })
}

/// `container --version`'s version ("container CLI version 1.7.1 (build: …)").
fn container_version(said: &str) -> Option<(u32, u32, u32)> {
    let version = said.split("version ").nth(1)?.split_whitespace().next()?;
    let mut parts = version.split('.').map(|part| part.parse::<u32>().ok());
    Some((parts.next()??, parts.next()??, parts.next().flatten().unwrap_or(0)))
}

/// macOS's major version ("27.2.1" is 27).
fn macos_major() -> Option<u32> {
    let version = os_version()?;
    version.split('.').next()?.parse().ok()
}

#[cfg(target_os = "macos")]
fn os_version() -> Option<String> {
    let mut buffer = [0u8; 64];
    let mut length = buffer.len();
    // SAFETY: a sysctl read into a buffer of `length` bytes, which it updates.
    let read =
        unsafe { libc::sysctlbyname(c"kern.osproductversion".as_ptr(), buffer.as_mut_ptr().cast(), &mut length, std::ptr::null_mut(), 0) };
    if read != 0 {
        return None;
    }
    let text = buffer.get(..length)?;
    Some(String::from_utf8_lossy(text).trim_end_matches('\0').to_owned())
}

#[cfg(not(target_os = "macos"))]
fn os_version() -> Option<String> {
    None
}

/// Starts the runner in a new VM; or why it can't. The VM starts once
/// the one before has gone and none of this Claktory's is left (`clear`),
/// and the runner's end is told only once its VM is gone too (Fugo
/// 38515): killing `container run`, as a runner that won't stop is, leaves
/// its VM running otherwise, Chrome and all, and the next would share its
/// profile.
pub(crate) fn spawn(factory: &Arc<Factory>) -> Result<Io, String> {
    let home = factory.home().root();
    let container = container().ok_or("Apple's container tool isn't on this Mac.")?;
    let image = image(home, &container)?;
    let base = name(home);
    let kind = base.strip_prefix("claktory-browser-").unwrap_or_default().to_owned();
    // Its own name each time, so a VM still stopping never blocks the next.
    let own = format!("{base}-{:x}", crate::transcript::now_ms());
    let mut command = Command::new(&container);
    command
        .args(["run", "-i", "--rm", "--name", &own, "--label", &format!("{LABEL}={kind}")])
        .args(["-m", MEMORY, "--dns", DNS, "--cap-add", "CAP_NET_ADMIN"])
        // The runner mounts a /proc of its own (hidepid) in its own
        // namespaces; the paths `container` masks or makes read-only under
        // /proc lock that mount (EPERM). Only the VM's root sees them
        // unmasked, until the entrypoint drops to the browser's user with
        // no capabilities; the VM is the boundary.
        .args(["--masked-path", "NONE", "--read-only-path", "NONE"])
        .args(["-v", &format!("{base}:{MOUNT}")]);
    let (options, arguments) = TEST_RUN.get().cloned().unwrap_or_default();
    command.args(options).arg(&image).args(arguments);
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);

    let (to, mut to_vm) = mpsc::channel::<(Channel, Vec<u8>)>(64);
    let (from_vm, from) = mpsc::channel::<(Channel, Vec<u8>)>(64);
    let quit = CancellationToken::new();
    let quitting = quit.clone();
    tokio::spawn(async move {
        // One VM at a time: the one before goes first, its runner told to
        // end and given its time (a profile written whole), then killed.
        let _turn = TURN.lock().await;
        clear(&container, &base, &kind).await;
        // Let go of while it waited: none starts.
        if quitting.is_cancelled() {
            return;
        }
        let io = match command.spawn() {
            Ok(child) => super::linux::pipes(child),
            Err(error) => Err(error.to_string()),
        };
        let Io { to: runner, from: mut said, quit: runner_quit } = match io {
            Ok(io) => io,
            Err(why) => {
                tracing::warn!(%why, "Claktory couldn't start its browser's VM");
                return;
            }
        };
        // The host letting go (quit, or every sender gone) ends the
        // runner's stdin (pipes: then SIGTERM, then SIGKILL).
        let _ends = runner_quit.clone().drop_guard();
        tokio::spawn(async move {
            tokio::select! {
                () = quitting.cancelled() => runner_quit.cancel(),
                () = runner_quit.cancelled() => {}
            }
        });
        // Both ways until the runner's output ends.
        let mut runner = Some(runner);
        loop {
            tokio::select! {
                message = to_vm.recv(), if runner.is_some() => match message {
                    Some(message) => {
                        if let Some(runner) = &runner {
                            let _ = runner.send(message).await;
                        }
                    }
                    None => runner = None,
                },
                message = said.recv() => match message {
                    Some(message) => {
                        let _ = from_vm.send(message).await;
                    }
                    None => break,
                },
            }
        }
        drop(runner);
        // Ended: its VM too, whatever ended it; only then is the host told.
        stop(&container, &own).await;
        drop(from_vm);
    });
    Ok(Io { to, from, quit })
}

/// Held by a VM's run from before it starts until it's gone.
static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// How long a VM has to go once it's told to.
const GONE: Duration = Duration::from_secs(15);

/// Ends the VM `id` at once (its runner is gone, or never answered) and
/// waits until it's gone.
async fn stop(container: &Path, id: &str) {
    let _ = Command::new(container).args(["kill", id]).stdin(Stdio::null()).output().await;
    let _ = Command::new(container).args(["delete", "--force", id]).stdin(Stdio::null()).output().await;
    let until = tokio::time::Instant::now() + GONE;
    while listed(container).await.iter().any(|entry| entry.get("id").and_then(|value| value.as_str()) == Some(id)) {
        if tokio::time::Instant::now() >= until {
            tracing::warn!(%id, "a VM of Claktory's browser didn't go when told");
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Ends every VM of this Claktory's browser left from before (a factoryd
/// that ended without ending them), before a new one starts.
async fn clear(container: &Path, base: &str, kind: &str) {
    for id in leftovers(&listed(container).await, base, kind) {
        tracing::info!(%id, "stopping a VM Claktory's browser left running");
        stop(container, &id).await;
    }
}

/// `container list --all`, as it says it.
async fn listed(container: &Path) -> Vec<serde_json::Value> {
    let Ok(said) = Command::new(container).args(["list", "--all", "--format", "json"]).stdin(Stdio::null()).output().await else {
        return Vec::new();
    };
    serde_json::from_slice(&said.stdout).unwrap_or_default()
}

/// The ids of the VMs in `container list --format json` that are this
/// Claktory home's own: its label's value exactly `kind`, and named
/// `<base>-<hex>` as `spawn` names them, never another home's.
fn leftovers(listed: &[serde_json::Value], base: &str, kind: &str) -> Vec<String> {
    listed
        .iter()
        .filter(|entry| {
            entry.pointer(&format!("/configuration/labels/{}", LABEL.replace('/', "~1"))).and_then(|value| value.as_str()) == Some(kind)
        })
        .filter_map(|entry| entry.get("id").and_then(|id| id.as_str()))
        .filter(|id| own_id(id, base))
        .map(str::to_owned)
        .collect()
}

/// Whether `id` is a name `spawn` gives this home's VMs: `<base>-<hex>`.
fn own_id(id: &str, base: &str) -> bool {
    id.strip_prefix(base)
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_image_pinned_by_digest_runs() {
        let digest = "a".repeat(64);
        assert!(pinned(&format!("ghcr.io/claktor/claktory-browser@sha256:{digest}")));
        assert!(pinned(&format!("localhost:5000/claktory-browser@sha256:{digest}")));
        assert!(!pinned("ghcr.io/claktor/claktory-browser:latest"));
        assert!(!pinned(&format!("ghcr.io/claktor/claktory-browser@sha256:{}", "A".repeat(64))));
        assert!(!pinned(&format!("ghcr.io/claktor/claktory-browser@sha256:{}", "a".repeat(63))));
        assert!(!pinned(&format!("--entrypoint=sh@sha256:{digest}")));
        assert!(!pinned(&format!("ghcr.io/x y@sha256:{digest}")));
        assert!(!pinned(&format!("@sha256:{digest}")));
    }

    /// Fugo 50272: two Claktory homes get their own volumes; this user's
    /// own ~/.factory-dev keeps the dev one.
    #[test]
    fn each_claktory_home_keeps_its_own_volume() {
        let real = name(Path::new("/Users/someone/.factory"));
        let sandbox = name(Path::new("/Users/someone/qa/sandboxes/w12/user/.factory"));
        assert_ne!(real, sandbox);
        assert!(real.starts_with("claktory-browser-") && real != "claktory-browser-dev", "{real}");
        assert_ne!(name(Path::new("/Users/someone/qa/.factory-dev")), "claktory-browser-dev");
        if let Some(account) = super::super::account_home() {
            assert_eq!(name(&account.join(".factory-dev")), "claktory-browser-dev");
        }
    }

    /// Cleanup for home A leaves home B's VMs (and the dev one's) alone.
    #[test]
    fn leftovers_are_this_claktory_homes_own_vms_only() {
        let a = name(Path::new("/Users/someone/.factory"));
        let b = name(Path::new("/Users/someone/qa/sandboxes/w12/user/.factory"));
        let (kind_a, kind_b) = (a.strip_prefix("claktory-browser-").unwrap(), b.strip_prefix("claktory-browser-").unwrap());
        let listed: Vec<serde_json::Value> = serde_json::from_str(&format!(
            r#"[
            {{"id": "{a}-18f2a", "configuration": {{"labels": {{"claktory.browser": "{kind_a}"}}}}}},
            {{"id": "{b}-18f2b", "configuration": {{"labels": {{"claktory.browser": "{kind_b}"}}}}}},
            {{"id": "{b}-18f2e", "configuration": {{"labels": {{"claktory.browser": "{kind_a}"}}}}}},
            {{"id": "claktory-browser-dev-18f2c", "configuration": {{"labels": {{"claktory.browser": "dev"}}}}}},
            {{"id": "claktory-browser-18f2f", "configuration": {{"labels": {{"claktory.browser": "claktory"}}}}}},
            {{"id": "someone-elses", "configuration": {{"labels": {{"claktory.browser": "{kind_a}"}}}}}},
            {{"id": "{a}-18f2d", "configuration": {{"labels": {{}}}}}}
        ]"#
        ))
        .unwrap();
        assert_eq!(leftovers(&listed, &a, kind_a), [format!("{a}-18f2a")]);
        assert_eq!(leftovers(&listed, &b, kind_b), [format!("{b}-18f2b")]);
        assert_eq!(leftovers(&listed, "claktory-browser-dev", "dev"), ["claktory-browser-dev-18f2c"]);
        assert!(leftovers(&[], &a, kind_a).is_empty());
    }

    #[test]
    fn containers_version_is_read_from_what_it_says() {
        assert_eq!(container_version("container CLI version 1.7.1 (build: release, commit: unspeci)\n"), Some((1, 7, 1)));
        assert_eq!(container_version("container CLI version 2.0 (build: release)"), Some((2, 0, 0)));
        assert!(container_version("container CLI version 1.6.9").is_some_and(|version| version < CONTAINER_MIN));
        assert_eq!(container_version("something else"), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_says_its_version() {
        assert!(macos_major().is_some_and(|major| major >= 11));
    }
}
