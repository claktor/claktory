#!/usr/bin/env bash
# Proves nothing of Claktory Dev comes back from a backup or a
# device-to-device transfer, on an emulator with Android's local backup
# transport (DESIGN.md §5): the connections live in no_backup/ and the
# backup and extraction rules exclude every domain, and this checks it
# rather than trusting the XML.
#
#   app/android/tools/backup_exclusion_check.sh
#
# Needs: one emulator on adb (ANDROID_SERIAL to pick it), the dev debug app
# and its test APK installed from the same build (`./gradlew
# :app:installDevDebug :app:installDevDebugAndroidTest …`), both
# debuggable so run-as works. The test APK is the positive control: it
# allows backup, so its canary must come back, or the check proves nothing.
#
# For each mode, cloud backup and device transfer (the local transport's
# is_device_transfer), it writes a canary into each app's files/,
# shared_prefs/ and no_backup/, backs both up (with @pm@, the metadata a
# restore set needs), deletes the canaries, restores, and looks. Expected:
# cloud refuses Claktory outright (allowBackup is false); transfer, which
# ignores allowBackup from Android 12, gets an empty archive from the
# extraction rules, which the local transport rejects as 0 bytes. Exit 0
# only when the control's canary came back and none of Claktory's did. It
# starts Claktory Dev once, changes the emulator's backup settings and puts
# them back; never run it on a phone that matters.
set -euo pipefail

: "${ANDROID_HOME:?set ANDROID_HOME}"
adb="$ANDROID_HOME/platform-tools/adb"
app=com.claktory.app.dev
control=com.claktory.app.dev.test
transport=com.android.localtransport/.LocalTransport
failed=0

sh_() { "$adb" shell "$@" | tr -d '\r'; }

for pkg in "$app" "$control"; do
  sh_ pm list packages "$pkg" | grep -qx "package:$pkg" || { echo "$pkg isn't installed" >&2; exit 2; }
  sh_ run-as "$pkg" true || { echo "$pkg isn't debuggable (run-as)" >&2; exit 2; }
done

# Read from settings, not bmgr: with backup off, bmgr may list no transports.
was_enabled="$(sh_ settings get secure backup_enabled)"
was_transport="$(sh_ settings get secure backup_transport)"
was_params="$(sh_ settings get secure backup_local_transport_parameters)"
restore_settings() {
  if [[ "$was_params" == null || -z "$was_params" ]]; then
    sh_ settings delete secure backup_local_transport_parameters >/dev/null || true
  else
    sh_ settings put secure backup_local_transport_parameters "$was_params" >/dev/null || true
  fi
  [[ -n "$was_transport" && "$was_transport" != null ]] && sh_ bmgr transport "$was_transport" >/dev/null || true
  [[ "$was_enabled" != 1 ]] && sh_ bmgr enable false >/dev/null || true
}
trap restore_settings EXIT

sh_ bmgr enable true >/dev/null
sh_ bmgr list transports | grep -q "$transport" || { echo "no local transport on this device" >&2; exit 2; }
sh_ bmgr transport "$transport" >/dev/null

# adb shell joins its arguments and the device's shell splits them again,
# so each script goes over as one single-quoted word (none has a ' inside).
in_app() { sh_ "run-as $1 sh -c '$2'"; }
canary() { # pkg action(put|check|drop) mode
  local pkg=$1 action=$2 mode=$3
  case $action in
    put) in_app "$pkg" "mkdir -p files shared_prefs no_backup && echo $mode > files/claktory-canary && echo $mode > no_backup/claktory-canary && echo \"<map><string name=\\\"c\\\">$mode</string></map>\" > shared_prefs/claktory-canary.xml" ;;
    drop) in_app "$pkg" "rm -f files/claktory-canary no_backup/claktory-canary shared_prefs/claktory-canary.xml" ;;
    check) in_app "$pkg" "ls files/claktory-canary no_backup/claktory-canary shared_prefs/claktory-canary.xml 2>/dev/null" || true ;;
  esac
}

# The backup service skips stopped packages, and a fresh install is
# stopped: start each once (the control through its BackupControlWaker),
# and never force-stop them here.
wake() {
  sh_ am start -W -n "$app/com.claktory.app.MainActivity" >/dev/null
  sh_ input keyevent KEYCODE_HOME
  sh_ am broadcast --include-stopped-packages -n "$control/com.claktory.app.BackupControlWaker" >/dev/null
  for pkg in "$app" "$control"; do
    if sh_ dumpsys package "$pkg" | grep -q 'stopped=true'; then
      echo "$pkg is still stopped; the backup service would skip it" >&2
      exit 2
    fi
  done
}
wake

for mode in cloud transfer; do
  if [[ $mode == transfer ]]; then
    sh_ settings put secure backup_local_transport_parameters "is_device_transfer=true" >/dev/null
  else
    sh_ settings put secure backup_local_transport_parameters "is_device_transfer=false" >/dev/null
  fi
  sh_ bmgr wipe "$transport" "$app" >/dev/null 2>&1 || true
  sh_ bmgr wipe "$transport" "$control" >/dev/null 2>&1 || true
  canary "$app" put "$mode"
  canary "$control" put "$mode"
  echo "== $mode: backup"
  sh_ bmgr backupnow --non-incremental @pm@ "$app" "$control" | grep -E "Package|Backup finished" || true
  canary "$app" drop "$mode"
  canary "$control" drop "$mode"
  token="$(sh_ bmgr list sets | sed -n 's/^ *\([0-9a-f]*\) *:.*/\1/p' | head -1)"
  if [[ -z "$token" ]]; then
    echo "$mode: no restore set on the local transport" >&2
    failed=1
    continue
  fi
  echo "== $mode: restore from set $token"
  sh_ bmgr restore "$token" "$app" "$control" | grep -E "restoreFinished|done|error" || true
  back_app="$(canary "$app" check "$mode")"
  back_control="$(canary "$control" check "$mode")"
  if [[ "$back_control" != *files/claktory-canary* ]]; then
    echo "$mode: the control's canary didn't come back; this run proves nothing" >&2
    failed=1
  fi
  if [[ -n "$back_app" ]]; then
    echo "$mode: FAILED, Claktory's came back: $back_app" >&2
    failed=1
  else
    echo "$mode: nothing of Claktory came back (the control's did: ${back_control//$'\n'/ })"
  fi
  canary "$app" drop "$mode"
  canary "$control" drop "$mode"
done

exit $failed
