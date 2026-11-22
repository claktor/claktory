#!/usr/bin/env python3
"""Checks that the Android app's native libraries will load on a phone.

    app/android/tools/check_artifact.py [options] PATH...

The Android cousin of scripts/check-macho.py, and the gate a release build
must pass before it goes anywhere: Simulator success didn't protect the
iPhone's TestFlight build, and an emulator won't protect this one. Each PATH
is a native library (.so), an APK, an app bundle (.aab), a set of split APKs
(.apks, bundletool build-apks), or a folder searched for them.

For every library it checks what Android's loader and Play check:

- an ELF a 64-bit Android phone loads: little-endian, shared object, for the
  machine its lib/<abi>/ folder says, and an ABI the app ships;
- 16 KiB pages: every LOAD segment aligned to at least 16 KiB, and the RELRO
  range, rounded out to 16 KiB the way the loader protects it, covering no
  writable data outside it;
- the minimum Android it was linked for (.note.android.ident) no newer than
  the app's minSdk;
- its dependencies: Android's own libraries or ones shipped beside it;
- the tunnel library's exports, the ones Dart and Java look up by name;
- no debug info shipped (symbols are kept apart, beside the release);
- no home-folder paths (the hook remaps them to "~").

For every APK, the libraries stored uncompressed at 16 KiB-aligned offsets
(what `zipalign -c -P 16` checks), each ABI folder holding every library
the app needs, and no ABI the app doesn't ship. A bundle's 16 KiB setting is
read with bundletool (--bundletool), never assumed.

It prints one line per problem and exits 1 if there is any, 0 otherwise.
Warnings don't fail the check unless --strict. The pins it checks
against come from app/android/toolchain.properties.
"""
import argparse
import io
import os
import re
import struct
import subprocess
import sys
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
ANDROID = os.path.dirname(HERE)
APP = os.path.dirname(ANDROID)

EM_ARM = 40
EM_X86_64 = 62
EM_AARCH64 = 183
EM_386 = 3
EM_RISCV = 243

ABI_MACHINE = {
    "arm64-v8a": EM_AARCH64,
    "x86_64": EM_X86_64,
    "armeabi-v7a": EM_ARM,
    "x86": EM_386,
    "riscv64": EM_RISCV,
}

PT_LOAD = 1
PT_DYNAMIC = 2
PT_NOTE = 4
PT_GNU_RELRO = 0x6474E552
PF_W = 2

SHT_DYNSYM = 11
SHT_DYNAMIC = 6
SHT_NOTE = 7
SHN_UNDEF = 0
DT_NULL = 0
DT_NEEDED = 1
STB_GLOBAL = 1
STB_WEAK = 2
STV_DEFAULT = 0

# NT_VERSION in the "Android" note: the API level the library was linked for.
ANDROID_NOTE_NAME = b"Android"
ANDROID_NOTE_TYPE = 1

# Libraries the platform provides to apps (the NDK's stable system set).
SYSTEM_LIBS = {
    "libc.so", "libm.so", "libdl.so", "liblog.so", "libandroid.so",
    "libz.so", "libEGL.so", "libGLESv2.so", "libGLESv3.so", "libvulkan.so",
    "libjnigraphics.so", "libmediandk.so", "libOpenSLES.so", "libaaudio.so",
    "libnativewindow.so", "libsync.so", "libbinder_ndk.so", "libicu.so",
    "libamidi.so", "libcamera2ndk.so", "libneuralnetworks.so",
}

TUNNEL = "libclaktory_tunnel.so"

# What Dart (flutter_rust_bridge 2.13's io binding) looks up in the tunnel
# library by name; the generated frbgen_* ones are read from the Dart file.
FRB_CORE_EXPORTS = [
    "frb_create_shutdown_callback",
    "frb_dart_fn_deliver_output",
    "frb_dart_opaque_dart2rust_encode",
    "frb_dart_opaque_drop_thread_box_persistent_handle",
    "frb_dart_opaque_rust2dart_decode",
    "frb_free_wire_sync_rust2dart_dco",
    "frb_free_wire_sync_rust2dart_sse",
    "frb_get_rust_content_hash",
    "frb_init_frb_dart_api_dl",
    "frb_pde_ffi_dispatcher_primary",
    "frb_pde_ffi_dispatcher_sync",
    "frb_rust_vec_u8_free",
    "frb_rust_vec_u8_new",
    "frb_rust_vec_u8_resize",
    "store_dart_post_cobject",
]

# What Java binds by name: the bootstrap that hands iroh the JVM and the
# Application (app/rust/src/android.rs; DESIGN.md, "The JNI bootstrap").
BOOTSTRAP_EXPORTS = [
    "Java_com_claktory_app_TunnelBootstrap_nativeInstall",
    "Java_com_claktory_app_TunnelBootstrap_nativeSystemDnsServers",
]


def read_toolchain(path):
    """toolchain.properties as a dict."""
    values = {}
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("#") or "=" not in line:
                continue
            key, value = line.split("=", 1)
            values[key.strip()] = value.strip()
    return values


def generated_exports(frb_io_dart):
    """The frbgen_* symbols the generated Dart binding looks up."""
    with open(frb_io_dart, encoding="utf-8") as f:
        return sorted(set(re.findall(r"'(frbgen_[A-Za-z0-9_]+)'", f.read())))


class Policy:
    """What the app's artifacts must be, from the pins and the binding."""

    def __init__(self, toolchain, tunnel_exports, release, bootstrap=True):
        self.abis = [a for a in toolchain["android.abis"].split(",") if a]
        self.page = int(toolchain["android.pageSize"])
        self.min_sdk = int(toolchain["android.minSdk"])
        self.tunnel_exports = list(tunnel_exports)
        if bootstrap:
            self.tunnel_exports += BOOTSTRAP_EXPORTS
        # Every ABI folder holds all of these. libapp.so is the Dart code,
        # compiled ahead of time only in profile and release builds.
        self.required_libs = [TUNNEL, "libflutter.so"] + (["libapp.so"] if release else [])
        # Linked with the NDK, so they carry the minimum-API note. libapp.so
        # is written by Dart's gen_snapshot, which writes none.
        self.ndk_linked = {TUNNEL, "libflutter.so"}


class ElfError(Exception):
    pass


class Elf:
    """Just enough of an ELF64 little-endian file to check it."""

    def __init__(self, data):
        self.data = data
        if data[:4] != b"\x7fELF":
            raise ElfError("not an ELF file")
        if data[4] != 2:
            raise ElfError("32-bit ELF; the app ships only 64-bit libraries")
        if data[5] != 1:
            raise ElfError("big-endian ELF")
        (self.type, self.machine, _version, _entry, self.phoff, self.shoff, _flags,
         _ehsize, self.phentsize, self.phnum, self.shentsize, self.shnum,
         self.shstrndx) = struct.unpack_from("<HHIQQQIHHHHHH", data, 16)
        self.phdrs = []
        for i in range(self.phnum):
            off = self.phoff + i * self.phentsize
            if off + 56 > len(data):
                raise ElfError("program headers run past the end of the file")
            (ptype, flags, offset, vaddr, _paddr, filesz, memsz,
             align) = struct.unpack_from("<IIQQQQQQ", data, off)
            self.phdrs.append(dict(type=ptype, flags=flags, offset=offset, vaddr=vaddr,
                                   filesz=filesz, memsz=memsz, align=align))
        self.sections = []
        if self.shoff and self.shnum:
            raw = []
            for i in range(self.shnum):
                off = self.shoff + i * self.shentsize
                if off + 64 > len(data):
                    raise ElfError("section headers run past the end of the file")
                raw.append(struct.unpack_from("<IIQQQQIIQQ", data, off))
            names = raw[self.shstrndx] if self.shstrndx < len(raw) else None
            for (name, stype, flags, addr, offset, size, link, info, align,
                 entsize) in raw:
                label = self._str(names[4], names[5], name) if names else ""
                self.sections.append(dict(name=label, type=stype, offset=offset, size=size,
                                          link=link, entsize=entsize))

    def _str(self, table_offset, table_size, index):
        if index >= table_size:
            return ""
        start = table_offset + index
        end = self.data.find(b"\0", start, table_offset + table_size)
        return self.data[start:end if end >= 0 else table_offset + table_size].decode(
            "utf-8", "replace")

    def section(self, name):
        return next((s for s in self.sections if s["name"] == name), None)

    def dynamic_strtab(self):
        dynsym = next((s for s in self.sections if s["type"] == SHT_DYNSYM), None)
        if dynsym is None or dynsym["link"] >= len(self.sections):
            return None
        return self.sections[dynsym["link"]]

    def needed(self):
        dynamic = next((s for s in self.sections if s["type"] == SHT_DYNAMIC), None)
        strtab = self.dynamic_strtab()
        if dynamic is None or strtab is None:
            return []
        out = []
        for off in range(dynamic["offset"], dynamic["offset"] + dynamic["size"], 16):
            tag, value = struct.unpack_from("<qQ", self.data, off)
            if tag == DT_NULL:
                break
            if tag == DT_NEEDED:
                out.append(self._str(strtab["offset"], strtab["size"], value))
        return out

    def exports(self):
        """Names this library defines for others to look up."""
        dynsym = next((s for s in self.sections if s["type"] == SHT_DYNSYM), None)
        strtab = self.dynamic_strtab()
        if dynsym is None or strtab is None:
            return set()
        out = set()
        entsize = dynsym["entsize"] or 24
        for off in range(dynsym["offset"], dynsym["offset"] + dynsym["size"], entsize):
            name, info, other, shndx, _value, _size = struct.unpack_from("<IBBHQQ", self.data, off)
            if shndx == SHN_UNDEF or (other & 3) != STV_DEFAULT:
                continue
            if (info >> 4) not in (STB_GLOBAL, STB_WEAK):
                continue
            out.add(self._str(strtab["offset"], strtab["size"], name))
        return out

    def android_api(self):
        """The API level in the Android note, or None without one."""
        notes = [(s["offset"], s["size"]) for s in self.sections if s["type"] == SHT_NOTE]
        if not notes:
            notes = [(p["offset"], p["filesz"]) for p in self.phdrs if p["type"] == PT_NOTE]
        for start, size in notes:
            pos = start
            while pos + 12 <= start + size:
                namesz, descsz, ntype = struct.unpack_from("<III", self.data, pos)
                name_at = pos + 12
                desc_at = name_at + ((namesz + 3) & ~3)
                name = self.data[name_at:name_at + namesz].rstrip(b"\0")
                if name == ANDROID_NOTE_NAME and ntype == ANDROID_NOTE_TYPE and descsz >= 4:
                    return struct.unpack_from("<I", self.data, desc_at)[0]
                pos = desc_at + ((descsz + 3) & ~3)
        return None


def round_down(value, page):
    return value - value % page


def round_up(value, page):
    return round_down(value + page - 1, page)


def check_elf(label, data, policy, abi=None, siblings=None):
    """Problems and warnings for one library."""
    problems, warnings = [], []
    name = os.path.basename(label.split("!")[-1])
    try:
        elf = Elf(data)
    except ElfError as e:
        return [f"{label}: {e}"], []

    if elf.type != 3:
        problems.append(f"{label}: not a shared object (ELF type {elf.type})")
    if abi is not None:
        if abi not in policy.abis:
            problems.append(f"{label}: ABI {abi} isn't one the app ships ({', '.join(policy.abis)})")
        expected = ABI_MACHINE.get(abi)
        if expected is not None and elf.machine != expected:
            problems.append(f"{label}: built for machine {elf.machine}, but its folder is {abi} (machine {expected})")
    elif elf.machine not in (ABI_MACHINE[a] for a in policy.abis):
        problems.append(f"{label}: built for machine {elf.machine}, no ABI the app ships")

    loads = [p for p in elf.phdrs if p["type"] == PT_LOAD]
    if not loads:
        problems.append(f"{label}: no LOAD segments")
    for i, p in enumerate(loads):
        if p["align"] < policy.page:
            problems.append(
                f"{label}: LOAD segment {i} aligned to {p['align']} bytes, not {policy.page} "
                "(won't load on a 16 KiB-page phone)")
        elif p["offset"] % p["align"] != p["vaddr"] % p["align"]:
            problems.append(f"{label}: LOAD segment {i}'s offset and address disagree modulo its alignment")

    for relro in (p for p in elf.phdrs if p["type"] == PT_GNU_RELRO):
        start = relro["vaddr"]
        end = relro["vaddr"] + relro["memsz"]
        # The loader makes [round_down(start), round_up(end)) read-only after
        # relocating; nothing writable may share those pages.
        protected = (round_down(start, policy.page), round_up(end, policy.page))
        for p in loads:
            if not p["flags"] & PF_W:
                continue
            for lo, hi in ((p["vaddr"], min(start, p["vaddr"] + p["memsz"])),
                           (max(end, p["vaddr"]), p["vaddr"] + p["memsz"])):
                if lo < hi and lo < protected[1] and hi > protected[0]:
                    problems.append(
                        f"{label}: RELRO ends at {end:#x}, inside a {policy.page}-byte page with "
                        "writable data after it (it would be made read-only)")
                    break

    if name in policy.ndk_linked:
        api = elf.android_api()
        if api is None:
            problems.append(f"{label}: no Android API note; can't tell which Android it needs")
        elif api > policy.min_sdk:
            problems.append(f"{label}: linked for Android API {api}, newer than the app's minSdk {policy.min_sdk}")

    for dep in elf.needed():
        if dep in SYSTEM_LIBS:
            continue
        if siblings is not None and dep in siblings:
            continue
        where = "beside it" if siblings is not None else "(checked alone: pass the APK to see its neighbours)"
        problems.append(f"{label}: needs {dep}, which isn't Android's and isn't shipped {where}")

    if name == TUNNEL:
        missing = [s for s in policy.tunnel_exports if s not in elf.exports()]
        for symbol in missing:
            problems.append(f"{label}: doesn't export {symbol}")

    debug = [s["name"] for s in elf.sections if s["name"].startswith(".debug_")]
    if debug:
        problems.append(f"{label}: ships debug info ({', '.join(sorted(debug)[:3])}…); keep symbols apart")

    # The hook remaps the builder's home folder to "~" (hook/build.dart,
    # rustFlags); a path naming one means that didn't reach this library.
    # Only the libraries this repository builds: Flutter's debug engine
    # holds example strings such as "/Users/myself/" that are no one's.
    home_patterns = (rb"/Users/[A-Za-z0-9._-]+/", rb"/home/[A-Za-z0-9._-]+/") if name == TUNNEL else ()
    for pattern in home_patterns:
        hits = set(re.findall(pattern, data))
        if hits:
            shown = ", ".join(sorted(h.decode("utf-8", "replace") for h in hits)[:3])
            problems.append(f"{label}: names a home folder ({shown}), which would ship in the app")
    return problems, warnings


LIB_ENTRY = re.compile(r"^(?:base/)?lib/([^/]+)/([^/]+\.so)$")


def local_data_offset(blob, info):
    """Where an entry's bytes start: after its local header, whose extra
    field can differ from the central directory's."""
    header = blob[info.header_offset:info.header_offset + 30]
    if header[:4] != b"PK\x03\x04":
        raise zipfile.BadZipFile(f"no local header for {info.filename}")
    name_len, extra_len = struct.unpack_from("<HH", header, 26)
    return info.header_offset + 30 + name_len + extra_len


def check_zip(label, blob, policy, kind):
    """kind: 'apk' (aligned, stored libraries) or 'aab' (bundle, unaligned)."""
    problems, warnings = [], []
    try:
        archive = zipfile.ZipFile(io.BytesIO(blob))
    except zipfile.BadZipFile as e:
        return [f"{label}: {e}"], [], set()
    by_abi = {}
    for info in archive.infolist():
        m = LIB_ENTRY.match(info.filename)
        if m:
            by_abi.setdefault(m.group(1), []).append((m.group(2), info))
    for abi, libs in sorted(by_abi.items()):
        names = {n for n, _ in libs}
        for name, info in libs:
            entry = f"{label}!{info.filename}"
            if kind == "apk":
                if info.compress_type != zipfile.ZIP_STORED:
                    problems.append(f"{entry}: compressed; the app loads libraries straight from the APK, stored")
                else:
                    offset = local_data_offset(blob, info)
                    if offset % policy.page:
                        problems.append(f"{entry}: starts at {offset:#x}, not on a {policy.page}-byte boundary")
            p, w = check_elf(entry, archive.read(info), policy, abi=abi, siblings=names)
            problems += p
            warnings += w
        if abi in policy.abis:
            for name in policy.required_libs:
                if name not in names:
                    problems.append(f"{label}: lib/{abi}/ has no {name}")
    return problems, warnings, set(by_abi)


def check_apk(label, blob, policy, split=False):
    problems, warnings, abis = check_zip(label, blob, policy, "apk")
    if not abis and not split:
        problems.append(f"{label}: no native libraries at all (the tunnel isn't in it)")
    return problems, warnings, abis


def check_apks(label, blob, policy):
    """bundletool build-apks output: split APKs, checked one by one."""
    problems, warnings, abis = [], [], set()
    try:
        archive = zipfile.ZipFile(io.BytesIO(blob))
    except zipfile.BadZipFile as e:
        return [f"{label}: {e}"], []
    for info in archive.infolist():
        if info.filename.endswith(".apk"):
            p, w, a = check_apk(f"{label}!{info.filename}", archive.read(info), policy, split=True)
            problems += p
            warnings += w
            abis |= a
    if not abis:
        problems.append(f"{label}: no split carries native libraries (the tunnel isn't in it)")
    return problems, warnings


def check_aab(label, path, blob, policy, bundletool):
    problems, warnings, abis = check_zip(label, blob, policy, "aab")
    if not abis:
        problems.append(f"{label}: no native libraries at all (the tunnel isn't in it)")
    if bundletool is None:
        problems.append(f"{label}: its 16 KiB setting is unchecked (give --bundletool JAR)")
    else:
        try:
            config = subprocess.run(
                ["java", "-jar", bundletool, "dump", "config", f"--bundle={path}"],
                capture_output=True, text=True, check=True).stdout
        except (OSError, subprocess.CalledProcessError) as e:
            problems.append(f"{label}: bundletool couldn't read its config ({e})")
        else:
            if "PAGE_ALIGNMENT_16K" not in config:
                problems.append(f"{label}: the bundle doesn't ask for 16 KiB page alignment")
    return problems, warnings


def check_path(path, policy, bundletool):
    with open(path, "rb") as f:
        blob = f.read()
    if path.endswith(".so"):
        m = re.search(r"/lib/([^/]+)/[^/]+\.so$", path)
        siblings = set(os.listdir(os.path.dirname(path) or "."))
        return check_elf(path, blob, policy, abi=m.group(1) if m else None, siblings=siblings)
    if path.endswith(".apk"):
        p, w, _ = check_apk(path, blob, policy)
        return p, w
    if path.endswith(".apks"):
        return check_apks(path, blob, policy)
    if path.endswith(".aab"):
        return check_aab(path, path, blob, policy, bundletool)
    return [f"{path}: not a .so, .apk, .apks or .aab"], []


def artifacts(root):
    if os.path.isfile(root):
        yield root
        return
    for folder, _dirs, files in os.walk(root):
        for name in sorted(files):
            if name.endswith((".so", ".apk", ".apks", ".aab")):
                yield os.path.join(folder, name)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("paths", nargs="+")
    parser.add_argument("--toolchain", default=os.path.join(ANDROID, "toolchain.properties"))
    parser.add_argument("--binding", default=os.path.join(APP, "lib", "tunnel", "frb_generated.io.dart"),
                        help="the generated Dart binding whose frbgen_* lookups the tunnel must export")
    parser.add_argument("--debug-build", action="store_true",
                        help="a debug build: no ahead-of-time Dart (libapp.so) expected")
    parser.add_argument("--no-bootstrap", action="store_true",
                        help="before the JNI bootstrap lands: don't require its export")
    parser.add_argument("--bundletool", help="bundletool's jar, to read a bundle's config")
    parser.add_argument("--strict", action="store_true", help="warnings fail the check too")
    args = parser.parse_args(argv)

    policy = Policy(read_toolchain(args.toolchain),
                    FRB_CORE_EXPORTS + generated_exports(args.binding),
                    release=not args.debug_build, bootstrap=not args.no_bootstrap)
    problems, warnings, seen = [], [], 0
    for root in args.paths:
        if not os.path.exists(root):
            problems.append(f"{root}: doesn't exist")
            continue
        for path in artifacts(root):
            seen += 1
            p, w = check_path(path, policy, args.bundletool)
            problems += p
            warnings += w
    if not seen and not problems:
        problems.append("nothing to check: no .so, .apk, .apks or .aab found")
    for line in problems:
        print(line)
    for line in warnings:
        print(f"warning: {line}")
    return 1 if problems or (args.strict and warnings) else 0


if __name__ == "__main__":
    sys.exit(main())
