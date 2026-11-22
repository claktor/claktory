#!/bin/bash
# Builds Claktory's Chrome extension into a folder, ready to load unpacked or
# to zip for the Chrome Web Store.
#
#   browser-extension/build.sh OUT          the release: host com.claktory.browser,
#                                           its id from the Chrome Web Store
#   browser-extension/build.sh --dev OUT    Claktory Dev: host com.claktory.browser.dev,
#                                           a fixed id from dev-public-key.b64
#                                           (jpdkjilehjhinhohikojjngehpbakcmo)
#
# The Dev key is only a public key, so the id stays the same wherever it's
# loaded; there's no private key, since unpacked loading doesn't need one.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
dev=0
if [ "${1:-}" = "--dev" ]; then dev=1; shift; fi
out="${1:?usage: build.sh [--dev] OUT}"
rm -rf "$out"
mkdir -p "$out"
cp -R "$here/src/." "$out/"
if [ "$dev" = 1 ]; then
  python3 - "$out" "$here/dev-public-key.b64" <<'PY'
import json, sys
out, key = sys.argv[1], open(sys.argv[2]).read().strip()
path = f"{out}/manifest.json"
manifest = json.load(open(path))
manifest["name"] = "Claktory Dev"
manifest["action"]["default_title"] = "Claktory Dev"
manifest["key"] = key
json.dump(manifest, open(path, "w"), indent=2)
config = f"{out}/config.js"
text = open(config).read()
assert "'com.claktory.browser'" in text
open(config, "w").write(text.replace("'com.claktory.browser'", "'com.claktory.browser.dev'"))
PY
fi
echo "$out"
