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
