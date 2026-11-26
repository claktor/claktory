# Shared by the build scripts. Source it; don't run it.

# Versions are v0.1.0, v0.1.1, v0.1.2…: each release counts up the last
# number; the middle one goes up only when the app and the backend of
# different versions can't work together (scripts/changelog.sh stamp
# --incompatible). Before v0.1.0 they were date stamps (2026.11.19-1958),
# which every numbered version is newer than. A dev build is the next
# version's, with when it was built: v0.1.3-dev.20261120-1012.30.

# A key that sorts versions in release order as text (crates/factoryd's
# update::compare_versions orders them the same way): nothing, then date
# stamps, then numbered versions by their numbers, a dev build before the
# release it leads to, then "dev" (an unstamped build).
version_key() {
  local v="$1"
  if [[ -z "$v" ]]; then
    echo 0
  elif [[ "$v" == dev ]]; then
    echo 3
  elif [[ "$v" =~ ^v([0-9]+)\.([0-9]+)\.([0-9]+)(-(.*))?$ ]]; then
    local release=1
    [[ -n "${BASH_REMATCH[4]}" ]] && release=0
    printf '2 %012d %012d %012d %s %s\n' "$((10#${BASH_REMATCH[1]}))" "$((10#${BASH_REMATCH[2]}))" "$((10#${BASH_REMATCH[3]}))" "$release" "${BASH_REMATCH[5]}"
  else
    echo "1 $v"
  fi
}

# Whether version $1 is newer than version $2.
version_newer() {
  [[ "$(version_key "$1")" > "$(version_key "$2")" ]]
}

# The newest numbered version heading in docs/CHANGELOG.md, or nothing.
newest_numbered() {
  local root
  root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
  awk '/^## v[0-9]+\.[0-9]+\.[0-9]+$/{print substr($0, 4); exit}' "$root/docs/CHANGELOG.md"
}

# The version after the newest numbered one: v0.1.0 when there's none yet;
# with "incompatible", the next middle number's first (v0.1.7 -> v0.2.0).
next_version() {
  local newest major minor patch
  newest="$(newest_numbered)"
  if [[ -z "$newest" ]]; then
    echo v0.1.0
    return
  fi
  IFS=. read -r major minor patch <<<"${newest#v}"
  if [[ "${1:-}" == incompatible ]]; then
    echo "v$major.$((minor + 1)).0"
  else
    echo "v$major.$minor.$((patch + 1))"
  fi
}

# A version for a dev build made now: the next release's, with when.
stamp_version() {
  echo "$(next_version)-dev.$(date -u +%Y%m%d-%H%M.%S)"
}

# A fingerprint of everything the backend is built from. Equal fingerprints
# mean the backend didn't change, so a release can reuse it. That includes the
# release channel stamped into it (FACTORY_UPDATE_URL, see scripts/publish.sh).
backend_fingerprint() {
  local root
  root="${SRC_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
  (
    cd "$root"
    # Built with this Mac's home folder remapped (release_rustflags): a
    # backend from before that carries the home path, so it's never reused.
    echo "build: home remapped (Rust, C, C++, Objective-C)"
    find crates proto -type f ! -name .DS_Store -print0 | sort -z | xargs -0 shasum -a 256
    shasum -a 256 Cargo.toml Cargo.lock docs/MANUAL.md
    # Patched crates the backend builds from ([patch.crates-io] in Cargo.toml).
    # Left out when absent, so older commits fingerprint as they always did.
    if [[ -d vendor ]]; then
      find vendor -type f ! -name .DS_Store -print0 | sort -z | xargs -0 shasum -a 256
    fi
    # ripgrep ships with the backend (tools/rg, scripts/build-rg.sh's pin).
    # Left out when absent, so older commits fingerprint as they always did.
    if [[ -f scripts/build-rg.sh ]]; then
      echo "ripgrep: $(sed -n 's/^RG_VERSION=//p; s/^RG_CRATE_SHA256=//p' scripts/build-rg.sh | tr '\n' ' ')"
    fi
    # And jq (tools/jq, scripts/build-jq.sh's pin), the same way.
    if [[ -f scripts/build-jq.sh ]]; then
      echo "jq: $(sed -n 's/^JQ_VERSION=//p; s/^JQ_TARBALL_SHA256=//p' scripts/build-jq.sh | tr '\n' ' ')"
    fi
    # Built from nothing an earlier build left, with the checked toolchain
    # (release_inputs): a backend from before that is never reused.
    if [[ -f scripts/check-toolchain.sh ]]; then
      echo "build: fresh inputs; rust $(sed -n 's/^RUST_VERSION=//p; s/^RUST_CHANNEL_SHA256=//p' scripts/check-toolchain.sh | tr '\n' ' ')"
    fi
    # The video preview helper ships with the backend (build_video_preview).
    if [[ -d app/macos/VideoPreview ]]; then
      find app/macos/VideoPreview -type f ! -name .DS_Store -print0 | sort -z | xargs -0 shasum -a 256
    fi
    if [[ -n "${FACTORY_UPDATE_URL:-}" ]]; then
      echo "update url: $FACTORY_UPDATE_URL"
    fi
  ) | shasum -a 256 | cut -d' ' -f1
}

# Moves a freshly built Factory.app out of Flutter's build folder. Left there,
# it carries the real app's name and bundle id, and macOS may open it when
# someone launches "Factory".
take_build() {
  local built="$1" destination="$2"
  rm -rf "$destination"
  mv "$built" "$destination"
  /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -u "$built" 2>/dev/null || true
}

# Turns a built Factory.app into Factory Dev (shown as Claktory Dev): its own
# bundle id, name and icon, so it runs next to the real Factory.
make_dev_app() {
  local app="$1" entitlements="$2" root
  root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
  local plist="$app/Contents/Info.plist"
  /usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier dev.factory.app.dev" -c "Set :CFBundleName Claktory Dev" "$plist"
  # Its own link scheme (claktory-dev://add?code=…), so claktory:// links
  # always open the real Claktory.
  /usr/libexec/PlistBuddy -c "Set :CFBundleURLTypes:0:CFBundleURLSchemes:0 claktory-dev" "$plist" 2>/dev/null || true
  # The name shown is Claktory Dev; the display name in Info.plist matches the
  # bundle's file name, Factory Dev.app, as macOS needs to show the localized one.
  /usr/libexec/PlistBuddy -c "Delete :CFBundleDisplayName" "$plist" 2>/dev/null || true
  /usr/libexec/PlistBuddy -c "Add :CFBundleDisplayName string Factory Dev" "$plist"
  mkdir -p "$app/Contents/Resources/en.lproj"
  printf 'CFBundleName = "Claktory Dev";\nCFBundleDisplayName = "Claktory Dev";\n' >"$app/Contents/Resources/en.lproj/InfoPlist.strings"
  # The dev icon (scripts/icons.swift). Without CFBundleIconName, macOS takes the
  # icon from AppIcon.icns instead of the asset catalog.
  cp "$root/app/macos/dev/AppIcon.icns" "$app/Contents/Resources/AppIcon.icns"
  /usr/libexec/PlistBuddy -c "Delete :CFBundleIconName" "$plist" 2>/dev/null || true
  codesign --force --sign - --entitlements "$root/app/macos/Runner/$entitlements" "$app"
}

# Signing releases for the real Claktory (scripts/sign-release.sh).

# The "Developer ID Application" identity in the keychain to sign with
# (CLAKTORY_SIGN_IDENTITY names one; with CLAKTORY_TEAM_ID, only that team's),
# or nothing.
developer_id_identity() {
  if [[ -n "${CLAKTORY_SIGN_IDENTITY:-}" ]]; then
    echo "$CLAKTORY_SIGN_IDENTITY"
    return
  fi
  security find-identity -v -p codesigning 2>/dev/null |
    sed -n 's/^ *[0-9][0-9]*) [0-9A-F]\{40\} "\(Developer ID Application: .*\)"$/\1/p' |
    { if [[ -n "${CLAKTORY_TEAM_ID:-}" ]]; then grep -F "($CLAKTORY_TEAM_ID)"; else cat; fi; } |
    head -1 || true
}

# Loads the App Store Connect API key's settings, as scripts/testflight.sh
# does: ASC_KEY_ID, ASC_ISSUER_ID and ASC_KEY_PATH (by default
# ~/.appstoreconnect/private_keys/AuthKey_<ASC_KEY_ID>.p8), from the
# environment or ~/.appstoreconnect/claktory.env; CLAKTORY_TEAM_ID too.
# Succeeds when the key is complete and its file is there. Prints nothing.
notary_key() {
  local settings="$HOME/.appstoreconnect/claktory.env" key value
  if [[ -f "$settings" ]]; then
    while IFS='=' read -r key value; do
      case "$key" in
        CLAKTORY_TEAM_ID | ASC_KEY_ID | ASC_ISSUER_ID | ASC_KEY_PATH)
          if [[ -z "${!key:-}" ]]; then
            value="${value%\"}"; value="${value#\"}"
            export "$key=$value"
          fi
          ;;
      esac
    done <"$settings"
  fi
  ASC_KEY_PATH="${ASC_KEY_PATH:-$HOME/.appstoreconnect/private_keys/AuthKey_${ASC_KEY_ID:-KEYID}.p8}"
  [[ -n "${ASC_KEY_ID:-}" && -n "${ASC_ISSUER_ID:-}" && -f "$ASC_KEY_PATH" ]]
}

# How a release for the real Claktory is signed now: "developer-id <identity>"
# when this Mac has a Developer ID identity and the notarization key, else
# "adhoc".
signing_mode() {
  local identity
  identity="$(developer_id_identity)"
  if [[ -n "$identity" ]] && notary_key; then
    echo "developer-id $identity"
  else
    echo adhoc
  fi
}

# The supervisor of the real Claktory as an app of its own,
# <release>/supervisor/Claktory Agents.app, made from the release's factoryd:
# macOS counts it as responsible for everything agents run, and only an app
# can be added under Privacy & Security → Developer Tools, which spares the
# commands agents build macOS's checks of each new program. Installed once
# (scripts/install-release.sh), never by an update. Signed ad hoc here;
# scripts/sign-release.sh signs it with the Developer ID when it can.
# The reason macOS shows when the backend first asks for Local Network access
# (macOS 15 and later), which devices on the same network need to reach it
# directly (#543, Trish's words). Plain text: no & or <, it goes into XML.
SUPERVISOR_LOCAL_NETWORK_REASON="Lets your phone, iPad and other computers on this network connect to Claktory directly, which is faster."
make_supervisor_app() {
  local release="$1" version
  local app="$release/supervisor/Claktory Agents.app"
  # Info.plist versions are numbers only: v0.1.3 is 0.1.3.
  version="$(sed -n 's/^v\([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\).*/\1/p' <<<"${FACTORY_VERSION:-}")"
  rm -rf "$release/supervisor"
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
  cp "$release/factoryd" "$app/Contents/MacOS/factoryd-supervisor"
  chmod 755 "$app/Contents/MacOS/factoryd-supervisor"
  if [[ -f "$release/Factory.app/Contents/Resources/AppIcon.icns" ]]; then
    cp "$release/Factory.app/Contents/Resources/AppIcon.icns" "$app/Contents/Resources/AppIcon.icns"
  fi
  cat >"$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>dev.factory.supervisor</string>
  <key>CFBundleName</key><string>Claktory Agents</string>
  <key>CFBundleDisplayName</key><string>Claktory Agents</string>
  <key>CFBundleExecutable</key><string>factoryd-supervisor</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundleShortVersionString</key><string>${version:-0.0.0}</string>
  <key>CFBundleVersion</key><string>${version:-0.0.0}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSBackgroundOnly</key><true/>
  <key>NSLocalNetworkUsageDescription</key><string>${SUPERVISOR_LOCAL_NETWORK_REASON}</string>
</dict>
</plist>
PLIST
  plutil -lint -s "$app/Contents/Info.plist"
  codesign --force --sign - "$app"
}

# Claktory's own apps, shipped in every release (apps/dist/<id>.clapp in the
# source, signed with the apps publisher key): checked, then copied into
# DEST/apps. factoryd offers what it finds there (beside claktory-apps, or in
# the running version's release folder) in Settings -> Apps. Each must be
# signed by a key pinned as Claktory's (CLAKTORY_KEYS in bundle.rs), with a
# signature that holds, and carry no trace of this Mac (check_no_traces).
# A backend built with QA's provider override (cargo feature qa-provider,
# de042953) could send the person's Claude token to another address: it must
# never ship (Fugo). Refuses any of the FILES that carries its marks.
check_no_qa_provider() { # FILE...
  local f
  for f in "$@"; do
    [[ -f "$f" ]] || { echo "expected $f to check for QA's provider override, and it isn't there; this release stops here" >&2; return 1; }
    if grep -aq -e FACTORY_QA_ANTHROPIC_URL -e "QA provider override" "$f"; then
      echo "$f was built with QA's provider override, which could send the person's Claude sign-in elsewhere; it must never ship, so this release stops here. Build it without the qa-provider feature." >&2
      return 1
    fi
  done
}

bundled_apps() { # SRC DEST
  local src="$1" dest="$2" clapp work public pinned
  rm -rf "$dest/apps"
  compgen -G "$src/apps/dist/*.clapp" >/dev/null || return 0
  mkdir -p "$dest/apps"
  pinned="$(grep -A20 '^const CLAKTORY_KEYS' "$src/crates/factoryd/src/apps/bundle.rs" | sed '/\];/q')"
  for clapp in "$src"/apps/dist/*.clapp; do
    work="$(mktemp -d)"
    tar -xf "$clapp" -C "$work" manifest.toml app.wasm signature 2>/dev/null || { rm -rf "$work"; echo "$(basename "$clapp") isn't a .clapp" >&2; return 1; }
    public="$(sed -n 's/^key ed25519://p' "$work/signature")"
    grep -qF "\"$public\"" <<<"$pinned" || { rm -rf "$work"; echo "$(basename "$clapp") isn't signed by Claktory's apps key (CLAKTORY_KEYS in bundle.rs)" >&2; return 1; }
    ( cd "$work"
      { printf 'claktory-app-v1\n'; shasum -a 256 app.wasm manifest.toml; } >message
      sed -n 's/^sig //p' signature | base64 -d >sig.bin
      { printf '302a300506032b6570032100' | xxd -r -p; printf '%s' "$public" | base64 -d; } >pub.der
      openssl pkey -pubin -inform DER -in pub.der -out pub.pem 2>/dev/null &&
        openssl pkeyutl -verify -pubin -inkey pub.pem -rawin -in message -sigfile sig.bin >/dev/null 2>&1
    ) || { rm -rf "$work"; echo "$(basename "$clapp")'s signature doesn't hold" >&2; return 1; }
    rm -rf "$work"
    cp "$clapp" "$dest/apps/"
  done
  # Raw bytes, as for the binaries: a path in a wasm's panic strings is a trace.
  local needle found=""
  while IFS= read -r needle; do
    [[ -n "$needle" ]] || continue
    for clapp in "$dest"/apps/*.clapp; do
      LC_ALL=C grep -a -q -F "$needle" "$clapp" && found="$found $(basename "$clapp")"
    done
  done < <(traces)
  [[ -z "$found" ]] || { echo "these apps carry this Mac's home folder, user or host name; not shipping them:$found (build the wasm with --remap-path-prefix=\$HOME=~)" >&2; return 1; }
  check_tar_headers "$dest"/apps/*.clapp || return 1
  echo "bundled apps: $(cd "$dest/apps" && ls | tr '\n' ' ')"
}

# Puts the helpers in the app (Contents/Helpers): the backend (factoryd),
# the apps host (claktory-apps), the speech host (claktory-speech, which
# writes what voice messages say) and the supervisor's app, as the release has
# them, so the app can install Claktory on a Mac by itself (`factoryd
# install`, from a .dmg) or repair it. scripts/sign-release.sh signs them in
# the app.
add_helpers() {
  local release="$1" app="$1/$2"
  local helpers="$app/Contents/Helpers"
  rm -rf "$helpers"
  mkdir -p "$helpers"
  cp "$release/factoryd" "$helpers/factoryd"
  if [[ -f "$release/claktory-apps" ]]; then cp "$release/claktory-apps" "$helpers/claktory-apps"; fi
  if [[ -f "$release/claktory-speech" ]]; then cp "$release/claktory-speech" "$helpers/claktory-speech"; fi
  if [[ -f "$release/claktory-video-preview" ]]; then cp "$release/claktory-video-preview" "$helpers/claktory-video-preview"; fi
  # Agents' tools (ripgrep, jq): the programs in Helpers/tools, signed there
  # by scripts/sign-release.sh; their licenses in Resources/tools, since
  # codesign refuses anything unsigned in Helpers.
  rm -rf "$app/Contents/Resources/tools"
  if [[ -d "$release/tools" ]]; then
    local tool
    mkdir -p "$helpers/tools"
    for tool in rg jq; do
      if [[ -f "$release/tools/$tool" ]]; then cp "$release/tools/$tool" "$helpers/tools/$tool"; fi
    done
    mkdir -p "$app/Contents/Resources/tools"
    cp -R "$release/tools/LICENSES" "$app/Contents/Resources/tools/LICENSES"
  fi
  # Claktory's own apps are data, not code: in Resources, since codesign
  # refuses anything unsigned in Helpers ("code object is not signed at all").
  rm -rf "$app/Contents/Resources/apps"
  if [[ -d "$release/apps" ]]; then cp -R "$release/apps" "$app/Contents/Resources/apps"; fi
  if [[ -d "$release/supervisor/Claktory Agents.app" ]]; then
    ditto "$release/supervisor/Claktory Agents.app" "$helpers/Claktory Agents.app"
  fi
}

# The video preview helper (#379): app/macos/VideoPreview/main.swift
# (AVFoundation only), universal for macOS 12, into OUT. factoryd runs it
# from its own folder to make a phone-sized copy of a video; it ships with
# the backend, so backend_fingerprint covers its source. Signed ad hoc here
# under its fixed identifier; scripts/sign-release.sh signs it for real.
#   build_video_preview OUT
build_video_preview() {
  local out="$1" root src work arch
  root="${SRC_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
  src="$root/app/macos/VideoPreview/main.swift"
  [[ -f "$src" ]] || return 0
  work="$(mktemp -d)"
  for arch in arm64 x86_64; do
    xcrun swiftc -target "$arch-apple-macos12" -O -o "$work/$arch" "$src" || { rm -rf "$work"; return 1; }
  done
  lipo -create -output "$out" "$work/arm64" "$work/x86_64"
  rm -rf "$work"
  codesign --force --sign - --identifier dev.factory.claktory-video-preview "$out"
}

# Release builds carry no trace of this Mac (Fugo, 2026-11-22): its user name,
# home folder, host name and folder layout would go public in every binary.
# Rust's panic locations name the registry and toolchain under $HOME, so
# release cargo builds remap it; Dart's AOT snapshot names the generated
# plugin registrant's absolute path (dart-lang/sdk#55282), so the app is
# built from an export of the commit in one fixed, private folder; and
# check_no_traces refuses whatever still carries one.

# Flags for a release build for the Mac, exported for cargo: $HOME as ~.
# Only the Mac target's, so the tunnel's own per-target flags (app/hook)
# aren't overridden. The C, C++ and Objective-C that crates build (ggml in
# claktory-speech, through cc and CMake) name their sources in __FILE__ for
# assertions, so those compilers map $HOME too: CFLAGS_<target> reach cc and
# CMake's C and C++ flags; CMake's Objective-C flags come only from a define
# (TRANSCRIBE_CMAKE_ARGS, transcribe-cpp-sys's pass-through to CMake).
release_rustflags() {
  local triple map="-ffile-prefix-map=$HOME=~"
  export CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS="--remap-path-prefix=$HOME=~"
  export CARGO_TARGET_X86_64_APPLE_DARWIN_RUSTFLAGS="--remap-path-prefix=$HOME=~"
  for triple in aarch64_apple_darwin x86_64_apple_darwin; do
    export "CFLAGS_$triple=$map" "CXXFLAGS_$triple=$map"
  done
  export TRANSCRIBE_CMAKE_ARGS="-DCMAKE_OBJC_FLAGS=$map -DCMAKE_OBJCXX_FLAGS=$map"
}

# DIR, made private (0700) if it's absent, and refused unless it's a folder
# of ours, mode 700 and not a link: /tmp is everyone's, and a folder someone
# else made first could have its contents swapped mid-build (Fugo).
private_dir() {
  local dir="$1" owner mode
  if [[ ! -e "$dir" && ! -L "$dir" ]]; then
    (umask 077 && mkdir -m 700 "$dir")
  fi
  [[ -d "$dir" && ! -L "$dir" ]] || { echo "$dir isn't a plain folder; not building there" >&2; return 1; }
  if [[ "$(uname)" == Darwin ]]; then
    owner="$(stat -f %u "$dir")" mode="$(stat -f %Lp "$dir")"
  else
    owner="$(stat -c %u "$dir")" mode="$(stat -c %a "$dir")"
  fi
  [[ "$owner" == "$(id -u)" && "$mode" == 700 ]] || {
    echo "$dir isn't this user's private folder (owner $owner, mode $mode); not building there. Remove it and try again." >&2
    return 1
  }
}

# The fixed folder release builds run in: COMMIT's files exported from the
# checkout ORIGIN into /tmp/claktory-release/src (the same path every time,
# so the binaries name only it), with an empty target/: a release builds
# from nothing an earlier build left. Prints the folder. One build at a time:
# release_unlock lets go.
RELEASE_BUILD=/tmp/claktory-release
release_tree() {
  local origin="$1" commit="$2" src="$RELEASE_BUILD/src" holder
  private_dir "$RELEASE_BUILD" || return 1
  if ! (umask 077 && mkdir "$RELEASE_BUILD/lock" 2>/dev/null); then
    holder="$(cat "$RELEASE_BUILD/lock/pid" 2>/dev/null || true)"
    if [[ -n "$holder" ]] && kill -0 "$holder" 2>/dev/null; then
      echo "another release build (pid $holder) is using $RELEASE_BUILD; try again when it's done" >&2
      return 1
    fi
    rm -rf "$RELEASE_BUILD/lock"
    (umask 077 && mkdir "$RELEASE_BUILD/lock") || return 1
  fi
  echo "$$" >"$RELEASE_BUILD/lock/pid" || return 1
  # Run in $(...), where errexit is off: every step says when it failed.
  rm -rf "$src" || return 1
  private_dir "$src" || return 1
  git -C "$origin" archive --format=tar "$commit" | tar -x -m -C "$src" || return 1
  (umask 077 && mkdir -m 700 "$src/target") || return 1
  echo "$src"
}
# What a release build uses besides its source, all fresh every time, in
# the same private folder (call it after release_tree, outside $(...)):
# cargo's home, so every crate is downloaded again and checked against
# Cargo.lock; pub's package cache (the plugins' native sources are compiled
# from it, and their paths end up in the app's debug map), filled only as
# pubspec.lock says (release_pub_get); the Rust toolchain, checked against
# what Rust publishes (scripts/check-toolchain.sh) and used for every Rust
# build, the app's included; the Flutter SDK, a clean checkout of the
# pinned release; no cargo configuration but the source's own
# (cargo_parents_ok; cargo runs in the source folder); and Linux builds in a
# container with nothing kept (scripts/build-linux.sh, FACTORY_LINUX_FRESH).
# Cargo reads a .cargo/config.toml in the folder it runs in and in every
# folder above it. Above a build folder in /tmp, only the build's own may
# hold one: a .cargo beside it in a folder anyone can write to (/tmp) is
# claimed, ours, empty and read-only to others; elsewhere above it one may
# exist only if it's root's and only root can change it. Refuses otherwise.
cargo_parents_ok() {
  local dir mode owner stat_mode stat_owner
  dir="$(cd "$1" && pwd -P)" || return 1
  if [[ "$(uname)" == Darwin ]]; then stat_mode=(stat -f %Lp) stat_owner=(stat -f %u); else stat_mode=(stat -c %a) stat_owner=(stat -c %u); fi
  while [[ "$dir" != / ]]; do
    dir="$(dirname "$dir")"
    mode="$("${stat_mode[@]}" "$dir")"
    if (( 8#$mode & 8#002 )); then
      [[ -e "$dir/.cargo" || -L "$dir/.cargo" ]] || mkdir -m 755 "$dir/.cargo" 2>/dev/null || true
      owner="$("${stat_owner[@]}" "$dir/.cargo" 2>/dev/null || true)"
      if [[ -L "$dir/.cargo" || ! -d "$dir/.cargo" || "$owner" != "$(id -u)" || "$("${stat_mode[@]}" "$dir/.cargo")" != 755 || -n "$(ls -A "$dir/.cargo")" ]]; then
        echo "$dir/.cargo isn't an empty folder of ours that only we can write to: cargo would read it; not building" >&2
        return 1
      fi
    elif [[ -e "$dir/.cargo" || -L "$dir/.cargo" ]]; then
      owner="$("${stat_owner[@]}" "$dir/.cargo")" mode="$("${stat_mode[@]}" "$dir/.cargo")"
      if [[ -L "$dir/.cargo" || "$owner" != 0 ]] || (( 8#$mode & 8#022 )); then
        echo "$dir/.cargo is there and isn't root's alone: cargo would read it; not building" >&2
        return 1
      fi
    fi
  done
}
FLUTTER_VERSION=3.50.0
FLUTTER_REVISION=ab2912035e160d9cad6306df307576da6128ebdb
release_inputs() {
  local dir tc flutter
  for dir in cargo-home pub-cache; do
    rm -rf "${RELEASE_BUILD:?}/$dir" || return 1
    private_dir "$RELEASE_BUILD/$dir" || return 1
  done
  export CARGO_HOME="$RELEASE_BUILD/cargo-home" PUB_CACHE="$RELEASE_BUILD/pub-cache"
  cargo_parents_ok "$RELEASE_BUILD/src" || return 1
  tc="$("$(dirname "${BASH_SOURCE[0]}")/check-toolchain.sh")" || return 1
  export RUSTUP_TOOLCHAIN="$tc"
  export PATH="${RUSTUP_HOME:-$HOME/.rustup}/toolchains/$tc/bin:$PATH"
  flutter="$(command -v flutter)" || { echo "no flutter on PATH" >&2; return 1; }
  flutter="$(cd "$(dirname "$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$flutter")")/.." && pwd -P)"
  [[ "$(git -C "$flutter" rev-parse HEAD 2>/dev/null)" == "$FLUTTER_REVISION" ]] || {
    echo "Flutter in $flutter isn't $FLUTTER_VERSION ($FLUTTER_REVISION): not building" >&2; return 1; }
  [[ -z "$(git -C "$flutter" status --porcelain --ignored=no 2>/dev/null)" ]] || {
    echo "Flutter in $flutter has changes of its own: not building" >&2; return 1; }
  export FACTORY_LINUX_FRESH=1
}
# Pub's packages for the app in DIR, exactly as its pubspec.lock lists them
# (their hashes checked), before flutter builds it.
release_pub_get() {
  (cd "$1" && flutter pub get --enforce-lockfile)
}
release_unlock() {
  [[ "$(cat "$RELEASE_BUILD/lock/pid" 2>/dev/null)" == "$$" ]] && rm -rf "$RELEASE_BUILD/lock"
  return 0
}

# What this Mac would give away in a binary: its home folder, user name,
# host names and agents' checkouts.
traces() {
  local name
  printf '%s\n' "$HOME" "/Users/$USER" "/home/$USER" ".factory/trees"
  for name in "$(hostname -s 2>/dev/null)" "$(scutil --get ComputerName 2>/dev/null)" "$(scutil --get LocalHostName 2>/dev/null)"; do
    [[ ${#name} -ge 4 ]] && printf '%s\n' "$name"
  done
  return 0
}

# Refuses (exit 1, naming each) unless every header in the given tar files
# (.tar, .tar.gz, .clapp) and in each .clapp inside them is neutral: uid and
# gid 0, no user or group name, and no name of this Mac (its user, home or
# host names) in a member's path or link (Fugo, v0.1.19's Charts). gitleaks
# can't see inside an archive, and a header's owner is this Mac's user name.
check_tar_headers() {
  local user="${USER:-$(id -un)}"
  python3 - "$user" "$(traces | tr '\n' '\t')" "$@" <<'PYCHECK'
import io, sys, tarfile
user, needles, files = sys.argv[1], [n for n in sys.argv[2].split("\t") if n] + [sys.argv[1]], sys.argv[3:]
bad = []
def check(label, fileobj=None, name=None):
    with tarfile.open(name=name, fileobj=fileobj, mode="r:*") as t:
        for m in t:
            where = f"{label}: {m.name}"
            if m.uid or m.gid or m.uname or m.gname:
                bad.append(f"{where} has owner {m.uid}:{m.gid} ({m.uname or '-'}:{m.gname or '-'})")
            for n in needles:
                if n in m.name or n in (m.linkname or ""):
                    bad.append(f"{where} names this Mac ({'its user' if n == user else 'a home or host name'})")
            if m.isfile() and m.name.endswith(".clapp"):
                check(where, fileobj=io.BytesIO(t.extractfile(m).read()))
for f in files:
    check(f, name=f)
if bad:
    print("these archives carry this Mac's owner or names; not shipping them (pack with --uid 0 --gid 0 --uname '' --gname ''):", file=sys.stderr)
    for b in bad[:20]:
        print("  " + b, file=sys.stderr)
    if len(bad) > 20:
        print(f"  … and {len(bad) - 20} more", file=sys.stderr)
    sys.exit(1)
PYCHECK
}

# Refuses (exit 1, naming each file and what it holds) unless no Mach-O or
# ELF file under the given paths carries one of this Mac's traces.
check_no_traces() {
  local patterns found="" file hits
  patterns="$(mktemp)"
  traces | sort -u >"$patterns"
  while IFS= read -r -d '' file; do
    case "$(file -b "$file")" in *Mach-O* | *ELF*) ;; *) continue ;; esac
    # Raw bytes, not strings(1): Apple's reads a Mach-O's sections only,
    # not its symbol table, where debug-map entries name source paths (Fugo).
    hits="$(LC_ALL=C grep -a -F -c -f "$patterns" "$file" || true)"
    [[ "$hits" == 0 ]] || found="$found
  $file: $hits ($(LC_ALL=C grep -a -F -o -f "$patterns" "$file" | sort | uniq -c | sort -rn | head -3 | awk '{print $2" ×"$1}' | sed "s|$HOME|\$HOME|g" | paste -sd, -))"
  done < <(find "$@" -type f -print0)
  rm -f "$patterns"
  if [[ -n "$found" ]]; then
    echo "these binaries carry this Mac's home folder, user or host name; not shipping them:$found" >&2
    return 1
  fi
}

# Claktory's browser helper (crates/claktory-browser-host, browser-control.md
# §6), the native-messaging host Chrome starts for Claktory's extension, put
# in APP's Contents/Helpers. FLAVOR real or dev: each is its own build (the
# dev feature fixes Claktory Dev's host name, socket, extension and app into
# it), and PROFILE release or debug. A Dev helper is signed ad hoc here, with
# its fixed identifier, before the app is sealed around it; a real one is
# signed with the Developer ID by scripts/sign-release.sh, and ad hoc only
# when the release is (a real helper refuses to run ad hoc anyway).
add_browser_host() {
  local app="$1" flavor="$2" profile="$3" root features=() build=()
  root="${SRC_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
  [[ "$flavor" == dev ]] && features=(--features dev)
  [[ "$profile" == release ]] && build=(--release)
  [[ "$profile" == release ]] && release_rustflags
  cargo build --manifest-path "$root/Cargo.toml" ${build[@]+"${build[@]}"} -p claktory-browser-host ${features[@]+"${features[@]}"}
  mkdir -p "$app/Contents/Helpers"
  cp "$root/target/$profile/claktory-browser-host" "$app/Contents/Helpers/claktory-browser-host"
  if [[ "$flavor" == dev ]]; then
    codesign --force --sign - --identifier com.claktory.browser-host.dev "$app/Contents/Helpers/claktory-browser-host"
  fi
}

# The public repository holds one commit per release: its files are exactly
# that release's source, its parent is the previous release's public commit,
# and its message is what the release brings, from the changelog. The
# day-to-day commits stay on this Mac (and its private backup).
#   public_commit REPO SOURCE VERSION [PARENT]
# makes that commit in the git repository REPO (without moving any branch)
# and prints its id: SOURCE's public files (public_tree), VERSION's changelog section from
# SOURCE's docs/CHANGELOG.md (and those of versions between PARENT's and it
# that weren't published on their own), dated when it was released, by
# Claktory. The same arguments always make the same commit, so a publish
# that's tried again makes the one it made before.
PUBLIC_NAME="Claktory"
PUBLIC_EMAIL="releases@claktory.com"
# What goes public of a release's source: only what people who install
# Claktory need, these top-level folders and files. Anything else there
# (agents' working notes and drafts, such as coordination/) stays here.
PUBLIC_TOP=(.factoryignore .gitignore .gitleaks.toml AGENTS.md CLAUDE.md Cargo.lock Cargo.toml LICENSE.md README.md
  TENETS.md rustfmt.toml app apps browser-extension crates docs proto scripts vendor)
#   public_tree REPO SOURCE
# prints the tree of SOURCE's files that go public (PUBLIC_TOP), and names
# on stderr what it left out.
public_tree() {
  local repo="$1" source="$2" entries kept left name line
  entries="$(git -C "$repo" ls-tree "$source^{tree}")" || return 1
  kept="" left=""
  while IFS= read -r line; do
    name="${line#*$'\t'}"
    if [[ " ${PUBLIC_TOP[*]} " == *" $name "* ]]; then kept+="$line"$'\n'; else left+=" $name"; fi
  done <<<"$entries"
  [[ -z "$left" ]] || echo "left out of the public source (not in PUBLIC_TOP, scripts/lib.sh):$left" >&2
  printf '%s' "$kept" | git -C "$repo" mktree
}
public_commit() {
  local repo="$1" source="$2" version="$3" parent="${4:-}"
  local changelog previous="" sections released date tree
  changelog="$(git -C "$repo" show "$source:docs/CHANGELOG.md")" || return 1
  tree="$(public_tree "$repo" "$source")" || return 1
  # The newest version the parent has: its sections are already public.
  [[ -z "$parent" ]] ||
    previous="$(git -C "$repo" show "$parent:docs/CHANGELOG.md" 2>/dev/null | sed -n 's/^## \(v[0-9][0-9.]*\)$/\1/p' | head -1)"
  sections="$(awk -v version="$version" -v previous="$previous" '
    /^## / { name = substr($0, 4)
             if (name == version) { on = 1; count = 0 }
             else if (on && (name == previous || previous == "" || name !~ /^v[0-9]/)) exit
             if (on) { count++; print; next } }
    on' <<<"$changelog")"
  [[ -n "$sections" ]] || { echo "$source's docs/CHANGELOG.md has no section for $version" >&2; return 1; }
  # One version: its lines. Several (versions never published on their own
  # come with the next): each under its heading.
  if [[ "$(grep -c '^## ' <<<"$sections")" == 1 ]]; then
    sections="$(sed '1d' <<<"$sections")"
  fi
  sections="$(sed -e '/./,$!d' <<<"$sections" | sed -e ':a' -e '/^\n*$/{$d;N;ba' -e '}')"
  released="$(sed -n 's/^Released \([0-9-]*\) \([0-9:]*\) UTC\.$/\1T\2:00+0000/p' <<<"$sections" | head -1)"
  date="${released:-$(git -C "$repo" log -1 --format=%cI "$source")}"
  printf 'Claktory %s\n\n%s\n' "$version" "$sections" |
    GIT_AUTHOR_NAME="$PUBLIC_NAME" GIT_AUTHOR_EMAIL="$PUBLIC_EMAIL" GIT_AUTHOR_DATE="$date" \
    GIT_COMMITTER_NAME="$PUBLIC_NAME" GIT_COMMITTER_EMAIL="$PUBLIC_EMAIL" GIT_COMMITTER_DATE="$date" \
    git -C "$repo" -c commit.gpgsign=false commit-tree "$tree" ${parent:+-p "$parent"}
}
