#!/usr/bin/env bash
# Checks that the Rust a release builds with is exactly the one Rust
# publishes, before building: rustup itself against its published SHA-256,
# and every file of every component of the pinned toolchain against the
# component's archive as Rust's channel manifest lists it (the manifest
# pinned here by its SHA-256, its signature checked with Rust's release key
# when this was written), with no file the archives don't have. Anything
# else stops the release. Prints the toolchain's name for RUSTUP_TOOLCHAIN.
#
#   scripts/check-toolchain.sh
#
# The archives are kept in CLAKTORY_TOOLCHAIN_CACHE
# (~/Library/Caches/claktory-toolchain), each checked against the manifest's
# hash every time; a new Rust means a new pin in scripts/toolchain.pins and in
# app/rust/rust-toolchain.toml (they must agree).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUST_VERSION="$(sed -n 's/^RUST_VERSION=\([0-9][0-9.]*\)$/\1/p' "$ROOT/scripts/toolchain.pins")"
RUST_CHANNEL_SHA256="$(sed -n 's/^RUST_CHANNEL_SHA256=\([0-9a-f]\{64\}\)$/\1/p' "$ROOT/scripts/toolchain.pins")"
[[ -n "$RUST_VERSION" && -n "$RUST_CHANNEL_SHA256" ]] || { echo "scripts/toolchain.pins has no RUST_VERSION or RUST_CHANNEL_SHA256" >&2; exit 1; }
RUSTUP_VERSION=1.29.3
RUSTUP_SHA256=63752a1919a3a4793173ec70f8f212621a7ce38ba6a246095984482f4de20f9f
HOST=aarch64-apple-darwin

CACHE="${CLAKTORY_TOOLCHAIN_CACHE:-$HOME/Library/Caches/claktory-toolchain}"
RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
TOOLCHAIN="$RUST_VERSION-$HOST"

pinned="$(sed -n 's/^channel *= *"\(.*\)"/\1/p' "$ROOT/app/rust/rust-toolchain.toml" 2>/dev/null || true)"
[[ -z "$pinned" || "$pinned" == "$RUST_VERSION" ]] || { echo "app/rust/rust-toolchain.toml pins Rust $pinned, scripts/toolchain.pins $RUST_VERSION: make them agree" >&2; exit 1; }

RUSTUP="$(command -v rustup || true)"
[[ -n "$RUSTUP" ]] || { echo "no rustup on PATH" >&2; exit 1; }
got="$(shasum -a 256 "$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$RUSTUP")" | cut -d' ' -f1)"
[[ "$got" == "$RUSTUP_SHA256" ]] || { echo "$RUSTUP isn't rustup $RUSTUP_VERSION as published (sha256 $got): not building" >&2; exit 1; }

[[ -d "$RUSTUP_HOME/toolchains/$TOOLCHAIN" ]] || { echo "Rust $RUST_VERSION isn't installed: rustup toolchain install $RUST_VERSION" >&2; exit 1; }

mkdir -p "$CACHE"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
python3 - "$RUSTUP_HOME/toolchains/$TOOLCHAIN" "$RUST_VERSION" "$RUST_CHANNEL_SHA256" "$CACHE" "$WORK" <<'PY'
import hashlib, os, sys, tarfile, tomllib, urllib.request

tc, version, channel_sha, cache, work = sys.argv[1:]
def fail(msg):
    print(msg, file=sys.stderr); sys.exit(1)
def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()
def fetch(url, path):
    if not os.path.exists(path):
        with urllib.request.urlopen(url) as r, open(path + ".partial", "wb") as f:
            while b := r.read(1 << 20):
                f.write(b)
        os.replace(path + ".partial", path)

manifest = os.path.join(cache, f"channel-rust-{version}.toml")
fetch(f"https://static.rust-lang.org/dist/channel-rust-{version}.toml", manifest)
if sha256(manifest) != channel_sha:
    os.remove(manifest)
    fail(f"Rust {version}'s channel manifest isn't the pinned {channel_sha}: not building")
pkgs = tomllib.load(open(manifest, "rb"))["pkg"]

# The components rustup installed, each a package for a target.
rustlib = os.path.join(tc, "lib", "rustlib")
components = [c for c in open(os.path.join(rustlib, "components")).read().split() if c]
bookkeeping = {"components", "multirust-channel-manifest.toml", "multirust-config.toml", "rust-installer-version"}
bookkeeping |= {f"manifest-{c}" for c in components}
expected = {os.path.join("lib", "rustlib", b) for b in bookkeeping}
links = {}  # the archives' symlinks: path -> target
real_tc = os.path.realpath(tc)
checked = 0
for comp in components:
    for name in sorted(pkgs, key=len, reverse=True):
        if comp == name:
            pkg, target = name, "*"
        elif comp.startswith(name + "-") and comp[len(name) + 1:] in pkgs[name].get("target", {}):
            pkg, target = name, comp[len(name) + 1:]
        else:
            continue
        break
    else:
        fail(f"{comp} isn't a component of Rust {version}")
    entry = pkgs[pkg]["target"][target]
    if not entry.get("available"):
        fail(f"{comp} isn't published for Rust {version}")
    url, want = entry["xz_url"], entry["xz_hash"]
    archive = os.path.join(cache, os.path.basename(url))
    fetch(url, archive)
    if sha256(archive) != want:
        os.remove(archive)
        fail(f"{os.path.basename(url)} isn't the {want} Rust {version} publishes: not building")
    out = os.path.join(work, comp)
    with tarfile.open(archive, "r:xz") as t:
        t.extractall(out, filter="data")
    top = os.path.join(out, os.listdir(out)[0])
    inner = [d for d in os.listdir(top) if os.path.isfile(os.path.join(top, d, "manifest.in"))]
    if len(inner) != 1:
        fail(f"{os.path.basename(url)} doesn't hold one component")
    base = os.path.join(top, inner[0])
    for dirpath, dirs, files in os.walk(base):
        for f in dirs + files:
            src = os.path.join(dirpath, f)
            rel = os.path.relpath(src, base)
            if rel == "manifest.in":
                continue
            dst = os.path.join(tc, rel)
            # Read only through folders that are the toolchain's own.
            if os.path.realpath(os.path.dirname(dst)) != os.path.join(real_tc, os.path.dirname(rel)).rstrip("/"):
                fail(f"{comp}: {rel} is reached through a symlink in {tc}")
            if os.path.islink(src):
                if not os.path.islink(dst) or os.readlink(dst) != os.readlink(src):
                    fail(f"{comp}: {rel} isn't the symlink Rust {version} publishes")
                links[rel] = os.readlink(src)
                expected.add(rel)
                continue
            if f in dirs:
                # rustup leaves out the archives' empty folders.
                if os.path.lexists(dst) and (os.path.islink(dst) or not os.path.isdir(dst)):
                    fail(f"{comp}: {rel} isn't a plain folder in {tc}")
                continue
            if os.path.islink(dst) or not os.path.isfile(dst):
                fail(f"{comp}: {rel} is missing or not a plain file in {tc}")
            if sha256(src) != sha256(dst):
                fail(f"{comp}: {rel} isn't the file Rust {version} publishes: not building")
            expected.add(rel)
            checked += 1
    os.system(f"rm -rf '{out}'")

extra = []
for dirpath, dirs, files in os.walk(tc):
    for f in dirs + files:
        path = os.path.join(dirpath, f)
        rel = os.path.relpath(path, tc)
        if os.path.islink(path):
            if links.get(rel) != os.readlink(path):
                fail(f"{tc} has a symlink Rust {version}'s components don't: {rel}")
        elif f in files and rel not in expected:
            extra.append(rel)
if extra:
    fail(f"{tc} has files Rust {version}'s components don't: " + ", ".join(sorted(extra)[:10]))
print(f"Rust {version}: rustup and {len(components)} components ({checked} files) are as published", file=sys.stderr)
PY
echo "$TOOLCHAIN"
