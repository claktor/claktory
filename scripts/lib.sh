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
  root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
  (
    cd "$root"
    find crates proto -type f ! -name .DS_Store -print0 | sort -z | xargs -0 shasum -a 256
    shasum -a 256 Cargo.toml Cargo.lock docs/MANUAL.md
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
</dict>
</plist>
PLIST
  plutil -lint -s "$app/Contents/Info.plist"
  codesign --force --sign - "$app"
}

# Puts the helpers in the app (Contents/Helpers): the backend (factoryd),
# the apps host (claktory-apps) and the supervisor's app, as the release has
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
  if [[ -d "$release/supervisor/Claktory Agents.app" ]]; then
    ditto "$release/supervisor/Claktory Agents.app" "$helpers/Claktory Agents.app"
  fi
}

# The public repository holds one commit per release: its files are exactly
# that release's source, its parent is the previous release's public commit,
# and its message is what the release brings, from the changelog. The
# day-to-day commits stay on this Mac (and its private backup).
#   public_commit REPO SOURCE VERSION [PARENT]
# makes that commit in the git repository REPO (without moving any branch)
# and prints its id: SOURCE's files, VERSION's changelog section from
# SOURCE's docs/CHANGELOG.md (and those of versions between PARENT's and it
# that weren't published on their own), dated when it was released, by
# Claktory. The same arguments always make the same commit, so a publish
# that's tried again makes the one it made before.
PUBLIC_NAME="Claktory"
PUBLIC_EMAIL="releases@claktory.com"
public_commit() {
  local repo="$1" source="$2" version="$3" parent="${4:-}"
  local changelog previous="" sections released date
  changelog="$(git -C "$repo" show "$source:docs/CHANGELOG.md")" || return 1
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
    git -C "$repo" -c commit.gpgsign=false commit-tree "$source^{tree}" ${parent:+-p "$parent"}
}
