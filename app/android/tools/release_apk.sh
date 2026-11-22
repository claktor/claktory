#!/usr/bin/env bash
# Builds Claktory's release APK for installing from a link: prod flavor,
# arm64 + x86_64, signed with the release key, stamped with Claktory's
# version, then checked (app/android/tools/check_artifact.py, apksigner)
# and copied out with its SHA-256 beside it. With --bundle, also the app
# bundle (.aab) Google Play takes, from the same build: checked the same
# way (with bundletool for its 16 KiB setting) and with jarsigner, and
# copied out beside the APK.
#
#   app/android/tools/release_apk.sh v0.1.15 [REBUILD] --key FILE --out DIR [--bundle]
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
# and SDK). Builds the commit checked out where it's run (the current
# folder's), clean and landed, which it names in its output: exported with
# `git archive` into one fixed folder, /tmp/claktory-android-release,
# never the checkout itself. Dart's snapshot keeps the absolute path of a
# file Flutter generates (dart-lang/sdk#55282), so a build in a home
# folder would put that path in the APK, and no build in another folder
# would match it. From that folder, with the same toolchain, the same
# commit, version and key give the same APK, byte for byte (DESIGN.md §6),
# and the same bundle. --bundle needs bundletool's jar: $BUNDLETOOL, or
# the bundletool-all-*.jar beside the SDK folder.
set -euo pipefail

usage="usage: release_apk.sh vMAJOR.MINOR.PATCH [REBUILD] --key FILE --out DIR [--bundle]"
version="" rebuild=0 key="" out="" bundle=0
while (($#)); do
  case "$1" in
    --key) key="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --bundle) bundle=1; shift ;;
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
# The build runs in another folder: the key by its full path.
key="$(cd "$(dirname "$key")" && pwd)/$(basename "$key")"
: "${JAVA_HOME:?set JAVA_HOME}" "${ANDROID_HOME:?set ANDROID_HOME}"
if ((bundle)); then
  bundletool="${BUNDLETOOL:-$(ls "$ANDROID_HOME"/../bundletool-all-*.jar 2>/dev/null | sort -V | tail -1)}"
  [[ -f "$bundletool" ]] || { echo "--bundle needs bundletool's jar: set BUNDLETOOL" >&2; exit 2; }
fi

root="$(git rev-parse --show-toplevel)"
app="$root/app"
if [[ -n "$(git -C "$root" status --porcelain)" ]]; then
  echo "The checkout has uncommitted changes: build a clean commit." >&2
  exit 1
fi
commit="$(git -C "$root" rev-parse --short=10 HEAD)"

# /tmp is everyone's: another user could make this folder first and swap
# the source mid-build, to have their code signed with the release key.
# So it must be ours alone (Fugo): made 700 under umask 077, and refused
# unless it's a real folder (not a link), owned by us, mode 700.
build_root=/tmp/claktory-android-release
umask 077
[[ -e "$build_root" || -L "$build_root" ]] || mkdir -m 700 "$build_root"
owner_mode() { stat -f '%u %Lp' "$1" 2>/dev/null || stat -c '%u %a' "$1"; }
if [[ ! -d "$build_root" || -L "$build_root" || "$(owner_mode "$build_root")" != "$(id -u) 700" ]]; then
  echo "$build_root isn't a folder of this user's alone (mode 700): refusing to build in it. Move it away." >&2
  exit 1
fi
mkdir "$build_root/lock" 2>/dev/null \
  || { echo "Another release build is using $build_root (or left $build_root/lock behind)." >&2; exit 1; }
trap 'rmdir "$build_root/lock"' EXIT
rm -rf "$build_root/factory"
mkdir "$build_root/factory"
git -C "$root" archive HEAD | tar -x -C "$build_root/factory"
app="$build_root/factory/app"
build_tools="$ANDROID_HOME/build-tools/$(sed -n 's/^android.buildTools=//p' "$app/android/toolchain.properties")"

echo "Building Claktory $version (versionCode $code) from $commit"
build() { # apk | appbundle
  (cd "$app" && ORG_GRADLE_PROJECT_claktoryKeyProperties="$key" flutter build "$1" --release \
    --flavor prod --target-platform android-arm64,android-x64 \
    --build-name "${version#v}" --build-number "$code" \
    --dart-define=FACTORY_VERSION="$version")
}
build apk
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

if ((bundle)); then
  build appbundle
  aab="$app/build/app/outputs/bundle/prodRelease/app-prod-release.aab"
  python3 "$app/android/tools/check_artifact.py" --bundletool "$bundletool" "$aab"
  # Not -strict: a self-signed key (the release key is one) with no
  # timestamp, in a bundle whose manifest isn't its first entry, is what
  # Play takes, and -strict fails all three.
  [[ "$("$JAVA_HOME/bin/jarsigner" -verify "$aab" 2>&1)" == *"jar verified."* ]] \
    || { echo "The bundle's signature doesn't verify" >&2; exit 1; }
  apk_cert="$("$build_tools/apksigner" verify --print-certs "$apk" | sed -n 's/^Signer #1 certificate SHA-256 digest: //p')"
  aab_cert="$("$JAVA_HOME/bin/keytool" -printcert -jarfile "$aab" | sed -n 's/^[[:space:]]*SHA256: //p' | head -1 | tr -d ':' | tr 'A-F' 'a-f')"
  [[ -n "$apk_cert" && "$aab_cert" == "$apk_cert" ]] \
    || { echo "The bundle isn't signed with the APK's certificate" >&2; exit 1; }
  config="$("$JAVA_HOME/bin/java" -jar "$bundletool" dump manifest --bundle="$aab")"
  [[ "$config" == *"android:versionCode=\"$code\""* && "$config" == *"android:versionName=\"${version#v}\""* ]] \
    || { echo "The bundle's manifest says otherwise" >&2; exit 1; }
  bname="claktory-${version#v}-$code.aab"
  cp "$aab" "$out/$bname"
  (cd "$out" && shasum -a 256 "$bname" > "$bname.sha256")
  echo "Built $out/$bname from $commit"
  cat "$out/$bname.sha256"
  echo "bundle signing certificate SHA-256: $aab_cert (the APK's)"
fi
