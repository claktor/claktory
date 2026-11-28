# Sourced by the build scripts (scripts/lib.sh, scripts/ios.sh): when this
# shell already runs inside a macOS sandbox, as agents' commands do on a Mac
# where Claktory's browser is set up, Xcode's Swift package steps can't start
# their own (macOS refuses a sandbox inside a sandbox: "sandbox_apply:
# Operation not permitted"), so every xcodebuild here, and Flutter's, runs
# with Swift packages' own sandbox off (this folder's xcodebuild and xcrun).
# The build stays inside the sandbox it's in; only the packages' extra one
# is gone. Nothing changes Xcode's settings, and outside a sandbox nothing
# is different.
if [[ "$(uname -s)" == Darwin && -x /usr/bin/sandbox-exec ]] \
  && ! /usr/bin/sandbox-exec -p '(version 1)(allow default)' /usr/bin/true 2>/dev/null; then
  PATH="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd):$PATH"
  export PATH
fi
