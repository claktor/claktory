#!/usr/bin/env python3
"""Checks that a Windows release runs on a fresh Windows 10 (2004) or 11:
every .exe and .dll in it imports only DLLs Windows itself has, or ones
the release carries beside the program that loads them. A DLL from the
Visual C++ Redistributable (vcruntime140.dll, msvcp140.dll…) a fresh
Windows doesn't have, from the Build Tools, or a debug runtime, fails it.

    scripts/windows/check-windows-imports.py RELEASE_DIR      check; exit 1 on a problem
    scripts/windows/check-windows-imports.py --list RELEASE_DIR
                                                              every program's imports

It reads each file's import and delay-load tables itself (no dumpbin or
llvm-readobj), so it runs in any lane, on a Mac too.

A DLL counts as carried when it's in the same folder as the program that
imports it, the first place Windows looks. Windows' own DLLs are the
`SYSTEM` list below and the API sets (api-ms-win-*, ext-ms-win-*), which
Windows resolves itself. The Universal C Runtime (ucrtbase.dll and its
api-ms-win-crt-* sets) is part of Windows 10 and later. A Visual C++ runtime
the release needs ships app-locally, beside the program, as Microsoft
allows; a debug one never ships.
"""

import os
import struct
import sys

# Windows' own DLLs, in System32 on every Windows 10 (2004) and 11, Home
# included. One missing here fails the check: add it only once it's known
# to be in a fresh install.
SYSTEM = {
    name.lower()
    for name in """
advapi32 authz avrt bcrypt bcryptprimitives cabinet cfgmgr32 combase comctl32
comdlg32 credui crypt32 cryptbase cryptnet cryptsp d2d1 d3d10warp d3d11 d3d12
d3d9 d3dcompiler_47 dbgeng dbghelp dcomp dhcpcsvc dnsapi dpapi dsound dwmapi
dwrite dxgi dxva2 esent fltlib gdi32 gdiplus glu32 hid imagehlp imm32 iphlpapi
kernel32 kernelbase ksuser mf mfplat mfreadwrite mmdevapi mpr msacm32 msasn1
mscms msctf msimg32 msvcrt mswsock ncrypt netapi32 normaliz ntdll ntdsapi
ole32 oleacc oleaut32 opengl32 pdh powrprof profapi propsys psapi rasapi32
rpcrt4 rstrtmgr secur32 setupapi shcore shell32 shlwapi spoolss sspicli
taskschd tdh ucrtbase uiautomationcore urlmon user32 userenv usp10 uxtheme
version wbemuuid wer wevtapi winhttp wininet winmm winspool.drv winsta
wintrust wldap32 wlanapi ws2_32 wtsapi32 xinput1_4
""".split()
}

# The Visual C++ runtime: not in a fresh Windows, so carried or it fails.
REDIST = ("vcruntime140", "vcruntime140_1", "msvcp140", "msvcp140_1", "msvcp140_2", "msvcp140_atomic_wait",
          "msvcp140_codecvt_ids", "concrt140", "vccorlib140", "vcomp140", "vcamp140", "mfc140", "mfc140u")


def stem(dll):
    name = dll.lower()
    return name[:-4] if name.endswith(".dll") else name


def debug_runtime(dll):
    """A debug C or C++ runtime, which no release carries or Windows has."""
    name = stem(dll)
    return name == "ucrtbased" or name in {f"{base}d" for base in REDIST} or name.startswith(("msvcr", "msvcp")) and name.endswith("d") and name[5:-1].isdigit()


def system(dll):
    name = stem(dll)
    return name in SYSTEM or (dll.lower() in SYSTEM) or name.startswith(("api-ms-win-", "ext-ms-win-", "ext-ms-onecore-"))


class NotPE(Exception):
    pass


def imports(path):
    """The DLLs `path` imports, plain and delay-loaded, as written."""
    with open(path, "rb") as file:
        data = file.read()
    if data[:2] != b"MZ" or len(data) < 0x40:
        raise NotPE("no MZ header")
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    if data[pe:pe + 4] != b"PE\0\0":
        raise NotPE("no PE header")
    sections, optional_size = struct.unpack_from("<H12xH", data, pe + 6)
    optional = pe + 24
    magic = struct.unpack_from("<H", data, optional)[0]
    if magic == 0x20B:
        image_base = struct.unpack_from("<Q", data, optional + 24)[0]
        directories = optional + 112
    elif magic == 0x10B:
        image_base = struct.unpack_from("<I", data, optional + 28)[0]
        directories = optional + 96
    else:
        raise NotPE(f"unknown optional header {magic:#x}")
    count = struct.unpack_from("<I", data, directories - 4)[0]
    table = optional + optional_size
    spans = []
    for index in range(sections):
        virtual_size, address, raw_size, raw = struct.unpack_from("<8xIII I", data, table + index * 40)
        spans.append((address, max(virtual_size, raw_size), raw))

    def offset(rva):
        for address, size, raw in spans:
            if address <= rva < address + size:
                return rva - address + raw
        raise NotPE(f"RVA {rva:#x} is in no section")

    def text(rva):
        at = offset(rva)
        end = data.index(b"\0", at)
        return data[at:end].decode("ascii", "replace")

    def directory(index):
        if index >= count:
            return 0, 0
        return struct.unpack_from("<II", data, directories + index * 8)

    found = []
    rva, size = directory(1)
    if rva:
        at = offset(rva)
        while True:
            lookup, _, _, name, thunk = struct.unpack_from("<IIIII", data, at)
            if not (lookup or name or thunk):
                break
            found.append(text(name))
            at += 20
    rva, size = directory(13)
    if rva:
        at = offset(rva)
        while True:
            attributes, name = struct.unpack_from("<II", data, at)
            if not (attributes or name):
                break
            # Old-style descriptors hold addresses, not RVAs.
            found.append(text(name if attributes & 1 else name - image_base))
            at += 32
    return found


def programs(root):
    for folder, _, files in os.walk(root):
        for name in sorted(files):
            if name.lower().endswith((".exe", ".dll")):
                yield os.path.join(folder, name)


def problems(root):
    """(program, dll, why) for each import a fresh Windows can't satisfy."""
    found = []
    for program in programs(root):
        try:
            dlls = imports(program)
        except (NotPE, struct.error, ValueError) as error:
            found.append((program, "", f"can't be read as a Windows program: {error}"))
            continue
        here = {name.lower() for name in os.listdir(os.path.dirname(program))}
        for dll in dlls:
            if debug_runtime(dll):
                found.append((program, dll, "a debug runtime: built in Debug, never shipped"))
            elif system(dll):
                continue
            elif dll.lower() not in here:
                why = "the Visual C++ runtime, not in a fresh Windows" if stem(dll) in REDIST else "neither Windows' own nor beside it"
                found.append((program, dll, why))
    return found


def main(args):
    listing = "--list" in args
    args = [arg for arg in args if arg != "--list"]
    if len(args) != 1 or not os.path.isdir(args[0]):
        print(__doc__.split("\n\n")[1], file=sys.stderr)
        return 2
    root = args[0]
    if listing:
        for program in programs(root):
            try:
                print(f"{os.path.relpath(program, root)}: {', '.join(imports(program))}")
            except (NotPE, struct.error, ValueError) as error:
                print(f"{os.path.relpath(program, root)}: unreadable ({error})")
        return 0
    found = problems(root)
    for program, dll, why in found:
        print(f"{os.path.relpath(program, root)}: {dll}{': ' if dll else ''}{why}")
    checked = sum(1 for _ in programs(root))
    if found:
        print(f"{len(found)} problem(s) in {checked} programs: a fresh Windows won't run this release.", file=sys.stderr)
        return 1
    print(f"{checked} programs import only Windows' own DLLs or ones beside them.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
