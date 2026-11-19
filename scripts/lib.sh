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
