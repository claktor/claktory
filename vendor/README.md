# Vendored crates

The backend's workspace (`crates/`) builds these copies in place of
crates.io's, through `[patch.crates-io]` in the root `Cargo.toml`. They're
excluded from the workspace: dependencies, never members, so `cargo test
--workspace` and clippy leave them alone. The app's Rust tunnel (`app/rust`)
is a workspace of its own and keeps crates.io's (its own copies are in
`app/rust/vendor`), except iroh, whose copy here both build.

## noq-udp 1.5.0

iroh's UDP sockets (the noq QUIC stack's), version **1.5.0** exactly (iroh
1.5.0's), from crates.io (`https://crates.io/crates/noq-udp/1.5.0`, source in
`https://github.com/n0-computer/noq`, `noq-udp/`).

**Why (#392):** factoryd's tunnel listens on every address (0.0.0.0) and
answers each packet from the address it came to: noq-udp reads that address
on receive and asks for it as the source on send. On Apple platforms
upstream asks with an `IP_RECVDSTADDR` control message, which macOS ignores
on send (only FreeBSD reads it there, as `IP_SENDSRCADDR`), so the answer
leaves from the route's address instead. A device that dialled the Mac at
another of its addresses (its second network, a port forward, a VM's host)
then follows the handshake to the wrong address, the Mac's half of the
connection, pinned to the address dialled, discards it, and the direct path
stalls for good without falling back to the relay. macOS takes the source
from `IP_PKTINFO`'s `ipi_spec_dst` (xnu's `udp_check_pktinfo`), as Linux
does.

**What differs from upstream**, and nothing else:
- `src/unix.rs` (`prepare_msg`) and `src/apple_fast.rs` (`prepare_msg_x`):
  on Apple platforms an IPv4 source goes as `IP_PKTINFO` (`in_pktinfo`, its
  `ipi_spec_dst`), not `IP_RECVDSTADDR`; the BSDs and Solaris keep
  `IP_RECVDSTADDR`. Each change is marked `// Claktory:`.
- `Cargo.toml`: its test and benchmark targets and dev-dependencies are
  gone, since they aren't copied.
- Not copied: `tests/`, `benches/`, `Cargo.toml.orig`, `Cargo.lock`,
  `release.toml`. The licences (`LICENSE-MIT`, `LICENSE-APACHE`) are
  upstream's.

**Its test:** factoryd's `tests/all/remote.rs`,
`a_device_reaching_claktory_at_an_address_other_than_its_route_back_gets_answers_from_it`:
a device on 127.0.0.1 dials Claktory at this machine's network address.
Upstream's copy stalls there; this one connects.

**To compare with upstream:**

```sh
# crates.io's copy, which any build leaves in cargo's registry:
diff -ru ~/.cargo/registry/src/*/noq-udp-1.5.0/src vendor/noq-udp/src
```

**Advisories:** cargo-audit and RustSec skip path crates, so an advisory
against noq-udp 1.5.0 isn't flagged for this copy while it's here: check
noq's releases and advisories by hand when upgrading iroh, and on any noq
security release.

**Releases:** `scripts/lib.sh`'s `backend_fingerprint` hashes `vendor/`, so
a change here alone builds a new backend.

**Every iroh or noq upgrade** re-vendors this copy from the new upstream
release and applies the same change again, unless upstream has it. The
patch applies only while the version here matches the one iroh asks for: a
mismatch makes cargo warn that the patch is unused, and the test above then
fails on a Mac.

**Drop this copy** once upstream sends an IPv4 source with `IP_PKTINFO` on
Apple platforms (reported to n0: coordination/upstream/noq-udp-pktinfo.md):
remove the `[patch.crates-io]` entry and this folder.

## iroh 1.5.0

The tunnel's networking library, version **1.5.0** exactly, from crates.io
(`https://crates.io/crates/iroh/1.5.0`, source in
`https://github.com/n0-computer/iroh`, tag `v1.5.0`, commit
`ea3bbb6ab41fa23816d7f9e3220ab77d444435b2`, `iroh/`). Both the backend and the
app's tunnel build it (`app/rust/Cargo.toml` patches with this folder too), so
every Claktory endpoint, phone, Mac app and backend, has the same fixes.

**Why (Polpo's iphone-efficiency.md, v0.1.22):**
- **The relay address check every ~22 s (n0-computer/iroh#4523).** At each net
  report iroh checks this device's public address with the home relay over a
  QUIC connection (QAD). It keeps that connection to reuse, but starts its
  report empty (`Watchable::new(None)` in `run_probe_v4`/`run_probe_v6`), so
  the next report can't reuse it and makes a new QUIC connection, a full TLS
  handshake of ~7 KB, every 20 to 26 s, for as long as the endpoint lives: on
  a quiet open phone ~60% of its bytes, and on every backend around the clock.
  The fix proposed upstream: start it with the first report.
- **The relay's health ping.** iroh-relay times the client's ping out at 3x
  the last round trip, never less than 0.5 s, then drops the relay connection
  and makes it again with a new TLS session. On cellular, waking the radio can
  take longer than that: Matt's iPhone lost its relay 10 to 17 times per
  foreground hour. Here a health ping gets at least 3 s (iroh-relay's 5 s
  ceiling stays); a relay that's really gone is noticed within 3 s instead of
  0.5 s, and a direct path isn't affected.

**What differs from upstream**, and nothing else (each change is marked
`// Claktory:` or `/// Claktory:`):
- `src/net_report.rs` (`run_probe_v4`, `run_probe_v6`): the kept QAD
  connection's observer starts with `Some(report.clone())`, not `None`.
- `src/socket/transports/relay/actor.rs`: `MIN_HEALTH_CHECK_TIMEOUT` (3 s) and
  `health_check_timeout`, used for both health-check pings instead of
  `PingTracker::new_ping`'s own timeout.
- `Cargo.toml`: its examples, tests and dev-dependencies are gone, since they
  aren't copied.
- Not copied: `examples/`, `tests/`, `Cargo.toml.orig`, `Cargo.lock`,
  `release.toml`. The licences: `LICENSE-BSD3` is crates.io's; `LICENSE-MIT`
  and `LICENSE-APACHE` are the iroh repository's at `v1.5.0` (crates.io's
  package leaves them out).

**Measured** (Polpo's `efficiency/tools/idle_cost.sh`: the app's own device
endpoint, connected to a backend endpoint with relay.claktory.com, quiet for
two minutes, the device's bytes from nettop), on this Mac, 24 Nov 2026:
crates.io's iroh 30.2 and 62.6 KB a minute (Polpo's runs: 26.8 and 28.9),
with a 7.4 to 10 KB burst every ~26 s; this copy 6.4 and 6.8 KB a minute,
with none.

**Its tests:** `app/rust`'s tunnel tests (`cargo test --lib` there) and
factoryd's `tests/all/remote.rs` run on this copy.

**To compare with upstream:**

```sh
diff -ru ~/.cargo/registry/src/*/iroh-1.5.0/src vendor/iroh/src
```

**Advisories:** as for noq-udp, cargo-audit skips path crates: check iroh's
releases and advisories by hand while this copy is here.

**Drop this copy** for the first iroh release with #4523's fix: remove the
`iroh` line from both `[patch.crates-io]` tables (root and `app/rust`) and
this folder. If that release still gives the relay's health ping 0.5 s,
keep the copy for the ping floor alone, or ask upstream for it first.
