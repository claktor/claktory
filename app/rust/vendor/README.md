# Vendored crates

The app's Rust tunnel (`app/rust`) builds these copies in place of crates.io's,
through `[patch.crates-io]` in `app/rust/Cargo.toml`. Only this workspace sees
them: the backend (`crates/`) keeps crates.io's.

## n0-dns-resolver 0.1.1

iroh's DNS resolver, version **0.1.1** exactly (iroh 1.5.0's), from crates.io
(`https://crates.io/crates/n0-dns-resolver/0.1.1`, source in
`https://github.com/n0-computer/iroh`).

**Why:** upstream reads the system's DNS servers from SystemConfiguration's
dynamic store (`SCDynamicStoreCreateWithOptions`, `SCDynamicStoreCopyValue`) on
every Apple platform. On iOS that API is `API_UNAVAILABLE(ios)`, so an iPhone
or iPad app that links it risks App Review rejecting it for private API
(guideline 2.5.1; coordination/store-launch.md, S2). iroh builds its own
resolvers internally (`DnsResolver::new()`), so the app can't keep the code out
by passing a resolver of its own.

**What differs from upstream**, and nothing else:
- `src/system_config/apple.rs` is built for macOS only; its doc comment says so.
- `src/system_config/ios.rs` is new: iOS (and tvOS, watchOS) read the default
  resolver's nameservers through libresolv (`res_ninit`, `res_getservers`;
  public `<resolv.h>`), which Apple's libresolv fills from the same system DNS
  configuration. When it gives none, the caller falls back exactly as before:
  iroh's public resolvers, never no DNS.
- `src/system_config.rs` picks `apple` on macOS and `ios` on the other Apple
  targets.
- `Cargo.toml`: `system-configuration` only for macOS; the examples and the
  `lossy_compare` test aren't copied, so their entries are gone.
- Not copied: `examples/`, `tests/`, `Cargo.toml.orig`, `Cargo.lock`,
  `Makefile.toml`, `deny.toml`, `AI_POLICY.md`. The licences
  (`LICENSE-MIT`, `LICENSE-APACHE`) and `README.md` are upstream's.

**To compare with upstream:**

```sh
# crates.io's copy, which any build of the backend leaves in cargo's registry:
diff -ru ~/.cargo/registry/src/*/n0-dns-resolver-0.1.1/src app/rust/vendor/n0-dns-resolver/src
```

The iroh repository's tag for that release holds the same files under
`n0-dns-resolver/`.

**Every iroh or n0-dns-resolver upgrade** re-vendors this copy from the new
upstream release and applies the same changes again, so it never pins an older
resolver past upstream's fixes. `[patch.crates-io]` applies only while the
version here matches the one iroh asks for: a mismatch makes cargo warn that
the patch is unused, and the iOS build would go back to upstream's
SCDynamicStore, which `scripts/ios-privacy-scan.py` then refuses.

**Drop this copy** once upstream reads SCDynamicStore on macOS only: remove the
`[patch.crates-io]` entry and this folder, and check that
`nm -u` on the iOS framework lists no `SCDynamicStore` symbol.
