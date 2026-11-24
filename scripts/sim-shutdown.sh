#!/usr/bin/env bash
# Shuts an iOS Simulator device down without leaving anything behind.
#
#   scripts/sim-shutdown.sh <udid or exact device name>
#
# Use it instead of `simctl terminate` and instead of killing simulator
# processes by hand:
# - a graceful `simctl terminate` can wedge the device's whole simctl queue;
# - killing launchd_sim orphans the device's daemons, and an orphaned
#   diagnosticd has grown 25 MB/s to 13 GB and filled the Mac's swap.
#
# It stops this Mac's `log stream` processes for the device, notes every
# process the device runs, shuts it down with `simctl shutdown`, then
# kills whichever of those processes is still alive (diagnosticd above all).
# Every simctl call has a time limit. It touches no other device.
set -uo pipefail

[[ $# -eq 1 ]] || { echo "usage: scripts/sim-shutdown.sh <udid or exact device name>" >&2; exit 2; }
WANT="$1"
t() { local secs="$1"; shift; timeout "$secs" "$@"; }

# The device: by UDID, or by its exact name when only one has it.
list="$(t 30 xcrun simctl list devices)" || { echo "simctl list didn't answer in 30 s" >&2; exit 1; }
if [[ "$WANT" =~ ^[0-9A-Fa-f-]{36}$ ]]; then
  UDID="$(tr a-z A-Z <<<"$WANT")"
  grep -q "($UDID)" <<<"$list" || { echo "no device $UDID" >&2; exit 1; }
else
  matches="$(grep -F "    $WANT (" <<<"$list" | grep -oE '\([0-9A-F-]{36}\)' | tr -d '()')"
  [[ -n "$matches" ]] || { echo "no device named \"$WANT\"" >&2; exit 1; }
  [[ "$(wc -l <<<"$matches")" -eq 1 ]] || { echo "several devices are named \"$WANT\": give its UDID" >&2; exit 1; }
  UDID="$matches"
fi
state="$(grep "($UDID)" <<<"$list" | grep -oE '\((Booted|Shutdown|Shutting Down|Booting)\)' | tr -d '()')"
echo "device $UDID: ${state:-unknown}"

# 1. This Mac's log streams for the device (`log stream` naming it, or
#    `simctl spawn <udid> log stream`), and their simctl parents.
streams="$(ps -axo pid=,command= | grep -E "(log stream|simctl spawn).*$UDID|$UDID.*log stream" | grep -v grep | awk '{print $1}')"
if [[ -n "$streams" ]]; then
  echo "stopping log streams: $(tr '\n' ' ' <<<"$streams")"
  kill $streams 2>/dev/null
fi

# 2. Every process the device runs: its launchd_sim and that one's children.
launchd_sim="$(ps -axo pid=,command= | grep "launchd_sim .*/Devices/$UDID/" | grep -v grep | awk '{print $1}')"
device_pids=""
for l in $launchd_sim; do
  device_pids+=" $l $(ps -axo pid=,ppid= | awk -v p="$l" '$2==p {print $1}' | tr '\n' ' ')"
done

# 3. Shut it down.
if [[ "$state" != "Shutdown" ]]; then
  if t 90 xcrun simctl shutdown "$UDID"; then echo "shut down"
  else echo "simctl shutdown didn't finish in 90 s; cleaning up the device's processes anyway" >&2; fi
fi

# 4. Whatever of the device is still alive, after a moment to exit: its
#    daemons left behind, then launchd_sim itself.
sleep 3
left=""
for p in $device_pids; do kill -0 "$p" 2>/dev/null && left+=" $p"; done
# Orphans from an earlier run that touched this device (diagnosticd keeps its data folder open).
for p in $(ps -axo pid=,ppid=,command= | awk '$2==1 && /CoreSimulator\/Volumes\/iOS_.*diagnosticd/ {print $1}'); do
  lsof -p "$p" 2>/dev/null | grep -q "/Devices/$UDID/" && left+=" $p"
done
if [[ -n "${left// }" ]]; then
  echo "killing what the device left running:$left"
  kill -9 $left 2>/dev/null
fi
echo "done: $UDID"
