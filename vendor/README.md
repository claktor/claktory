# Vendored crates

The backend's workspace (`crates/`) builds these copies in place of
crates.io's, through `[patch.crates-io]` in the root `Cargo.toml`. They're
excluded from the workspace: dependencies, never members, so `cargo test
--workspace` and clippy leave them alone. The app's Rust tunnel (`app/rust`)
is a workspace of its own and keeps crates.io's (its own copies are in
`app/rust/vendor`).

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
