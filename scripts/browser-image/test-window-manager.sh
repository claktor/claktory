#!/usr/bin/env bash
# Checks that the browser image's window manager can start nothing: run it
# again whenever the base image, Openbox or the runner's Openbox settings
# (browser_runner/xvfb.rs) change.
#
#   scripts/browser-image/test-window-manager.sh IMAGE
#
# In a throwaway container of IMAGE (podman; it installs xdotool and
# x11-utils there), as the browser's user: Xvfb, Openbox with the runner's
# settings and environment, and Chrome maximised. Then XTest clicks on the
# root in the panel's margin and the keys that open a menu, a run dialog or
# a terminal elsewhere, each followed by a count of the viewable windows and
# of processes outside Xvfb, Openbox and Chrome. The same with Openbox's
# stock settings is the control: its root menu and its Super+E must show up
# there, or the check proves nothing. Ends 0 only when the runner's settings
# open nothing and the control does.
set -euo pipefail
(($# == 1)) || { echo "usage: test-window-manager.sh IMAGE" >&2; exit 2; }
IMAGE="$1"
ENGINE="${ENGINE:-podman}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

cat >"$work/inside.sh" <<'INSIDE'
#!/bin/sh
set -eu
apt-get update -q >/dev/null 2>&1
apt-get install -yq --no-install-recommends xdotool x11-utils procps >/dev/null 2>&1
mkdir -p /tmp/.X11-unix && chmod 1777 /tmp/.X11-unix
T=/tmp/wm && rm -rf "$T" && install -d -o claktory-browser -m 0700 "$T"
cat >"$T/run.sh" <<'RUN'
set -eu
T=/tmp/wm
cd "$T"
Xvfb :7 -screen 0 1440x900x24 -nolisten tcp -nolisten local >xvfb.log 2>&1 &
sleep 1
export DISPLAY=:7
mkdir -p "$T/openbox/config" "$T/openbox/data"
printf '<?xml version="1.0" encoding="UTF-8"?>\n<openbox_menu xmlns="http://openbox.org/3.4/menu"></openbox_menu>\n' >"$T/openbox/menu.xml"
# As browser_runner/xvfb.rs writes them.
cat >"$T/openbox/openbox.xml" <<CONFIG
<?xml version="1.0" encoding="UTF-8"?>
<openbox_config xmlns="http://openbox.org/3.4/rc">
  <margins><top>0</top><bottom>44</bottom><left>0</left><right>0</right></margins>
  <desktops><number>1</number></desktops>
  <theme><name>/usr/share/themes/Clearlooks</name></theme>
  <keyboard></keyboard>
  <mouse></mouse>
  <menu><file>$T/openbox/menu.xml</file></menu>
</openbox_config>
CONFIG
if [ "$MODE" = runner ]; then
  env -i PATH=/usr/bin:/bin HOME="$T" XDG_CONFIG_HOME="$T/openbox/config" XDG_CONFIG_DIRS="$T/openbox/config" \
    XDG_DATA_HOME="$T/openbox/data" XDG_DATA_DIRS="$T/openbox/data" DISPLAY=:7 \
    openbox --sm-disable --config-file "$T/openbox/openbox.xml" >openbox.log 2>&1 &
else
  env -i PATH=/usr/bin:/bin HOME="$T" DISPLAY=:7 openbox --sm-disable >openbox.log 2>&1 &
fi
sleep 2
google-chrome-stable --no-sandbox --user-data-dir="$T/u" --no-first-run --window-position=0,0 --window-size=1440,900 \
  --start-maximized about:blank >chrome.log 2>&1 &
sleep 6
echo "workarea $(xprop -root _NET_WORKAREA | sed 's/.*= //')"
windows() {
  n=0
  for w in $(xwininfo -root -children | sed -n 's/^ *\(0x[0-9a-f]*\).*/\1/p'); do
    if xwininfo -id "$w" | grep -q IsViewable; then n=$((n + 1)); fi
  done
  echo "$n"
}
others() {
  ps -u claktory-browser -o comm= | grep -v -x -e 'chrome' -e 'chrome_crashpad' -e 'chrome_crashpad_handler' -e 'Xvfb' -e 'openbox' \
    -e 'ps' -e 'grep' -e 'sh' -e 'sleep' -e 'xwininfo' -e 'xdotool' -e 'sed' -e 'cat' -e 'wc' | wc -l
}
echo "start $(windows) $(others)"
step() {
  name="$1"; shift
  xdotool mousemove 700 880 "$@"
  sleep 1
  echo "$name $(windows) $(others)"
  xdotool key Escape
  sleep 0.5
}
step right-click click 3
step middle-click click 2
step left-click click 1
step Alt+F2 key alt+F2
step Alt+Tab key alt+Tab
step Super key super
step Super+E key super+e
step Ctrl+Alt+T key ctrl+alt+t
step Alt+F3 key alt+F3
RUN
setpriv --reuid=claktory-browser --regid=claktory-browser --init-groups env MODE="$MODE" sh "$T/run.sh"
INSIDE

run() {
  "$ENGINE" run --rm --entrypoint sh -e MODE="$1" -v "$work:/w:ro" "$IMAGE" /w/inside.sh
}
runner="$(run runner)"
control="$(run control)"
echo "runner's settings:"; echo "$runner" | sed 's/^/  /'
echo "control (stock settings):"; echo "$control" | sed 's/^/  /'
fail=0
grep -q '^workarea 0, 0, 1440, 856$' <<<"$runner" || { echo "FAIL: the work area isn't the screen less the panel" >&2; fail=1; }
read -r _ windows others < <(grep '^start ' <<<"$runner")
if grep -v -e '^start ' -e '^workarea ' <<<"$runner" | awk -v w="$windows" -v o="$others" '$2 != w || $3 != o' | grep -q .; then
  echo "FAIL: with the runner's settings, something opened a window or started a program" >&2
  fail=1
fi
grep -q '^right-click 2 ' <<<"$control" || { echo "FAIL: the control's root menu didn't show, so this check sees nothing" >&2; fail=1; }
((fail == 0)) && echo "ok: the runner's Openbox opens nothing"
exit "$fail"
