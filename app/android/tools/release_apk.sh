#!/usr/bin/env bash
# Builds Claktory's release APK for installing from a link: prod flavor,
# arm64 + x86_64, signed with the release key, stamped with Claktory's
# version, then checked (app/android/tools/check_artifact.py, apksigner)
# and copied out with its SHA-256 beside it. With --bundle, also the app
# bundle (.aab) Google Play takes, from the same build: checked the same
# way (with bundletool for its 16 KiB setting) and with jarsigner, and
# copied out beside the APK, with R8's mapping file and the native
# libraries' symbol tables, for Play's crash reports. Lint
# (tools/lint.sh) runs first over the exported source, and any error stops
# the build.
#
#   app/android/tools/release_apk.sh v0.1.15 [REBUILD] --key FILE [--keystore P12] --firebase JSON --out DIR [--bundle [--upload-key FILE]]
#
# JSON is Firebase's google-services.json with com.claktory.app's client
# (the vault's FIREBASE_PROD_JSON), for notifications through FCM
# (DESIGN.md §10, Notifications); never copied anywhere. A release without
# it would get no notifications, so it's refused unless --no-firebase says
# that's meant; the APK is checked to carry Firebase's app id.
#
# VERSION is Claktory's (vMAJOR.MINOR.PATCH), stamped into the app
# (FACTORY_VERSION, which its version checks and footer read) and its
# versionName. versionCode is MAJOR×10,000,000 + MINOR×100,000 + PATCH×100
# + REBUILD (0 by default; count it up to ship the same version again), so
# every APK installs over the last one (app/android/DESIGN.md, "Release
# signing"). FILE is the release key's properties (storeFile,
# storePassword, keyAlias, keyPassword); it's never copied anywhere. It
# signs the APK, and the bundle too unless --upload-key names another
# key's properties: Play's upload key, which then signs only the bundle
# (Play signs what it installs with the app key, our release key).
# --keystore names the release key's keystore over FILE's storeFile: the
# vault hands a command both files at fresh temporary paths, which no
# storeFile can name. Passwords are never printed: Gradle reads them from
# FILE, and keytool here from the environment.
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

usage="usage: release_apk.sh vMAJOR.MINOR.PATCH [REBUILD] --key FILE [--keystore P12] (--firebase JSON | --no-firebase) --out DIR [--bundle [--upload-key FILE]]"
version="" rebuild=0 key="" keystore="" out="" bundle=0 upload_key="" firebase="" no_firebase=0
while (($#)); do
  case "$1" in
    --key) key="$2"; shift 2 ;;
    --firebase) firebase="$2"; shift 2 ;;
    --no-firebase) no_firebase=1; shift ;;
    --keystore) keystore="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --bundle) bundle=1; shift ;;
    --upload-key) upload_key="$2"; shift 2 ;;
    v*) version="$1"; shift ;;
    [0-9]*) rebuild="$1"; shift ;;
    *) echo "$usage" >&2; exit 2 ;;
  esac
done
[[ -n "$version" && -n "$key" && -n "$out" ]] || { echo "$usage" >&2; exit 2; }
if [[ -n "$firebase" ]]; then
  ((!no_firebase)) || { echo "--firebase or --no-firebase, not both" >&2; exit 2; }
  [[ -f "$firebase" ]] || { echo "No Firebase config at $firebase" >&2; exit 2; }
  firebase="$(cd "$(dirname "$firebase")" && pwd)/$(basename "$firebase")"
else
  ((no_firebase)) || { echo "No --firebase JSON: this release would get no notifications. Give the vault's FIREBASE_PROD_JSON, or --no-firebase if that's meant." >&2; exit 2; }
fi
# arm64 and x86_64: nothing is built for x86 on an Apple-silicon Mac (Matt's
# rule), so a release is built on prime (DESIGN.md, "Where builds run").
if [[ "$(uname -s)/$(uname -m)" == Darwin/arm64 ]]; then
  echo "A release builds x86_64 too, and nothing is built for x86 on an Apple-silicon Mac: run release_apk.sh on prime (app/android/DESIGN.md, \"Where builds run\")." >&2
  exit 2
fi
[[ "$version" =~ ^v([0-9]+)\.([0-9]+)\.([0-9]+)$ ]] || { echo "Not a version: $version" >&2; exit 2; }
major=${BASH_REMATCH[1]} minor=${BASH_REMATCH[2]} patch=${BASH_REMATCH[3]}
((minor < 100 && patch < 1000 && rebuild < 100)) || { echo "$version/$rebuild doesn't fit the versionCode scheme" >&2; exit 2; }
code=$((major * 10000000 + minor * 100000 + patch * 100 + rebuild))
[[ -f "$key" ]] || { echo "No key properties at $key" >&2; exit 2; }
# The build runs in another folder: the key by its full path.
key="$(cd "$(dirname "$key")" && pwd)/$(basename "$key")"
if [[ -n "$keystore" ]]; then
  [[ -f "$keystore" ]] || { echo "No keystore at $keystore" >&2; exit 2; }
  keystore="$(cd "$(dirname "$keystore")" && pwd)/$(basename "$keystore")"
fi
if [[ -n "$upload_key" ]]; then
  ((bundle)) || { echo "--upload-key signs the bundle: it needs --bundle" >&2; exit 2; }
  [[ -f "$upload_key" ]] || { echo "No key properties at $upload_key" >&2; exit 2; }
  upload_key="$(cd "$(dirname "$upload_key")" && pwd)/$(basename "$upload_key")"
fi
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
# GNU's stat first: its -f is filesystem status, which succeeds with the
# wrong output (Linux, prime); BSD's (macOS) refuses -c and falls through.
owner_mode() { stat -c '%u %a' "$1" 2>/dev/null || stat -f '%u %Lp' "$1"; }
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
# Only what the lock files name (Fugo, #349): pub refuses a lock that would
# change, and after each build both locks must still be the commit's.
# native_toolchain_rust runs cargo without --locked, so a rewritten
# Cargo.lock is the only sign of a fresh resolution from crates.io.
(cd "$app" && flutter pub get --enforce-lockfile >/dev/null) \
  || { echo "app/pubspec.lock doesn't match pubspec.yaml: not building." >&2; exit 1; }
locks_unchanged() {
  local lock
  for lock in app/pubspec.lock app/rust/Cargo.lock; do
    cmp -s "$build_root/factory/$lock" <(git -C "$root" show "$commit:$lock") \
      || { echo "The build changed $lock: it resolved something the commit's lock doesn't name. Not shipping it." >&2; exit 1; }
  done
}
# Lint is a gate (DESIGN.md §1): all of it, over the exported source, not
# only the release build's fatal checks.
"$app/android/tools/lint.sh" || { echo "Lint found errors: not building." >&2; exit 1; }
build() { # apk | appbundle, KEY[, KEYSTORE]
  (cd "$app" && ORG_GRADLE_PROJECT_claktoryKeyProperties="$2" ORG_GRADLE_PROJECT_claktoryKeyStore="${3:-}" \
    ORG_GRADLE_PROJECT_claktoryFirebaseConfig="$firebase" \
    flutter build "$1" --release \
    --flavor prod --target-platform android-arm64,android-x64 \
    --build-name "${version#v}" --build-number "$code" \
    --dart-define=FACTORY_VERSION="$version")
}
build apk "$key" "$keystore"
locks_unchanged
apk="$app/build/app/outputs/flutter-apk/app-prod-release.apk"

python3 "$app/android/tools/check_artifact.py" "$apk"
"$build_tools/apksigner" verify "$apk"
badging="$("$build_tools/aapt2" dump badging "$apk" | sed -n 1p)"
[[ "$badging" == *"versionCode='$code'"* && "$badging" == *"versionName='${version#v}'"* ]] \
  || { echo "The APK says otherwise: $badging" >&2; exit 1; }
if [[ -n "$firebase" ]]; then
  resources="$("$build_tools/aapt2" dump resources "$apk")"
  [[ "$resources" == *string/google_app_id* ]] \
    || { echo "The APK has no Firebase app id: is com.claktory.app's client in $firebase?" >&2; exit 1; }
fi

mkdir -p "$out"
name="claktory-${version#v}-$code.apk"
cp "$apk" "$out/$name"
(cd "$out" && shasum -a 256 "$name" > "$name.sha256")
echo "Built $out/$name from $commit"
cat "$out/$name.sha256"
"$build_tools/apksigner" verify --print-certs "$out/$name" | sed -n 's/.*certificate SHA-256 digest: /signing certificate SHA-256: /p'

if ((bundle)); then
  if [[ -n "$upload_key" ]]; then build appbundle "$upload_key"; else build appbundle "$key" "$keystore"; fi
  locks_unchanged
  aab="$app/build/app/outputs/bundle/prodRelease/app-prod-release.aab"
  python3 "$app/android/tools/check_artifact.py" --bundletool "$bundletool" "$aab"
  # Not -strict: a self-signed key (the release key is one) with no
  # timestamp, in a bundle whose manifest isn't its first entry, is what
  # Play takes, and -strict fails all three.
  [[ "$("$JAVA_HOME/bin/jarsigner" -verify "$aab" 2>&1)" == *"jar verified."* ]] \
    || { echo "The bundle's signature doesn't verify" >&2; exit 1; }
  apk_cert="$("$build_tools/apksigner" verify --print-certs "$apk" | sed -n 's/^Signer #1 certificate SHA-256 digest: //p')"
  aab_certs="$("$JAVA_HOME/bin/keytool" -printcert -jarfile "$aab")"
  aab_cert="$(awk '$1 == "SHA256:" { print $2; exit }' <<<"$aab_certs" | tr -d ':' | tr 'A-F' 'a-f')"
  if [[ -n "$upload_key" ]]; then
    # Signed with the upload key's certificate, read from its keystore.
    prop() { awk -v key="$1" 'index($0, key "=") == 1 { print substr($0, length(key) + 2); exit }' "$upload_key"; }
    upload_cert="$(CLAKTORY_UPLOAD_PASS="$(prop storePassword)" "$JAVA_HOME/bin/keytool" -exportcert \
      -keystore "$(prop storeFile)" -storepass:env CLAKTORY_UPLOAD_PASS -alias "$(prop keyAlias)" \
      | shasum -a 256 | cut -d' ' -f1)"
    [[ -n "$aab_cert" && "$aab_cert" == "$upload_cert" ]] \
      || { echo "The bundle isn't signed with the upload key's certificate" >&2; exit 1; }
    [[ "$aab_cert" != "$apk_cert" ]] || { echo "The upload key is the app key" >&2; exit 1; }
  else
    [[ -n "$apk_cert" && "$aab_cert" == "$apk_cert" ]] \
      || { echo "The bundle isn't signed with the APK's certificate" >&2; exit 1; }
  fi
  # What Play reads for its crash reports: R8's mapping and the native
  # libraries' symbols, in the bundle and beside it for the Console.
  meta="$(unzip -Z1 "$aab")"
  [[ "$meta" == *"BUNDLE-METADATA/com.android.tools.build.obfuscation/proguard.map"* ]] \
    || { echo "The bundle has no R8 mapping" >&2; exit 1; }
  [[ "$meta" == *"BUNDLE-METADATA/com.android.tools.build.debugsymbols/arm64-v8a/libclaktory_tunnel.so.sym"* ]] \
    || { echo "The bundle has no symbols for the tunnel's library" >&2; exit 1; }
  symbols="$app/build/app/outputs/native-debug-symbols/prodRelease/native-debug-symbols.zip"
  mapping="$app/build/app/outputs/mapping/prodRelease/mapping.txt"
  [[ -s "$symbols" && -s "$mapping" ]] || { echo "No mapping file or native symbols beside the bundle" >&2; exit 1; }
  config="$("$JAVA_HOME/bin/java" -jar "$bundletool" dump manifest --bundle="$aab")"
  [[ "$config" == *"android:versionCode=\"$code\""* && "$config" == *"android:versionName=\"${version#v}\""* ]] \
    || { echo "The bundle's manifest says otherwise" >&2; exit 1; }
  bname="claktory-${version#v}-$code.aab"
  cp "$aab" "$out/$bname"
  cp "$mapping" "$out/claktory-${version#v}-$code-mapping.txt"
  cp "$symbols" "$out/claktory-${version#v}-$code-native-debug-symbols.zip"
  (cd "$out" && shasum -a 256 "$bname" > "$bname.sha256")
  echo "Built $out/$bname from $commit, with its mapping file and native symbols"
  cat "$out/$bname.sha256"
  if [[ -n "$upload_key" ]]; then
    echo "bundle signing certificate SHA-256: $aab_cert (the upload key's)"
  else
    echo "bundle signing certificate SHA-256: $aab_cert (the APK's)"
  fi
fi
