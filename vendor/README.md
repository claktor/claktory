# Vendored crates

The backend's workspace (`crates/`) builds these copies in place of
crates.io's, through `[patch.crates-io]` in the root `Cargo.toml`. They're
excluded from the workspace: dependencies, never members, so `cargo test
--workspace` and clippy leave them alone. The app's Rust tunnel (`app/rust`)
is a workspace of its own and keeps crates.io's (its own copies are in
`app/rust/vendor`), except iroh, noq-udp and netwatch, whose copies here
both build.

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

The app's tunnel (`app/rust/Cargo.toml`) builds this copy too: a Mac app is
reached at its own addresses once a direct path opens, and a Mac on two
networks at once (Wi-Fi and Ethernet on the same LAN) answered from the
wrong one, so its direct path went silent (Matt's MacBook, Bruno 36927).
iOS and Android build the same unix path; only Apple's send changes.

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
- `src/socket/transports/relay/actor.rs` (#450): a relay connection takes its
  auth token from the endpoint's relay map at every dial (`dial_relay`), not
  once when its actor starts. A signed-in Claktory's token for Claktory's
  relay lasts 7 days and is renewed with `Endpoint::insert_relay`; upstream,
  the home relay's actor, which lives as long as the endpoint, would keep
  reconnecting with the first token long after it expired. The map is the
  only source: an entry without a token (the person signed out) dials
  without one. Its tests: factoryd's
  `remote::a_renewed_relay_token_goes_with_the_next_connection_to_the_home_relay`
  and `remote::a_relay_config_without_a_token_dials_without_one` (both fail
  without the change).
- `src/socket/remote_map/remote_state.rs` (`register_and_configure_path`):
  a relay path keeps iroh's heartbeat (`HEARTBEAT_INTERVAL`, 5 s) as its
  keep-alive, whatever the endpoint's default path keep-alive. Claktory's
  devices check their direct paths every 2 s and give up on one silent for
  6 s (app/rust's `PATH_KEEP_ALIVE`, `PATH_IDLE_TIMEOUT`), so a direct path
  that dies mid-session falls back to the relay before the app takes the
  connection for dead (Matt's MacBook, Bruno 36927); the relay path needs
  none of that, and more keep-alives on it would only cost an idle phone
  data. A backend's paths keep iroh's defaults, so nothing changes there.
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

**Drop this copy** for the first iroh release with #4523's fix that also
reads a relay's auth token at each dial (and sets a relay path's keep-alive
apart from the default's, or lets an endpoint do so): remove the `iroh` line from both
`[patch.crates-io]` tables (root and `app/rust`) and this folder. If that
release still gives the relay's health ping 0.5 s, or still reads the token
once, keep the copy for what it lacks, or ask upstream for it first.

## iroh-relay 1.5.0

iroh's relay server and client, version **1.5.0** exactly (iroh 1.5.0's),
from crates.io (`https://crates.io/crates/iroh-relay/1.5.0`, source in
`https://github.com/n0-computer/iroh`, tag `v1.5.0`, `iroh-relay/`). Only
the backend's workspace builds it (root `[patch.crates-io]`); the app's
tunnel keeps crates.io's, since nothing changed on the client's side.

**Why (#450, Fugo's coordination/security/relay-access.md):** Claktory's
relay (relay.claktory.com) is for Claktories signed in to an account. It
admits every client, and forwards a packet only when its sender or its
recipient is a signed-in Claktory (one that showed a relay token), so a
signed-in Claktory's devices and pipe peers need nothing, while two
endpoints that aren't Claktory users can't use it with each other. That
needs a say at the one place the server forwards a packet, which upstream
has no hook for. `crates/claktory-relay` is the server built on it.

**What differs from upstream**, and nothing else (each change is marked
`// Claktory:` or `/// Claktory:`):
- `src/server.rs`: `AccessControl::allow_forward(src, dst, bytes)`, by
  default true, and the same on `DynAccessControl`.
- `src/server/clients.rs`: `Clients::with_forward_policy(access)`, and
  `send_packet` asking it first: a refused packet is dropped and counted in
  `send_packets_dropped`.
- `src/server/http_server.rs`: the relay service's clients ask its access
  control.
- `src/main.rs` (the `iroh-relay` binary) is gone: `crates/claktory-relay`'s
  `main.rs` is that file, changed only where marked.
- `Cargo.toml`: its binary, tests and dev-dependencies are gone, since they
  aren't copied.
- Not copied: `tests/`, `proptest-regressions/`, `CHANGELOG.md`,
  `Cargo.toml.orig`, `Cargo.lock`, `release.toml`. The licences:
  `LICENSE-BSD3` is crates.io's; `LICENSE-MIT` and `LICENSE-APACHE` are the
  iroh repository's at `v1.5.0`, as for iroh.

**Its tests:** `cargo test -p claktory-relay`, among them a relay on this
computer with iroh's own client: a phone reaches its signed-in Claktory and
hears back, two others don't reach each other, and observing forwards and
counts it.

**To compare with upstream:**

```sh
diff -ru ~/.cargo/registry/src/*/iroh-relay-1.5.0/src vendor/iroh-relay/src
```

**Advisories:** as for iroh, cargo-audit skips path crates: check
iroh-relay's releases and advisories by hand while this copy is here. It's
the server facing the internet, so that matters more than for the others.

**Every iroh upgrade** re-vendors this copy with the same hook, or drops it
if upstream has a forwarding hook of its own (then `claktory-relay` uses
that). Upgrade the relay with the backend and the apps: the relay protocol
is versioned, but the relay's iroh shouldn't lag theirs.

## netwatch 0.20.2

iroh's UDP sockets and network monitor, version **0.20.2** exactly (iroh
1.5.0's), from crates.io (`https://crates.io/crates/netwatch/0.20.2`, source
in `https://github.com/n0-computer/net-tools`, commit
`334793cfdaf3487db0c721b9312179a8ae3b0dcb`, `netwatch/`; the crate's sha256
`2acf7a1ca9205d7c1fc9a1595193a8cbaa7537b4a07dca93c6ae3de7c8d0e597`, as
`Cargo.lock` had it before this copy). Both the backend
and the app's tunnel build it (`app/rust/Cargo.toml` patches with this folder
too).

**Why (#490, Polpo's efficiency/udp-rebind.md):** after a major network
change (an interface coming or going: a VM, an emulator, a Simulator, a
laptop waking, a phone moving to cellular) iroh rebinds its UDP sockets. When
that bind fails (`AddrInUse`, the port held for a moment), upstream leaves the
socket closed for good: `UdpSocket::rebind` returns the error without marking
the socket broken, and `maybe_rebind`, the only retry, runs only on a broken
one (n0-computer/net-tools#200). Every send then fails with a "socket closed"
warning until the process restarts, so everything goes through the relay and
the log grows by ~190 MB a day (Matt's Claktory, 26 Nov 2026: no UDP socket in
factoryd after 01:45).

**What differs from upstream**, and nothing else (marked `// Claktory:`):
- `src/udp.rs` (`UdpSocket::rebind`): a failed bind marks the socket broken,
  so its next send or receive tries the bind again and the socket heals as
  soon as the port is free. A deliberate `close()` never marks it, so it stays
  closed. The first version of upstream's PR #201 is this same line.
- `Cargo.toml`: its test targets and dev-dependencies are gone, since they
  aren't copied.
- Not copied: `tests/`, `Cargo.toml.orig`, `Cargo.lock`. The licences
  (`LICENSE-MIT`, `LICENSE-APACHE`) are the net-tools repository's (crates.io's
  package leaves them out).

**Its tests:** `app/rust`'s `udp_rebind` (`cargo test --lib udp_rebind`
there): a rebind while another socket holds the port fails and leaves the
socket broken, and once the port is free the next send binds it again, on the
same port, and goes through, and it hears again (fails without the change);
and a socket closed on purpose stays closed. The held port is a macOS case
(Linux refuses the holder), so the first test runs only on Apple platforms.

**To compare with upstream:**

```sh
diff -ru ~/.cargo/registry/src/*/netwatch-0.20.2/src vendor/netwatch/src
```

**Advisories:** as for the others, cargo-audit skips path crates: check
netwatch's releases and advisories by hand while this copy is here.

**Drop this copy** for the first netwatch release that marks a socket broken
when its rebind fails (#200's fix, in whatever form): remove the `netwatch`
line from both `[patch.crates-io]` tables (root and `app/rust`), this folder,
and `app/rust`'s `netwatch` dev-dependency with `udp_rebind` (or keep the
test against the release). A later iroh that asks for another netwatch makes
cargo warn that the patch is unused: re-vendor then, or drop it if fixed.
