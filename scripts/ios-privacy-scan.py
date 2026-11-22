#!/usr/bin/env python3
"""Checks the iPhone app's binaries against App Review's two automatic
refusals, before scripts/testflight.sh uploads it (store plan S9):

- A required-reason API (Apple's list: file timestamps, system boot time,
  disk space, active keyboards, user defaults) used by a binary whose
  category isn't declared, with a reason, in a privacy manifest that covers
  it: its own bundle's PrivacyInfo.xcprivacy, or the app's (ITMS-91053).
- A private or iOS-unavailable symbol (guideline 2.5.1), imported or named
  as a string for dlsym: sendmsg_x and recvmsg_x (libsystem_kernel's
  private batch calls) and SystemConfiguration's SCDynamicStore functions,
  which iOS doesn't offer.

  ios-privacy-scan.py APP_DIR     (Runner.app, from the archive or the .ipa)

Prints each finding and exits 1 on any; prints what each binary uses and
exits 0 otherwise. Reads the binaries with nm and strings (Xcode's).
"""

import plistlib
import subprocess
import sys
from pathlib import Path

# Apple's required-reason APIs
# (developer.apple.com/documentation/bundleresources/describing-use-of-required-reason-api):
# C functions and exported constants are imported symbols; Objective-C and
# Swift API shows as a class reference or a selector name.
CATEGORIES = {
    "NSPrivacyAccessedAPICategoryFileTimestamp": {
        "symbols": {"stat", "fstat", "fstatat", "lstat", "getattrlist", "fgetattrlist", "getattrlistat",
                    "getattrlistbulk", "NSFileCreationDate", "NSFileModificationDate",
                    "NSURLContentModificationDateKey", "NSURLCreationDateKey"},
        "strings": {"creationDate", "modificationDate", "fileModificationDate", "contentModificationDateKey",
                    "creationDateKey"},
    },
    "NSPrivacyAccessedAPICategorySystemBootTime": {
        "symbols": {"mach_absolute_time"},
        "strings": {"systemUptime"},
    },
    "NSPrivacyAccessedAPICategoryDiskSpace": {
        "symbols": {"statfs", "statvfs", "fstatfs", "fstatvfs", "NSFileSystemFreeSize", "NSFileSystemSize",
                    "NSURLVolumeAvailableCapacityKey", "NSURLVolumeAvailableCapacityForImportantUsageKey",
                    "NSURLVolumeAvailableCapacityForOpportunisticUsageKey", "NSURLVolumeTotalCapacityKey"},
        "strings": {"volumeAvailableCapacityKey", "volumeAvailableCapacityForImportantUsageKey",
                    "volumeAvailableCapacityForOpportunisticUsageKey", "volumeTotalCapacityKey",
                    "systemFreeSize", "systemSize"},
    },
    "NSPrivacyAccessedAPICategoryActiveKeyboards": {
        "symbols": set(),
        "strings": {"activeInputModes"},
    },
    "NSPrivacyAccessedAPICategoryUserDefaults": {
        "symbols": {"OBJC_CLASS_$_NSUserDefaults"},
        "strings": {"NSUserDefaults"},
    },
}

PRIVATE_SYMBOLS = {"sendmsg_x", "recvmsg_x"}
PRIVATE_PREFIXES = ("SCDynamicStore",)


def run(*args: str) -> str:
    return subprocess.run(args, capture_output=True, text=True, errors="replace").stdout


def imported(binary: Path) -> set[str]:
    names = set()
    for line in run("nm", "-u", "-j", str(binary)).splitlines():
        name = line.strip()
        names.add(name[1:] if name.startswith("_") else name)
    return names


def strings(binary: Path) -> set[str]:
    return {line.strip() for line in run("strings", "-a", str(binary)).splitlines()}


def declared(manifest: Path) -> set[str]:
    if not manifest.is_file():
        return set()
    with manifest.open("rb") as file:
        data = plistlib.load(file)
    return {entry.get("NSPrivacyAccessedAPIType") for entry in data.get("NSPrivacyAccessedAPITypes", [])
            if entry.get("NSPrivacyAccessedAPITypeReasons")}


def binaries(app: Path):
    """Each Mach-O in the app with the bundle it belongs to."""
    def main_binary(bundle: Path) -> Path | None:
        info = bundle / "Info.plist"
        if not info.is_file():
            return None
        with info.open("rb") as file:
            name = plistlib.load(file).get("CFBundleExecutable")
        return bundle / name if name and (bundle / name).is_file() else None

    for bundle in [app, *sorted((app / "PlugIns").glob("*.appex")), *sorted((app / "Frameworks").glob("*.framework"))]:
        binary = main_binary(bundle)
        if binary:
            yield bundle, binary
    for dylib in sorted((app / "Frameworks").glob("*.dylib")):
        yield app, dylib


def main() -> int:
    if len(sys.argv) != 2 or not Path(sys.argv[1], "Info.plist").is_file():
        print(__doc__.strip().splitlines()[-4], file=sys.stderr)
        print("usage: ios-privacy-scan.py APP_DIR", file=sys.stderr)
        return 2
    app = Path(sys.argv[1])
    app_declared = declared(app / "PrivacyInfo.xcprivacy")
    findings = []
    for bundle, binary in binaries(app):
        name = binary.relative_to(app)
        syms, strs = imported(binary), strings(binary)
        covered = app_declared | declared(bundle / "PrivacyInfo.xcprivacy")
        used = []
        for category, api in CATEGORIES.items():
            hits = sorted((api["symbols"] & syms) | (api["strings"] & strs))
            if not hits:
                continue
            short = category.removeprefix("NSPrivacyAccessedAPICategory")
            used.append(short)
            if category not in covered:
                findings.append(f"{name}: uses {short} ({', '.join(hits)}) with no reason declared in "
                                f"{(bundle / 'PrivacyInfo.xcprivacy').relative_to(app.parent)} or the app's manifest (ITMS-91053)")
        private = sorted(n for n in (syms | strs) if n in PRIVATE_SYMBOLS or n.startswith(PRIVATE_PREFIXES))
        for symbol in private:
            how = "imports" if symbol in syms else "names (for dlsym)"
            findings.append(f"{name}: {how} {symbol}, private or unavailable on iOS (guideline 2.5.1)")
        print(f"{name}: {', '.join(used) or 'no required-reason API'}")
    for finding in findings:
        print(f"REFUSED: {finding}", file=sys.stderr)
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
