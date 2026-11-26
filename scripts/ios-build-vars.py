#!/usr/bin/env python3
"""The iPhone app's build variables, for scripts/testflight.sh.

testflight.sh archives without signing (CODE_SIGNING_ALLOWED=NO), and then
Xcode expands $(AppIdentifierPrefix) to nothing in every Info.plist: the
v0.1.16 TestFlight build shipped ClaktoryPushKeychainGroup as a bare
"com.claktory.app.notifications", a keychain group the app isn't entitled
to, so its push key never saved (errSecMissingEntitlement) and the phone
never registered for notifications.

  ios-build-vars.py fill APP_DIR SOURCE_DIR TEAM
      For Runner.app and each of its extensions, every top-level string in
      the source Info.plist (SOURCE_DIR/Runner/Info.plist,
      SOURCE_DIR/<Name>/Info.plist) that uses $(AppIdentifierPrefix) is set
      in the archived Info.plist with TEAM. in its place.

  ios-build-vars.py check APP_DIR TEAM
      Refuses (exit 1) when anything in the app, its extensions or the
      entitlements on their code still holds an unexpanded "$(", or names a
      keychain group or application identifier without the TEAM. prefix.
"""

import plistlib
import subprocess
import sys
from pathlib import Path


def bundles(app: Path):
    yield "Runner", app
    for appex in sorted((app / "PlugIns").glob("*.appex")):
        yield appex.stem, appex


def strings(value, where):
    if isinstance(value, str):
        yield where, value
    elif isinstance(value, dict):
        for key, item in value.items():
            yield from strings(item, f"{where}:{key}")
    elif isinstance(value, list):
        for index, item in enumerate(value):
            yield from strings(item, f"{where}:{index}")


def fill(app: Path, source: Path, team: str) -> int:
    for name, bundle in bundles(app):
        src = source / name / "Info.plist"
        if not src.is_file():
            continue
        wanted = plistlib.loads(src.read_bytes())
        path = bundle / "Info.plist"
        plist = plistlib.loads(path.read_bytes())
        for key, value in wanted.items():
            if isinstance(value, str) and "$(AppIdentifierPrefix)" in value:
                built = plist.get(key)
                # Xcode expanded the rest (CLAKTORY_PUSH_GROUP, #501) and
                # left the prefix empty: the team's goes in front of what it
                # built.
                if (
                    value.startswith("$(AppIdentifierPrefix)")
                    and isinstance(built, str)
                    and built
                    and "$(" not in built
                    and not built.startswith(f"{team}.")
                ):
                    plist[key] = f"{team}.{built}"
                else:
                    plist[key] = value.replace("$(AppIdentifierPrefix)", f"{team}.")
                print(f"{name}: {key} = {plist[key].replace(team, 'TEAM')}")
        path.write_bytes(plistlib.dumps(plist, fmt=plistlib.FMT_BINARY))
    return 0


def entitlements(bundle: Path):
    out = subprocess.run(
        ["codesign", "-d", "--entitlements", "-", "--xml", str(bundle)],
        capture_output=True,
    )
    if out.returncode != 0 or not out.stdout.strip():
        return None
    return plistlib.loads(out.stdout)


def check(app: Path, team: str) -> int:
    problems = []
    prefix = f"{team}."
    for name, bundle in bundles(app):
        info = plistlib.loads((bundle / "Info.plist").read_bytes())
        for where, value in strings(info, "Info.plist"):
            if "$(" in value:
                problems.append(f"{name} {where} is unexpanded: {value}")
        for key, value in info.items():
            if isinstance(value, str) and key.endswith(("KeychainGroup", "AccessGroup")) and not value.startswith(prefix):
                problems.append(f"{name} Info.plist:{key} has no team prefix: {value}")
        ents = entitlements(bundle)
        if ents is None:
            problems.append(f"{name} has no entitlements on its code")
            continue
        for where, value in strings(ents, "entitlements"):
            if "$(" in value:
                problems.append(f"{name} {where} is unexpanded: {value}")
        for group in ents.get("keychain-access-groups", []):
            if not group.startswith(prefix):
                problems.append(f"{name} keychain-access-groups has no team prefix: {group}")
        ident = ents.get("application-identifier")
        if ident is not None and not ident.startswith(prefix):
            problems.append(f"{name} application-identifier has no team prefix: {ident}")
        groups = set(ents.get("keychain-access-groups", []))
        group = info.get("ClaktoryPushKeychainGroup")
        if isinstance(group, str) and group not in groups:
            problems.append(f"{name} Info.plist:ClaktoryPushKeychainGroup isn't one of its keychain-access-groups: {group}")
    if problems:
        for problem in problems:
            print("  - " + problem.replace(team, "TEAM"), file=sys.stderr)
        return 1
    print("build variables: every Info.plist and entitlement is expanded, keychain groups carry the team prefix")
    return 0


def main(argv):
    if len(argv) == 5 and argv[1] == "fill":
        return fill(Path(argv[2]), Path(argv[3]), argv[4])
    if len(argv) == 4 and argv[1] == "check":
        return check(Path(argv[2]), argv[3])
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
