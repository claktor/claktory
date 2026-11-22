#!/usr/bin/env bash
# Packs and signs an app as a .clapp, the bundle Claktory installs
# (crates/factoryd/src/apps/bundle.rs): an uncompressed ustar of
# manifest.toml, app.wasm and signature, where signature is
#   key ed25519:<base64 of the 32-byte public key>
#   sig <base64 of the 64-byte Ed25519 signature>
# over "claktory-app-v1\n" and the files' SHA-256 lines, in name order
# (`shasum -a 256 app.wasm manifest.toml`).
#
#   scripts/sign-clapp.sh DIR OUT.clapp [--key FILE] [--claktory]
#
# DIR holds manifest.toml and app.wasm. The key is FILE, or the file named by
# CLAKTORY_APPS_KEY (the vault's "Claktory apps publisher key", granted to
# whoever publishes Claktory's own apps): an Ed25519 private key, either
# OpenSSH's format (as the vault generates it) or PKCS#8 PEM (openssl
# genpkey). It's only read, and converted, if need be, in a private folder
# removed on exit. --claktory refuses unless the key's public half is one of
# CLAKTORY_KEYS in bundle.rs, so an app Claktory ships as its own (tier
# Claktory) is never signed with another key by mistake.
set -euo pipefail

usage="usage: scripts/sign-clapp.sh DIR OUT.clapp [--key FILE] [--claktory]"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dir="" out="" key="${CLAKTORY_APPS_KEY:-}" claktory=false
while (($#)); do
  case "$1" in
    --key) key="${2:?$usage}"; shift 2 ;;
    --claktory) claktory=true; shift ;;
    -*) echo "$usage" >&2; exit 2 ;;
    *) if [[ -z "$dir" ]]; then dir="$1"; elif [[ -z "$out" ]]; then out="$1"; else echo "$usage" >&2; exit 2; fi; shift ;;
  esac
done
[[ -n "$dir" && -n "$out" ]] || { echo "$usage" >&2; exit 2; }
[[ -n "$key" && -f "$key" ]] || { echo "sign-clapp: no key: give --key FILE, or run with the vault's \"Claktory apps publisher key\" (CLAKTORY_APPS_KEY)" >&2; exit 2; }
for f in manifest.toml app.wasm; do
  [[ -f "$dir/$f" ]] || { echo "sign-clapp: $dir has no $f" >&2; exit 2; }
done
[[ "$(head -c 4 "$dir/app.wasm" | od -An -tx1 | tr -d ' \n')" == 0061736d ]] || { echo "sign-clapp: $dir/app.wasm isn't WebAssembly" >&2; exit 2; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
chmod 700 "$work"
umask 077

# The key as PKCS#8 PEM, which openssl signs with.
if grep -q -- '-----BEGIN OPENSSH PRIVATE KEY-----' "$key"; then
  python3 - "$key" "$work/key.pem" <<'PY'
import base64, struct, sys
text = open(sys.argv[1]).read()
body = "".join(line for line in text.splitlines() if line and not line.startswith("-----"))
blob = base64.b64decode(body)
magic = b"openssh-key-v1\0"
if not blob.startswith(magic):
    sys.exit("sign-clapp: the key isn't an OpenSSH private key")
at = len(magic)
def string():
    global at
    (n,) = struct.unpack(">I", blob[at:at + 4]); at += 4
    value = blob[at:at + n]; at += n
    return value
cipher, kdf, _ = string(), string(), string()
if cipher != b"none":
    sys.exit("sign-clapp: the key has a passphrase; give an unencrypted one")
(count,) = struct.unpack(">I", blob[at:at + 4]); at += 4
if count != 1:
    sys.exit("sign-clapp: the key file holds more than one key")
string()  # the public key
private = string()
at = 8  # past the two check numbers
blob = private
kind = string()
if kind != b"ssh-ed25519":
    sys.exit("sign-clapp: the key isn't Ed25519 (it's %s)" % kind.decode(errors="replace"))
public = string()
secret = string()
if len(secret) != 64 or secret[32:] != public:
    sys.exit("sign-clapp: the key's halves don't match")
der = bytes.fromhex("302e020100300506032b657004220420") + secret[:32]
pem = "-----BEGIN PRIVATE KEY-----\n" + base64.b64encode(der).decode() + "\n-----END PRIVATE KEY-----\n"
open(sys.argv[2], "w").write(pem)
PY
else
  # A file secret may lack its last newline; openssl wants it.
  { cat "$key"; echo; } >"$work/key.pem"
fi
public="$(openssl pkey -in "$work/key.pem" -pubout -outform DER 2>/dev/null | tail -c 32 | base64)" ||
  { echo "sign-clapp: openssl can't read the key" >&2; exit 2; }
[[ ${#public} -eq 44 ]] || { echo "sign-clapp: the key isn't Ed25519" >&2; exit 2; }

pinned=false
grep -A20 '^const CLAKTORY_KEYS' "$root/crates/factoryd/src/apps/bundle.rs" | sed '/\];/q' | grep -qF "\"$public\"" && pinned=true
if $claktory && ! $pinned; then
  echo "sign-clapp: --claktory, but ed25519:${public:0:16}… isn't one of CLAKTORY_KEYS in crates/factoryd/src/apps/bundle.rs" >&2
  exit 1
fi

cp "$dir/app.wasm" "$dir/manifest.toml" "$work/"
(
  cd "$work"
  { printf 'claktory-app-v1\n'; shasum -a 256 app.wasm manifest.toml; } >message
  openssl pkeyutl -sign -inkey key.pem -rawin -in message -out sig.bin
  openssl pkey -in key.pem -pubout -out public.pem
  openssl pkeyutl -verify -pubin -inkey public.pem -rawin -in message -sigfile sig.bin >/dev/null
  printf 'key ed25519:%s\nsig %s\n' "$public" "$(base64 <sig.bin | tr -d '\n')" >signature
)
mkdir -p "$(dirname "$out")"
# Nothing of this Mac in the archive: no owner's name, the same mode and time.
chmod 644 "$work/manifest.toml" "$work/app.wasm" "$work/signature"
touch -t 202601010000 "$work/manifest.toml" "$work/app.wasm" "$work/signature"
COPYFILE_DISABLE=1 tar --format ustar --no-mac-metadata --uid 0 --gid 0 --uname "" --gname "" \
  -cf "$out.partial" -C "$work" manifest.toml app.wasm signature
mv -f "$out.partial" "$out"
chmod 644 "$out"
if $pinned; then tier="tier Claktory (the key is pinned in bundle.rs)"; else tier="tier Local (the key isn't one of Claktory's)"; fi
echo "$out  sha256 $(shasum -a 256 "$out" | cut -d' ' -f1), signed by ed25519:${public:0:16}…, $tier"
