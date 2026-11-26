#!/usr/bin/env bash
# Publishes this checkout as a new version of Factory without touching the
# running one. It builds the backend and app into <home>/releases/<version>/
# and names that version in <home>/releases/latest. Factory notices within a
# second, and its app shows an Update banner; the person decides when.
#
# When the backend's source hasn't changed since the last release, the
# release reuses that backend, so updating to it only replaces the app and
# never interrupts the agents. A backend that fails to start after an update
# is undone by the supervisor (crates/factoryd/src/update.rs).
#
# A release for the real Factory is signed with the Developer ID and notarized
# (scripts/sign-release.sh); without the identity and the notarization key
# it's refused, unless --adhoc says to sign it ad hoc on purpose.
#
# A release for the real Factory needs its entries in docs/CHANGELOG.md,
# stamped with its version by scripts/changelog.sh stamp (and committed); it
# builds that version. Each release carries the changelog, which the app shows.
#
#   scripts/release.sh         for the real Factory (~/.factory, Factory.app)
#   scripts/release.sh --dev   for the dev Factory (~/.factory-dev, Factory Dev)
#   scripts/release.sh --out DIR
#                              a release for the real Factory, built and
#                              signed exactly the same, into DIR instead of
#                              ~/.factory/releases: a rehearsal (of signing
#                              and notarizing, say) that no running Claktory
#                              sees, so it shows no Update banner.
#   --rebuild                  the newest release again from its own source
#                              commit (its `source`), replacing it: for a fix
#                              in how releases are built, before anyone has
#                              installed it.
#   --no-channel               a real release without an update channel
#                              (FACTORY_UPDATE_URL): refused without it, since
#                              a Claktory that installs one never hears of
#                              another release. Only on purpose.
#   --adhoc                    with either of those: allows a real release
#                              signed ad hoc. Without it, one that this Mac
#                              can't sign with the Developer ID and notarize
#                              is refused: an ad hoc app is another app to
#                              macOS, which then hides what the person's app
#                              kept in the Keychain (its connections, say).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/lib.sh"

usage() { echo "usage: scripts/release.sh [--dev | [--out DIR] [--adhoc] [--rebuild] [--no-channel]]" >&2; exit 2; }
RELEASES="$HOME/.factory/releases"; APP_NAME="Factory.app"; DEFINES=(); ADHOC=false; REBUILD=false; NO_CHANNEL=false
while (($#)); do
  case "$1" in
    --dev) RELEASES="$HOME/.factory-dev/releases"; APP_NAME="Factory Dev.app"; DEFINES=(--dart-define=FACTORY_PORT=7421 --dart-define=FACTORY_DEV=true); shift ;;
    --out)
      [[ -n "${2:-}" ]] || usage
      RELEASES="$(mkdir -p "$2" && cd "$2" && pwd)"
      case "$RELEASES/" in
        "$HOME/.factory/"* | "$HOME/.factory-dev/"*) echo "--out is for a folder outside ~/.factory and ~/.factory-dev" >&2; exit 2 ;;
      esac
      shift 2
      ;;
    --adhoc) ADHOC=true; shift ;;
    --rebuild) REBUILD=true; shift ;;
    --no-channel) NO_CHANNEL=true; shift ;;
    *) usage ;;
  esac
done
if [[ "$APP_NAME" == "Factory Dev.app" ]] && { [[ "$RELEASES" != "$HOME/.factory-dev/releases" ]] || $ADHOC || $REBUILD || $NO_CHANNEL; }; then usage; fi
if [[ "$APP_NAME" == "Factory.app" ]] && ! $ADHOC && [[ "$(signing_mode)" == adhoc ]]; then
  echo "a real release is signed with the Developer ID and notarized, and this Mac can't now: it needs the \"Developer ID Application\" identity and the App Store Connect key (scripts/sign-release.sh says where). An ad hoc app is another app to macOS, and can't read what the person's app keeps in the Keychain. Build it ad hoc anyway only on purpose, with --adhoc." >&2
  exit 1
fi
# Direct messages keep their keys in the data-protection keychain, in the
# group only the app's Developer ID provisioning profile grants: a release
# signed without it would leave every Mac unable to keep them (Fugo, E6).
if [[ "$APP_NAME" == "Factory.app" ]] && ! $ADHOC; then
  MAC_PROFILE="${CLAKTORY_MAC_PROFILE:-$HOME/.appstoreconnect/claktory-mac.provisionprofile}"
  [[ -f "$MAC_PROFILE" ]] || {
    echo "a real release carries the Mac app's Developer ID provisioning profile, and there's none at $MAC_PROFILE (CLAKTORY_MAC_PROFILE names another): without it the app can't keep direct-message keys. scripts/sign-release.sh says what it must be." >&2
    exit 1
  }
fi
# The browser helper in a real release only as a rehearsal (--out DIR) until
# browser control passes B6b (Fugo): a stray variable can't publish it.
if [[ "${CLAKTORY_BROWSER_HOST:-}" == 1 && "$RELEASES" == "$HOME/.factory/releases" ]]; then
  echo "CLAKTORY_BROWSER_HOST=1 is for rehearsals (--out DIR) until browser control passes B6b" >&2
  exit 1
fi
if [[ "$APP_NAME" == "Factory.app" ]]; then
  # Built only from what's committed: a release is never someone's work in
  # progress. The real one only from main's commit (on main, or a checkout
  # whose branch is exactly main).
  [[ -z "$(git -C "$ROOT" status --porcelain)" ]] || {
    echo "the checkout has uncommitted or untracked changes; a release is built from a clean tree only:" >&2
    git -C "$ROOT" status --short >&2
    exit 1
  }
  if $REBUILD; then
    # --rebuild: the newest release again, from its own source commit, with
    # today's build (a fix in how releases are built, not in what they hold).
    # It replaces that release; only one nobody has installed yet should be.
    FACTORY_VERSION="$(cat "$RELEASES/latest" 2>/dev/null)" || { echo "no release in $RELEASES to rebuild" >&2; exit 1; }
    SOURCE="$(cat "$RELEASES/$FACTORY_VERSION/source" 2>/dev/null)" || { echo "$RELEASES/$FACTORY_VERSION doesn't say what it was built from" >&2; exit 1; }
    git -C "$ROOT" merge-base --is-ancestor "$SOURCE" main || { echo "$FACTORY_VERSION's source, $SOURCE, isn't on main" >&2; exit 1; }
    # And on the channel it was built for, unless told otherwise.
    if [[ -z "${FACTORY_UPDATE_URL:-}" ]]; then
      FACTORY_UPDATE_URL="$(cat "$RELEASES/$FACTORY_VERSION/update-url" 2>/dev/null || true)"
      if [[ -n "$FACTORY_UPDATE_URL" ]]; then export FACTORY_UPDATE_URL; else unset FACTORY_UPDATE_URL; fi
    fi
  else
    SOURCE="$(git -C "$ROOT" rev-parse HEAD)"
    if [[ "$RELEASES" == "$HOME/.factory/releases" && "$SOURCE" != "$(git -C "$ROOT" rev-parse main)" ]]; then
      echo "a release is built from main's commit only; this checkout is at $(git -C "$ROOT" rev-parse --short HEAD), main at $(git -C "$ROOT" rev-parse --short main)" >&2
      exit 1
    fi
  fi
  # The person's release checks a channel for its successors
  # (FACTORY_UPDATE_URL, stamped into its backend): built without one, a
  # Claktory that installs it never hears of another release (v0.1.17's first
  # build, 2026-11-22). Only on purpose, with --no-channel.
  if [[ "$RELEASES" == "$HOME/.factory/releases" && -z "${FACTORY_UPDATE_URL:-}" ]] && ! $NO_CHANNEL; then
    echo "a release of the real Claktory needs its update channel: FACTORY_UPDATE_URL=https://github.com/claktory/claktory/releases/latest/download/latest.json scripts/release.sh (or --no-channel, only on purpose)" >&2
    exit 1
  fi
  # Publishing it pushes its source, which the secret check must pass: find
  # out now, at the freeze, not once it's built and tried.
  "$ROOT/scripts/check-secrets.sh" "$SOURCE"
  # Built from an export of that commit in one fixed, private folder, so the
  # binaries name only that folder, never this Mac's home or checkouts; Rust's
  # toolchain paths come out as ~ (scripts/lib.sh). Nothing an earlier build
  # left is used (release_inputs).
  SRC_ROOT="$(release_tree "$ROOT" "$SOURCE")"
  trap release_unlock EXIT
  export SRC_ROOT
  release_rustflags
  release_inputs
  release_pub_get "$SRC_ROOT/app"
  # QA-only lying features (E11) must stay refused in release builds: their
  # guards are checked here, never remembered (Fugo 40183). A target folder of
  # its own, so this release's build starts from nothing as before.
  if [[ -x "$SRC_ROOT/scripts/check-qa-lies.sh" ]]; then
    QA_LIES_TARGET="$(mktemp -d)"
    CARGO_TARGET_DIR="$QA_LIES_TARGET" "$SRC_ROOT/scripts/check-qa-lies.sh" || { rm -rf "$QA_LIES_TARGET"; echo "a QA-only lying feature isn't refused in release builds; not releasing" >&2; exit 1; }
    rm -rf "$QA_LIES_TARGET"
  fi
else
  SRC_ROOT="$ROOT"
fi
mkdir -p "$RELEASES"
if [[ "$APP_NAME" == "Factory.app" ]]; then
  if $REBUILD; then
    [[ "$("$SRC_ROOT/scripts/changelog.sh" pending /nonexistent 2>/dev/null)" == "$FACTORY_VERSION" ]] || { echo "$SOURCE's changelog isn't stamped $FACTORY_VERSION" >&2; exit 1; }
  else
    # The version the changelog's newest entries are stamped with.
    FACTORY_VERSION="$("$SRC_ROOT/scripts/changelog.sh" pending "$RELEASES")"
  fi
else
  FACTORY_VERSION="$(stamp_version "$RELEASES")"
fi
export FACTORY_VERSION

FINGERPRINT="$(backend_fingerprint)"
if [[ "$APP_NAME" == "Factory.app" ]]; then
  # Signing with a Developer ID makes a different backend: the first signed
  # release ships one even when the source didn't change.
  SIGNING="$(signing_mode)"
  if [[ "$SIGNING" != adhoc ]]; then
    FINGERPRINT="$(printf '%s\nsigning: %s\n' "$FINGERPRINT" "$SIGNING" | shasum -a 256 | cut -d' ' -f1)"
  fi
fi
PREVIOUS=""
if [[ -f "$RELEASES/latest" ]]; then
  PREVIOUS="$RELEASES/$(cat "$RELEASES/latest")"
fi
# The backend is factoryd and the hosts beside it (claktory-apps and
# claktory-speech, which factoryd runs from its own folder): they're built,
# reused and shipped together.
# claktory-video-preview (a Mac helper, #379) is one of them once the source has it.
VIDEO_PREVIEW=false
[[ -f "$SRC_ROOT/app/macos/VideoPreview/main.swift" ]] && VIDEO_PREVIEW=true
# Agents' tools (ripgrep and jq, tools/rg and tools/jq) ship with the
# backend once the source pins them.
RG=false
[[ -f "$SRC_ROOT/scripts/build-rg.sh" ]] && RG=true
JQ=false
[[ -f "$SRC_ROOT/scripts/build-jq.sh" ]] && JQ=true
if [[ -n "$PREVIOUS" && -f "$PREVIOUS/factoryd" && -f "$PREVIOUS/claktory-apps" && -f "$PREVIOUS/claktory-speech" && -f "$PREVIOUS/backend-version" && "$(cat "$PREVIOUS/backend-fingerprint" 2>/dev/null)" == "$FINGERPRINT" ]] &&
   { ! $VIDEO_PREVIEW || [[ -f "$PREVIOUS/claktory-video-preview" ]]; } && { ! $RG || [[ -f "$PREVIOUS/tools/rg" ]]; } && { ! $JQ || [[ -f "$PREVIOUS/tools/jq" ]]; }; then
  BACKEND="$PREVIOUS"
  BACKEND_VERSION="$(cat "$PREVIOUS/backend-version")"
  echo "backend unchanged since $BACKEND_VERSION; this release only updates the app"
else
  # In the source folder: cargo reads the configuration of the folder it runs in.
  (cd "$SRC_ROOT" && cargo build --manifest-path "$SRC_ROOT/Cargo.toml" --release -p factoryd -p claktory-apps -p claktory-speech)
  BACKEND="$SRC_ROOT/target/release"
  rm -f "$BACKEND/claktory-video-preview"
  if $VIDEO_PREVIEW; then build_video_preview "$BACKEND/claktory-video-preview"; fi
  rm -rf "$BACKEND/tools"
  if $RG; then
    "$SRC_ROOT/scripts/build-rg.sh" mac "$BACKEND/tools/rg"
    "$SRC_ROOT/scripts/build-rg.sh" licenses "$BACKEND/tools/LICENSES/ripgrep"
  fi
  if $JQ; then
    "$SRC_ROOT/scripts/build-jq.sh" mac "$BACKEND/tools/jq"
    "$SRC_ROOT/scripts/build-jq.sh" licenses "$BACKEND/tools/LICENSES/jq"
  fi
  BACKEND_VERSION="$FACTORY_VERSION"
fi
# Finder's version too (Info.plist takes numbers only: v0.1.3 is 0.1.3).
BUNDLE_VERSION="$(sed -n 's/^v\([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\).*/\1/p' <<<"$FACTORY_VERSION")"
BUNDLE_VERSION="${BUNDLE_VERSION:-0.0.0}"
(cd "$SRC_ROOT/app" && flutter build macos --release --build-name "$BUNDLE_VERSION" --build-number "$BUNDLE_VERSION" --dart-define=FACTORY_VERSION="$FACTORY_VERSION" ${DEFINES[@]+"${DEFINES[@]}"})

partial="$RELEASES/$FACTORY_VERSION.partial"
rm -rf "$partial"
mkdir -p "$partial"
cp "$BACKEND/factoryd" "$BACKEND/claktory-apps" "$BACKEND/claktory-speech" "$partial/"
if $VIDEO_PREVIEW; then cp "$BACKEND/claktory-video-preview" "$partial/"; fi
if $RG || $JQ; then cp -R "$BACKEND/tools" "$partial/tools"; fi
echo "$BACKEND_VERSION" >"$partial/backend-version"
echo "$FINGERPRINT" >"$partial/backend-fingerprint"
# What it was built from, for scripts/publish.sh --built: the commit, and the
# channel stamped into its backend (none unless FACTORY_UPDATE_URL was set).
git -C "$ROOT" rev-parse "${SOURCE:-HEAD}" >"$partial/source"
echo "${FACTORY_UPDATE_URL:-}" >"$partial/update-url"
if [[ "$APP_NAME" == "Factory.app" ]]; then
  # What's unreleased stays out of a real release.
  awk '/^## /{skip = ($0 == "## Unreleased")} !skip' "$SRC_ROOT/docs/CHANGELOG.md" >"$partial/CHANGELOG.md"
else
  cp "$ROOT/docs/CHANGELOG.md" "$partial/CHANGELOG.md"
fi
take_build "$SRC_ROOT/app/build/macos/Build/Products/Release/Factory.app" "$partial/$APP_NAME"
# Claktory's own apps (apps/dist), checked: offered in Settings -> Apps.
bundled_apps "$SRC_ROOT" "$partial"
if [[ "$APP_NAME" == "Factory Dev.app" ]]; then
  # The browser helper, for trying browser control in Claktory Dev.
  add_browser_host "$partial/$APP_NAME" dev release
  make_dev_app "$partial/$APP_NAME" Release.entitlements
else
  # The supervisor as an app (Developer Tools needs one); installers put it
  # in place, updates never do.
  make_supervisor_app "$partial"
  # And the helpers in the app, so it can set Claktory up from a .dmg.
  add_helpers "$partial" "$APP_NAME"
  # The browser helper ships in the real app only once browser control has
  # passed its release proof (B6b): until then CLAKTORY_BROWSER_HOST=1 puts it
  # in a rehearsal (--out DIR), never in a release people get by default.
  if [[ "${CLAKTORY_BROWSER_HOST:-}" == 1 ]]; then
    add_browser_host "$partial/$APP_NAME" real release
  fi
  # Nothing in it may name this Mac (its home, user, host or checkouts).
  check_no_traces "$partial"
  # Never QA's provider override (Fugo, #332).
  check_no_qa_provider "$partial/factoryd" "$partial/$APP_NAME/Contents/Helpers/factoryd" || exit 1
  # The app's libraries load the way dyld demands (scripts/check-macho.py):
  # a misaligned string pool stops macOS loading the tunnel. The helpers
  # beside it are executables, which macOS's dyld doesn't hold to that.
  python3 "$ROOT/scripts/check-macho.py" "$partial/$APP_NAME/Contents/Frameworks" ||
    { echo "the app's libraries wouldn't load (above); not releasing it" >&2; exit 1; }
  # Developer ID and notarized when this Mac has what that takes; ad hoc
  # otherwise, as dev releases always are.
  "$ROOT/scripts/sign-release.sh" "$partial"
  if ! $ADHOC; then
    spctl --assess --type execute "$partial/$APP_NAME" || { echo "$partial/$APP_NAME isn't notarized after signing; not releasing it" >&2; exit 1; }
  fi
fi
if $REBUILD; then
  # The rebuilt one takes the old one's place under the same name.
  rm -rf "$RELEASES/$FACTORY_VERSION.replaced"
  mv "$RELEASES/$FACTORY_VERSION" "$RELEASES/$FACTORY_VERSION.replaced"
  mv "$partial" "$RELEASES/$FACTORY_VERSION"
  rm -rf "$RELEASES/$FACTORY_VERSION.replaced"
else
  mv "$partial" "$RELEASES/$FACTORY_VERSION"
fi
# Named only once complete; the rename makes the change atomic.
echo "$FACTORY_VERSION" >"$RELEASES/latest.new"
mv -f "$RELEASES/latest.new" "$RELEASES/latest"

# Keep the three newest, by version (v0.1.10 is newer than v0.1.9).
ls -1 "$RELEASES" | grep -v '^latest' | while read -r name; do
  printf '%s\t%s\n' "$(version_key "$name")" "$name"
done | sort -r | cut -f2 | tail -n +4 | while read -r old; do
  rm -rf "${RELEASES:?}/$old"
done
echo "published Claktory $FACTORY_VERSION to $RELEASES"
