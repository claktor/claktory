#!/bin/sh
# Claktory's browser image's entrypoint (#435, Mac). Root only until the
# exec: loads the firewall (or stops), readies the volume at
# /var/lib/claktory-browser, then becomes claktory-browser with no
# capabilities and no_new_privs and runs the runner. stdout is the runner's
# C1 stream: until the exec, everything here writes to stderr.
set -eu
exec 3>&1 1>&2
die() { echo "claktory-browser: $*" >&2; exit 70; }
/usr/local/sbin/claktory-browser-nft || die "the firewall didn't load, so the browser won't start"
nft list table inet claktory_browser >/dev/null 2>&1 || die "the firewall isn't loaded, so the browser won't start"
home=/var/lib/claktory-browser
[ ! -L "$home" ] || die "$home is a symbolic link"
mkdir -p "$home"
chown -h claktory-browser:claktory-browser "$home"
chmod 0700 "$home"
# X's socket folder, root's and 1777: the runner mounts its own tmpfs over it
# and refuses to start without it.
[ ! -L /tmp/.X11-unix ] || die "/tmp/.X11-unix is a symbolic link"
mkdir -p /tmp/.X11-unix
chown root:root /tmp/.X11-unix
chmod 1777 /tmp/.X11-unix
drop="setpriv --reuid=claktory-browser --regid=claktory-browser --init-groups --no-new-privs --inh-caps=-all --bounding-set=-all"
$drop install -d -m 0700 "$home/profiles"
[ $# -gt 0 ] || set -- run
exec 1>&3 3>&-
exec $drop /opt/claktory-browser/runner browser-runner "$@"
