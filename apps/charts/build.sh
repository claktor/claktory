#!/usr/bin/env bash
# Builds the Charts app and packs it as dist/charts.clapp: an uncompressed
# tar of manifest.toml, app.wasm and signature, as Claktory installs it.
#
#   apps/charts/build.sh
#
# signature is "key ed25519:<public key>" and "sig <signature>" (base64) over
# "claktory-app-v1\n" and the files' SHA-256 lines (`sha256sum app.wasm
# manifest.toml`). It's signed with a development key, made on first use at
# ~/.claktory/app-dev-key.pem, outside the repository: never commit it.
# Claktory installs a file signed by any key but its own as tier Local, and
# pins the key to the app at its first install.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
key="${CLAKTORY_APP_KEY:-$HOME/.claktory/app-dev-key.pem}"
cd "$here"

cargo build --quiet --target wasm32-wasip2 --release

if [[ ! -f "$key" ]]; then
  mkdir -p "$(dirname "$key")"
  (umask 077 && openssl genpkey -algorithm ed25519 -out "$key")
  echo "Made a development key: $key"
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cp target/wasm32-wasip2/release/claktory_charts.wasm "$work/app.wasm"
cp manifest.toml "$work/manifest.toml"
(
  cd "$work"
  { printf 'claktory-app-v1\n'; shasum -a 256 app.wasm manifest.toml; } > message
  openssl pkeyutl -sign -inkey "$key" -rawin -in message -out sig.bin
  public="$(openssl pkey -in "$key" -pubout -outform DER | tail -c 32 | base64)"
  printf 'key ed25519:%s\nsig %s\n' "$public" "$(base64 < sig.bin | tr -d '\n')" > signature
  openssl pkeyutl -verify -pubin -inkey <(openssl pkey -in "$key" -pubout) -rawin -in message -sigfile sig.bin >/dev/null
)

mkdir -p dist
COPYFILE_DISABLE=1 tar --format ustar --no-mac-metadata -cf dist/charts.clapp -C "$work" manifest.toml app.wasm signature
echo "dist/charts.clapp  sha256 $(shasum -a 256 dist/charts.clapp | cut -d' ' -f1)"
