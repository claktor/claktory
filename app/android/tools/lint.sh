#!/usr/bin/env bash
# Android lint over the app and its tests (debug, dev flavor): fails on any
# error, and lists the errors and the API-level warnings. Run it before
# landing anything that touches app/android (DESIGN.md §1). Release builds
# check the fatal ones (calls minSdk lacks, receivers other apps can reach)
# on their own; this is the whole of it.
#
#   app/android/tools/lint.sh
#
# Needs JAVA_HOME and ANDROID_HOME (toolchain.properties' JDK and SDK).
set -euo pipefail
: "${JAVA_HOME:?set JAVA_HOME}" "${ANDROID_HOME:?set ANDROID_HOME}"
here="$(cd "$(dirname "$0")/.." && pwd)"
app="$(dirname "$here")"
(cd "$app" && flutter build apk --config-only --debug --flavor dev --target-platform android-arm64 >/dev/null)
status=0
(cd "$here" && ./gradlew -q :app:lintDevDebug \
  -Pflutter.androidSdkRoot="$ANDROID_HOME" \
  -Pflutter.installedNdkVersions="$(sed -n 's/^android.ndk=//p' toolchain.properties)" \
  -Ptarget-platform=android-arm64 >/dev/null 2>&1) || status=$?
report="$app/build/app/reports/lint-results-devDebug.xml"
[[ -f "$report" ]] || { echo "lint wrote no report (exit $status)" >&2; exit 1; }
python3 - "$report" <<'EOF'
import sys, xml.etree.ElementTree as ET
errors = 0
for issue in ET.parse(sys.argv[1]).getroot().iter("issue"):
    severity, kind = issue.get("severity"), issue.get("id")
    if severity in ("Error", "Fatal") or kind in ("NewApi", "InlinedApi"):
        where = issue.find("location")
        place = f'{where.get("file").split("/app/src/")[-1]}:{where.get("line")}' if where is not None else ""
        print(f"{severity} {kind} {place}: {issue.get('message')}")
        errors += severity in ("Error", "Fatal")
print(f"lint: {errors} error(s)")
sys.exit(1 if errors else 0)
EOF
