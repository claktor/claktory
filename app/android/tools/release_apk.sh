#!/usr/bin/env bash
# Builds Claktory's release APK for installing from a link: prod flavor,
# arm64 + x86_64, signed with the release key, stamped with Claktory's
# version, then checked (app/android/tools/check_artifact.py, apksigner)
# and copied out with its SHA-256 beside it.
#
#   app/android/tools/release_apk.sh v0.1.15 [REBUILD] --key FILE --out DIR
#
# VERSION is Claktory's (vMAJOR.MINOR.PATCH), stamped into the app
# (FACTORY_VERSION, which its version checks and footer read) and its
# versionName. versionCode is MAJOR×10,000,000 + MINOR×100,000 + PATCH×100
# + REBUILD (0 by default; count it up to ship the same version again), so
# every APK installs over the last one (app/android/DESIGN.md, "Release
# signing"). FILE is the release key's properties (storeFile,
# storePassword, keyAlias, keyPassword); it's never copied anywhere.
#
# Needs JAVA_HOME and ANDROID_HOME (app/android/toolchain.properties' JDK
# and SDK). Builds the checkout it's run in (the current folder's), as
# committed: run it on a clean, landed commit, which it names in its output.
set -euo pipefail

usage="usage: release_apk.sh vMAJOR.MINOR.PATCH [REBUILD] --key FILE --out DIR"
version="" rebuild=0 key="" out=""
while (($#)); do
  case "$1" in
    --key) key="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    v*) version="$1"; shift ;;
    [0-9]*) rebuild="$1"; shift ;;
    *) echo "$usage" >&2; exit 2 ;;
  esac
done
[[ -n "$version" && -n "$key" && -n "$out" ]] || { echo "$usage" >&2; exit 2; }
[[ "$version" =~ ^v([0-9]+)\.([0-9]+)\.([0-9]+)$ ]] || { echo "Not a version: $version" >&2; exit 2; }
major=${BASH_REMATCH[1]} minor=${BASH_REMATCH[2]} patch=${BASH_REMATCH[3]}
((minor < 100 && patch < 1000 && rebuild < 100)) || { echo "$version/$rebuild doesn't fit the versionCode scheme" >&2; exit 2; }
code=$((major * 10000000 + minor * 100000 + patch * 100 + rebuild))
[[ -f "$key" ]] || { echo "No key properties at $key" >&2; exit 2; }
: "${JAVA_HOME:?set JAVA_HOME}" "${ANDROID_HOME:?set ANDROID_HOME}"

root="$(git rev-parse --show-toplevel)"
app="$root/app"
if [[ -n "$(git -C "$root" status --porcelain)" ]]; then
  echo "The checkout has uncommitted changes: build a clean commit." >&2
  exit 1
fi
commit="$(git -C "$root" rev-parse --short=10 HEAD)"
build_tools="$ANDROID_HOME/build-tools/$(sed -n 's/^android.buildTools=//p' "$app/android/toolchain.properties")"

echo "Building Claktory $version (versionCode $code) from $commit"
(cd "$app" && ORG_GRADLE_PROJECT_claktoryKeyProperties="$key" flutter build apk --release \
  --flavor prod --target-platform android-arm64,android-x64 \
  --build-name "${version#v}" --build-number "$code" \
  --dart-define=FACTORY_VERSION="$version")
apk="$app/build/app/outputs/flutter-apk/app-prod-release.apk"

python3 "$app/android/tools/check_artifact.py" "$apk"
"$build_tools/apksigner" verify "$apk"
badging="$("$build_tools/aapt2" dump badging "$apk" | sed -n 1p)"
[[ "$badging" == *"versionCode='$code'"* && "$badging" == *"versionName='${version#v}'"* ]] \
  || { echo "The APK says otherwise: $badging" >&2; exit 1; }

mkdir -p "$out"
name="claktory-${version#v}-$code.apk"
cp "$apk" "$out/$name"
(cd "$out" && shasum -a 256 "$name" > "$name.sha256")
echo "Built $out/$name from $commit"
cat "$out/$name.sha256"
"$build_tools/apksigner" verify --print-certs "$out/$name" | sed -n 's/.*certificate SHA-256 digest: /signing certificate SHA-256: /p'
