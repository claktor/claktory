#!/usr/bin/env bash
# Installs Factory on a Mac, or its backend on a Linux machine (Windows too,
# in WSL 2), from its published releases, with no checkout, Rust or Flutter
# needed:
#
#   curl -fsSL <channel>/install.sh | bash
#
# scripts/publish.sh uploads this file as install.sh with each release, with
# the channel's manifest address filled in below. It downloads the newest
# release, checks it against the manifest, puts it in ~/.factory/releases/ as
# scripts/release.sh would, and installs it with the install-release.sh that
# comes in the release (on Linux, its install-linux.sh: a systemd user
# service, remote access on, and a pairing code for the Mac's app). Running it
# again repairs the install. Afterwards the backend checks the channel itself,
# and the app offers each new release.
#
#   curl -fsSL <channel>/install.sh | bash -s -- [--yes] [--json] [--app-only]
#
#   --yes         ask nothing and wait for nothing: no pairing code on a first
#                 Linux install (make one with factoryd pair), and the Mac's
#                 app isn't opened
#   --json        for agents: implies --yes; what a person would read goes to
#                 stderr, and stdout gets one JSON line, {"ok":true,…} (with
#                 factoryd status's answer) or {"ok":false,"error":…,"message":…}.
#                 Exit codes as factoryd's: 0 done, 3 refused (this machine
#                 can't run it: error "unsupported"), 4 installed but not
#                 running ("not_running", or "port_in_use": another user's
#                 Claktory has its port), 1 anything else ("install_failed").
#   --app-only    on a Mac, only the app, from the release's .dmg, with no
#                 Claktory of its own: it connects to one elsewhere
#   --import BUNDLE [--map OLD=NEW]...
#                 move a Claktory here (#217): install the release held (not
#                 started), run `factoryd import BUNDLE` with the passphrase
#                 asked on the terminal (--map translates folder paths from
#                 the old computer), then start Claktory. Needs a terminal;
#                 not with --json or --app-only.
#   --receive     move a Claktory here over the network (#217): install the
#                 release held, then run `factoryd receive`, which shows the
#                 code to paste into the old computer's Claktory and waits;
#                 once the move is in, start Claktory. Not with --app-only.
#                 With --json, for an agent over ssh: receive's own JSON
#                 lines come through as they happen ({"event":"code"…},
#                 "progress", "confirm"), and the person's yes or no about
#                 the digits is read from this script's stdin, so it must not
#                 be the script itself: run it as
#                   bash -c "$(curl -fsSL <channel>/install.sh)" -- --receive --json
#                 The last line is {"ok":…}: the installer's, once Claktory
#                 runs with the move in, or receive's own when the move
#                 stopped (exit 3 refused, 1 failed, 130 stopped).
#   FACTORY_UPDATE_URL=<manifest url>  another channel, such as a file:// one
#   install-online.sh --download-only DIR   only download and check the
#                                           release, into DIR/<version>
#
# factoryd, the backend's command, ends up on PATH: a link in ~/.local/bin,
# or /usr/local/bin when run as root.
set -euo pipefail

# Filled in by scripts/publish.sh.
MANIFEST_URL=""

JSON=false
YES=false
# Piped into bash (curl … | bash), the script is its stdin: nothing else
# can be read from there.
SCRIPT_ON_STDIN=false
[[ -n "${BASH_SOURCE[0]:-}" || -n "${BASH_EXECUTION_STRING:-}" ]] || SCRIPT_ON_STDIN=true

# A string as JSON: quoted, with quotes, backslashes and control characters
# escaped.
json_string() {
  local text="$1"
  text="${text//\\/\\\\}"
  text="${text//\"/\\\"}"
  text="${text//$'\n'/\\n}"
  text="${text//$'\t'/ }"
  text="$(tr -d '\000-\037' <<<"$text")"
  printf '"%s"' "$text"
}

# Stops with ERROR (a code for agents) and EXIT, saying why (unless it's been
# said: --said).
stop() {
  local error="$1" code="$2" message="$3"
  if $JSON; then
    printf '{"ok":false,"error":"%s","message":%s}\n' "$error" "$(json_string "$message")"
  fi
  [[ "${4:-}" == --said ]] || echo "$message" >&2
  exit "$code"
}
fail() { stop install_failed 1 "Claktory can't be installed: $*"; }
refuse() { stop unsupported 3 "Claktory can't be installed: $*"; }

# What a person reads: stdout, or stderr when stdout is for JSON.
say() { if $JSON; then echo "$@" >&2; else echo "$@"; fi; }

file_size() { if [[ "$(uname -s)" == Darwin ]]; then stat -f %z "$1"; else stat -c %s "$1"; fi; }
file_sha256() { { shasum -a 256 "$1" 2>/dev/null || sha256sum "$1"; } | cut -d' ' -f1; }

# A Linux machine has no plutil: the manifest is read as scripts/publish.sh
# writes it, a key on each line, and one line per Linux build:
#     "x86_64": {"url": "…", "sha256": "…", "size": 123},
top_field() { sed -n "s/^  \"$1\": \"\{0,1\}\([^\",]*\)\"\{0,1\},\{0,1\}\$/\1/p" "$2" | head -n 1; }
linux_field() { sed -n "s/.*\"$1\": \"\{0,1\}\([^\",}]*\).*/\1/p" <<<"$2"; }

# "15.7.3" -> 15007, to compare macOS versions.
macos_number() {
  local major minor
  IFS=. read -r major minor _ <<<"$1"
  echo $((10#${major:-0} * 1000 + 10#${minor:-0}))
}

main() {
  local url="${FACTORY_UPDATE_URL:-$MANIFEST_URL}" download_only="" app_only=false import_bundle="" receive=false
  local import_maps=()
  while (($#)); do
    case "$1" in
      --yes) YES=true ;;
      --json) JSON=true YES=true ;;
      --app-only) app_only=true ;;
      --download-only)
        download_only="${2:-}"
        [[ -n "$download_only" ]] || stop usage 3 "--download-only needs a folder."
        shift
        ;;
      --import)
        import_bundle="${2:-}"
        [[ -n "$import_bundle" ]] || stop usage 3 "--import needs the move bundle's file."
        shift
        ;;
      --map)
        [[ "${2:-}" == *=* ]] || stop usage 3 "--map takes /old/path=/new/path."
        import_maps+=(--map "$2")
        shift
        ;;
      --receive) receive=true ;;
      *) stop usage 3 "install.sh doesn't know $1. It takes --yes, --json, --app-only, --import BUNDLE [--map OLD=NEW], --receive and --download-only DIR." ;;
    esac
    shift
  done
  if [[ -n "$import_bundle" ]] && $receive; then
    stop usage 3 "--import and --receive are two ways to move a Claktory here: use one."
  fi
  if [[ -n "$import_bundle" ]]; then
    $JSON && stop usage 3 "--import asks for the bundle's passphrase, so it needs a person's terminal: no --json. An agent runs factoryd import itself."
    $app_only && stop usage 3 "--import brings a whole Claktory, not only the app: no --app-only."
    [[ -z "$download_only" ]] || stop usage 3 "--import and --download-only don't mix."
    [[ -f "$import_bundle" ]] || refuse "there's no file at $import_bundle. Copy the move bundle here first (scp, say), then install with --import again."
    [[ -r /dev/tty ]] || refuse "--import asks for the bundle's passphrase, which needs a terminal."
    import_bundle="$(cd "$(dirname "$import_bundle")" && pwd)/$(basename "$import_bundle")"
    YES=true
  fi
  if $receive; then
    $app_only && stop usage 3 "--receive brings a whole Claktory, not only the app: no --app-only."
    [[ -z "$download_only" ]] || stop usage 3 "--receive and --download-only don't mix."
    if $JSON; then
      ! $SCRIPT_ON_STDIN || stop usage 3 "--receive --json reads the person's yes or no on stdin, and piped into bash that's the script itself: run it as bash -c \"\$(curl -fsSL <this script's address>)\" -- --receive --json"
    else
      [[ -r /dev/tty ]] || refuse "--receive confirms the move's digits, which needs a terminal (or --json, for an agent)."
    fi
    YES=true
  fi
  [[ -n "$url" ]] || fail "this installer doesn't know where releases are published (set FACTORY_UPDATE_URL)."
  local os
  os="$(uname -s)"
  [[ "$os" == Darwin || "$os" == Linux ]] || refuse "Claktory runs on macOS and Linux."
  [[ "$os" == Darwin ]] || ! $app_only || refuse "--app-only is for a Mac: on Linux, Claktory has no app, only what the app connects to."

  # Global, for the trap that runs after main returns.
  work="$(mktemp -d)"
  trap 'rm -rf "$work"' EXIT
  say "Reading $url"
  curl -fsSL --proto '=https,file' "$url" -o "$work/latest.json" || fail "couldn't read $url"
  local version backend tarball sha256 size min_macos="" archs="" arch
  arch="$(uname -m)"
  if [[ "$os" == Linux ]]; then
    case "$arch" in
      x86_64 | amd64) arch=x86_64 ;;
      aarch64 | arm64) arch=aarch64 ;;
      *) refuse "Claktory runs on x86_64 and aarch64 Linux, and this machine is $arch." ;;
    esac
    version="$(top_field version "$work/latest.json")"
    backend="$(top_field backend "$work/latest.json")"
    local build
    build="$(grep "^    \"$arch\": {" "$work/latest.json" || true)"
    [[ -n "$build" || -z "$version" ]] || refuse "Claktory $version has no build for Linux $arch."
    tarball="$(linux_field url "$build")"
    sha256="$(linux_field sha256 "$build")"
    size="$(linux_field size "$build")"
  else
    field() { plutil -extract "$1" raw -o - "$work/latest.json" 2>/dev/null || true; }
    version="$(field version)"
    backend="$(field backend)"
    if $app_only; then
      tarball="$(field dmg.url)"
      sha256="$(field dmg.sha256)"
      size="$(field dmg.size)"
      [[ -n "$tarball" || -z "$version" ]] || fail "Claktory $version has no .dmg to install the app from."
    else
      tarball="$(field url)"
      sha256="$(field sha256)"
      size="$(field size)"
    fi
    min_macos="$(field min_macos)"
    archs="$(plutil -extract archs json -o - "$work/latest.json" 2>/dev/null || true)"
  fi
  [[ "$version" =~ ^[A-Za-z0-9.-]+$ && -n "$tarball" && -n "$sha256" ]] || fail "the manifest at $url is incomplete."
  [[ "$backend" =~ ^[A-Za-z0-9.-]+$ ]] || backend="$version"

  if [[ -n "$min_macos" ]]; then
    local here
    here="$(sw_vers -productVersion)"
    (($(macos_number "$here") >= $(macos_number "$min_macos"))) || refuse "it needs macOS $min_macos or later, and this Mac has $here."
  fi
  if [[ -n "$archs" && "$archs" != *"\"$arch\""* ]]; then
    refuse "this release is built for $archs, and this Mac is $arch."
  fi

  # A Mac with its own Claktory has the app already, beside it.
  if $app_only && [[ -x "$HOME/.factory/bin/factoryd" && -d "$HOME/Applications/Factory.app" ]]; then
    say "This Mac already has Claktory, with its own agents: ~/Applications/Factory.app."
    $JSON && printf '{"ok":true,"app":%s,"version":"%s","own_claktory":true}\n' "$(json_string "$HOME/Applications/Factory.app")" "$version"
    return
  fi

  say "Downloading Claktory $version"
  local progress=--progress-bar
  $JSON && progress=-s
  curl -fSL "$progress" --proto '=https,file' "$tarball" -o "$work/download" || fail "couldn't download $tarball"
  [[ -z "$size" || "$(file_size "$work/download")" == "$size" ]] || fail "the download's size doesn't match the manifest."
  [[ "$(file_sha256 "$work/download")" == "$(tr 'A-F' 'a-f' <<<"$sha256")" ]] ||
    fail "the download doesn't match the manifest's SHA-256."
  if $app_only; then
    install_app_only "$version"
    return
  fi
  mkdir -p "$work/release"
  # On a Mac, the system's tar, as the updater uses: the Mac tarball is xz
  # (#454), which it reads by content despite -z; a GNU tar earlier in the
  # PATH wouldn't. Linux tarballs stay gzip.
  if [[ "$os" == Linux ]]; then tar -xzf "$work/download" -C "$work/release"; else /usr/bin/tar -xzf "$work/download" -C "$work/release"; fi
  if [[ "$os" == Linux ]]; then
    [[ -f "$work/release/factoryd" && -f "$work/release/install-linux.sh" ]] || fail "the release is incomplete."
  else
    [[ -f "$work/release/factoryd" && -d "$work/release/Factory.app" && -f "$work/release/install-release.sh" ]] ||
      fail "the release is incomplete."
    codesign --verify --deep --strict "$work/release/Factory.app" || fail "the app's signature doesn't verify."
    if [[ -d "$work/release/supervisor/Claktory Agents.app" ]]; then
      codesign --verify --deep --strict "$work/release/supervisor/Claktory Agents.app" || fail "the supervisor's signature doesn't verify."
    fi
  fi
  echo "$backend" >"$work/release/backend-version"
  # Windows, through WSL 2 (#476): the release's own checks of WSL and the
  # Windows disk under it, before anything goes into ~/.factory. Unsafe states
  # stop here with what to do; notes are passed on.
  if [[ "$os" == Linux && -z "$download_only" && -f "$work/release/claktory-wsl.sh" ]] && bash "$work/release/claktory-wsl.sh" is-wsl; then
    local wsl_out wsl_status=0
    wsl_out="$(bash "$work/release/claktory-wsl.sh" preflight 2>&1)" || wsl_status=$?
    if ((wsl_status != 0)); then
      [[ "$(wc -l <<<"$wsl_out")" -le 1 ]] || say "$(sed '$d' <<<"$wsl_out")"
      stop unsupported 3 "$(tail -n 1 <<<"$wsl_out")"
    fi
    [[ -z "$wsl_out" ]] || say "$wsl_out"
    export CLAKTORY_WSL_CHECKED=1
  fi

  local releases="$HOME/.factory/releases"
  [[ -n "$download_only" ]] && releases="$download_only"
  mkdir -p "$releases"
  rm -rf "${releases:?}/$version"
  mv "$work/release" "$releases/$version"
  if [[ -n "$download_only" ]]; then
    say "Claktory $version downloaded to $releases/$version"
    $JSON && printf '{"ok":true,"version":"%s","release":%s}\n' "$version" "$(json_string "$releases/$version")"
    return
  fi
  # Named only once complete, as scripts/release.sh does.
  echo "$version" >"$releases/latest.new"
  mv -f "$releases/latest.new" "$releases/latest"

  mkdir -p "$work/out"
  if [[ -n "$import_bundle" ]] || $receive; then
    # A move brings a whole Claktory: never stop one already running here
    # (installing held would). One held by a move that stopped carries on.
    if [[ -x "$HOME/.factory/bin/factoryd" ]] && "$HOME/.factory/bin/factoryd" status >/dev/null 2>&1; then
      refuse "this computer already runs a Claktory, and moving brings another here. Move to a computer without one."
    fi
    grep -q -- --hold "$releases/$version/install-linux.sh" 2>/dev/null || grep -q -- --hold "$releases/$version/install-release.sh" 2>/dev/null ||
      refuse "Claktory $version can't move a Claktory in yet; it needs a newer release."
    local installer="install-linux.sh" installer_args=(--no-pair)
    [[ "$os" == Linux ]] || { installer="install-release.sh"; installer_args=(); }
    say "Installing Claktory $version, held: it starts once the move is in."
    bash "$releases/$version/$installer" "$releases/$version" "${installer_args[@]+"${installer_args[@]}"}" --hold </dev/null >&2 ||
      fail "the release's installer stopped; see above."
  fi
  if [[ -n "$import_bundle" ]]; then
    local pass
    IFS= read -r -s -p "The move bundle's passphrase: " pass </dev/tty >&2 || refuse "--import needs the passphrase."
    echo >&2
    local import_status=0
    printf '%s' "$pass" | "$HOME/.factory/bin/factoryd" import "$import_bundle" --passphrase-stdin ${import_maps[@]+"${import_maps[@]}"} >&2 || import_status=$?
    pass=""
    if ((import_status == 3)); then
      stop import_refused 3 "the import was refused; see above. Nothing changed." --said
    elif ((import_status != 0)); then
      stop import_failed 1 "the import didn't finish. Run this same install command again to resume it, or undo it with: $HOME/.factory/bin/factoryd import --abandon"
    fi
    say "The move bundle is in; starting Claktory."
  elif $receive; then
    # factoryd receive shows the move code (and a QR) and waits for the old
    # computer; its yes/no about the digits is read from the terminal, since
    # this script's own stdin is the curl pipe.
    local receive_status=0
    if $JSON; then
      # Its events go out as they come; its own last line is kept, and
      # given only if the move stops here (else the installer's follows).
      set +e
      "$HOME/.factory/bin/factoryd" receive --json | while IFS= read -r line; do
        case "$line" in
          '{"ok":'*) printf '%s\n' "$line" >"$work/receive-end" ;;
          *) printf '%s\n' "$line" ;;
        esac
      done
      receive_status="${PIPESTATUS[0]}"
      set -e
      if ((receive_status != 0)); then
        if [[ -s "$work/receive-end" ]]; then cat "$work/receive-end"; else printf '{"ok":false,"error":"receive_failed","message":"the move didn'"'"'t finish."}\n'; fi
        exit "$receive_status"
      fi
    else
      "$HOME/.factory/bin/factoryd" receive </dev/tty >&2 || receive_status=$?
    fi
    if ((receive_status == 130)); then
      stop receive_stopped 130 "Stopped before the move finished. Claktory stays installed but not started; run this same command again to carry on."
    elif ((receive_status == 3)); then
      stop receive_refused 3 "the move didn't go ahead (refused or called off; see above). Nothing changed here: Claktory stays installed but not started, and running this same command again starts over with a new code."
    elif ((receive_status != 0)); then
      stop receive_failed 1 "the move didn't finish; see above. Run this same command again to carry on where it stopped."
    fi
    say "The move is done; starting Claktory."
  fi
  if [[ "$os" == Linux ]]; then
    local pair=() status=0
    $YES && pair=(--no-pair)
    # Its own words go to the person; the last of them, when it fails, is why.
    if $JSON; then
      CLAKTORY_INSTALL_OUT="$work/out" bash "$releases/$version/install-linux.sh" "$releases/$version" "${pair[@]}" </dev/null \
        >&2 2> >(tee "$work/out/errors" >&2) || status=$?
    else
      CLAKTORY_INSTALL_OUT="$work/out" bash "$releases/$version/install-linux.sh" "$releases/$version" "${pair[@]}" </dev/null \
        2> >(tee "$work/out/errors" >&2) || status=$?
    fi
    wait
    if ((status != 0)); then
      local why
      why="$(grep -v '^$' "$work/out/errors" 2>/dev/null | tail -n 1)"
      [[ -n "$why" ]] || why="Claktory can't be installed: its installer stopped ($status)."
      case "$status:$why" in
        3:*) stop unsupported 3 "$why" --said ;;
        4:*"already uses port"*) stop port_in_use 4 "$why" --said ;;
        4:*) stop not_running 4 "$why" --said ;;
        *) stop install_failed 1 "$why" --said ;;
      esac
    fi
    answer "$(cat "$work/out/cli" 2>/dev/null || true)" ""
    return
  fi

  # A Mac: install-release.sh's own last line would only say it again.
  local open=--open
  $YES && open=""
  bash "$releases/$version/install-release.sh" "$releases/$version" $open </dev/null >/dev/null ||
    fail "the release's installer stopped; see above."
  # This Mac has a Claktory of its own now, not only the app (--app-only's mark).
  defaults delete dev.factory.app ClaktoryAppOnly >/dev/null 2>&1 || true
  local bin="$HOME/.factory/bin/factoryd"
  waited_status "$bin" ||
    stop not_running 4 "Claktory is installed but isn't running: $(mac_stopped_reason). Its log: ~/.factory/factoryd.log"
  local cli
  cli="$(link_cli "$bin")"
  if $YES; then
    say "Claktory $version is installed in ~/Applications and running."
  else
    say "Claktory $version is installed in ~/Applications and open."
  fi
  say_cli "$cli"
  if ! xcode-select -p >/dev/null 2>&1; then
    say "Agents need git and other developer tools: run  xcode-select --install"
  fi
  answer "$cli" "$HOME/Applications/Factory.app"
}

# Waits up to 30 seconds for the backend BIN to answer factoryd status;
# its answer in $work/out/status.json.
waited_status() {
  local _
  for _ in $(seq 150); do
    "$1" status --json >"$work/out/status.json" 2>/dev/null && return 0
    sleep 0.2
  done
  return 1
}

# Why a Mac's backend stopped, from its log's last word on it.
mac_stopped_reason() {
  local reason
  reason="$(tail -n 200 "$HOME/.factory/factoryd.log" 2>/dev/null | sed -n 's/.*factoryd stopped error=//p' | tail -n 1)"
  [[ -n "$reason" ]] || reason="it didn't start"
  echo "$reason"
}

# Links factoryd onto PATH: /usr/local/bin for root, else ~/.local/bin; never
# over another program of that name. Prints the command's path.
link_cli() {
  local bin="$1" dir="$HOME/.local/bin"
  [[ "$(id -u)" == 0 ]] && dir=/usr/local/bin
  if mkdir -p "$dir" 2>/dev/null && { [[ -L "$dir/factoryd" ]] || [[ ! -e "$dir/factoryd" ]]; }; then
    ln -sfn "$bin" "$dir/factoryd"
    echo "$dir/factoryd"
  else
    echo "$bin"
  fi
}

# Says where factoryd is, and how to reach it when it isn't on PATH.
say_cli() {
  local dir
  dir="$(dirname "$1")"
  case ":$PATH:" in
    *":$dir:"*) say "Its command, factoryd, is in $dir." ;;
    *) say "Its command, factoryd, is in $dir, which isn't on your PATH: add  export PATH=\"$dir:\$PATH\"  to your shell's profile (~/.zprofile)." ;;
  esac
}

# The --json answer: factoryd status's, with where its command and app are.
answer() {
  $JSON || return 0
  local cli="$1" app="$2" status on_path=false extra
  status="$(cat "$work/out/status.json" 2>/dev/null || true)"
  [[ "$status" == "{"*"}" ]] || status='{}'
  status="$(sed -e 's/"ok":true,//' -e 's/,"ok":true}/}/' -e 's/^{"ok":true}$/{}/' <<<"$status")"
  case ":$PATH:" in *":$(dirname "$cli"):"*) on_path=true ;; esac
  extra="\"ok\":true,\"factoryd\":$(json_string "$cli"),\"on_path\":$on_path"
  [[ -n "$app" ]] && extra="$extra,\"app\":$(json_string "$app")"
  if [[ "$status" == "{}" ]]; then
    echo "{$extra}"
  else
    echo "{$extra,${status#\{}"
  fi
}

# --app-only: the app from the .dmg in $work/download, with no Claktory of its
# own on this Mac. The app reads ClaktoryAppOnly at its first launch and
# offers to connect to a Claktory elsewhere instead of setting one up here.
install_app_only() {
  local version="$1" mount="$work/mount" dir=/Applications app
  mkdir -p "$mount"
  # Its errors only when it fails: newer macOS warns that its options are deprecated.
  hdiutil attach -nobrowse -readonly -noautoopen -mountpoint "$mount" "$work/download" >/dev/null 2>"$work/attach.log" ||
    fail "couldn't open the .dmg: $(grep -v -i deprecated "$work/attach.log" | tail -n 1)"
  trap 'hdiutil detach -quiet "$work/mount" 2>/dev/null || true; rm -rf "$work"' EXIT
  [[ -d "$mount/Claktory.app" ]] || fail "the .dmg has no Claktory.app."
  codesign --verify --deep --strict "$mount/Claktory.app" || fail "the app's signature doesn't verify."
  [[ -w "$dir" ]] || dir="$HOME/Applications"
  mkdir -p "$dir"
  app="$dir/Claktory.app"
  rm -rf "$app.new"
  ditto "$mount/Claktory.app" "$app.new" || fail "couldn't copy the app to $dir."
  rm -rf "$app.old"
  [[ ! -e "$app" ]] || mv "$app" "$app.old"
  mv "$app.new" "$app"
  rm -rf "$app.old"
  hdiutil detach -quiet "$mount" 2>/dev/null || true
  # Not on a Mac with its own Claktory (the app ignores it there too).
  [[ -x "$HOME/.factory/bin/factoryd" ]] || defaults write dev.factory.app ClaktoryAppOnly -bool true
  if $YES; then
    say "Claktory $version is installed in $dir. Open it to connect to your Claktory."
  else
    open "$app" || true
    say "Claktory $version is installed in $dir and open: connect it to your Claktory there."
  fi
  $JSON && printf '{"ok":true,"app":%s,"version":"%s"}\n' "$(json_string "$app")" "$version"
  return 0
}

# Only once the whole script has arrived, when it's piped into bash.
main "$@"
