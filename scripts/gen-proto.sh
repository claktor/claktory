#!/usr/bin/env bash
# Regenerates the Dart protocol types from proto/. (Rust types are generated at build time.)
# Needs: protoc, and the Dart plugin: `dart pub global activate protoc_plugin`.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/app/lib/protocol"
rm -rf "$out" && mkdir -p "$out"
protoc \
  --plugin=protoc-gen-dart="$HOME/.pub-cache/bin/protoc-gen-dart" \
  --dart_out="$out" \
  -I "$root/proto" \
  "$root/proto/factory/v1/factory.proto"
# JSON descriptors are unused.
rm "$out"/factory/v1/*.pbjson.dart

# backend.dart switches over every ServerMessage body, so a new body without
# its arm stops the app compiling. Catch that here, before it lands.
if command -v flutter >/dev/null; then
  cd "$root/app"
  [[ -d .dart_tool ]] || flutter pub get >/dev/null
  if ! flutter analyze --no-pub --no-fatal-infos --no-fatal-warnings lib/backend.dart; then
    echo "backend.dart doesn't handle every ServerMessage body: add the new arm (its holder adds it) before landing." >&2
    exit 1
  fi
else
  echo "flutter not found: backend.dart not checked against the new types." >&2
fi
