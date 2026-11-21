#!/usr/bin/env python3
"""Checks that Mach-O files will load on a device the way dyld checks them.

    scripts/check-macho.py PATH...

Each PATH is a Mach-O file (thin or universal) or a folder searched for
them (an .app, an .xcarchive, an .ipa unzipped). For every architecture it
checks what dyld checks before it will load an image: that everything in
__LINKEDIT (the symbol table and its strings, fixups, exports, function
starts, the code signature…) lies inside the segment, doesn't overlap, and
starts on the alignment dyld demands. Newer iOS releases demand the symbol
table's string pool be pointer-aligned too, which older ones let pass, so
it's checked at pointer size here: the strictest dyld there is.

It prints one line per problem and exits 1 if there is any, 0 otherwise.
The Simulator's dyld is laxer than a device's, so a build that runs there
can still fail on an iPhone: this is how to know before uploading it.
"""
import os
import struct
import sys

FAT_MAGIC = 0xCAFEBABE
FAT_MAGIC_64 = 0xCAFEBABF
MH_MAGIC = 0xFEEDFACE
MH_MAGIC_64 = 0xFEEDFACF

LC_SEGMENT = 0x1
LC_SYMTAB = 0x2
LC_DYSYMTAB = 0xB
LC_SEGMENT_64 = 0x19
LC_CODE_SIGNATURE = 0x1D
LC_SEGMENT_SPLIT_INFO = 0x1E
LC_DYLD_INFO = 0x22
LC_DYLD_INFO_ONLY = 0x80000022
LC_FUNCTION_STARTS = 0x26
LC_DATA_IN_CODE = 0x29
LC_DYLIB_CODE_SIGN_DRS = 0x2B
LC_LINKER_OPTIMIZATION_HINT = 0x2E
LC_DYLD_EXPORTS_TRIE = 0x80000033
LC_DYLD_CHAINED_FIXUPS = 0x80000034
LC_ATOM_INFO = 0x36

LINKEDIT_DATA = {
    LC_CODE_SIGNATURE: "code signature",
    LC_SEGMENT_SPLIT_INFO: "shared cache info",
    LC_FUNCTION_STARTS: "function starts",
    LC_DATA_IN_CODE: "data in code",
    LC_DYLIB_CODE_SIGN_DRS: "code sign DRs",
    LC_LINKER_OPTIMIZATION_HINT: "linker optimization hints",
    LC_DYLD_EXPORTS_TRIE: "exports trie",
    LC_DYLD_CHAINED_FIXUPS: "chained fixups",
    LC_ATOM_INFO: "atom info",
}


def slices(data):
    """(arch label, offset, size) for each image in the file, or []."""
    if len(data) < 8:
        return []
    magic = struct.unpack(">I", data[:4])[0]
    if magic in (FAT_MAGIC, FAT_MAGIC_64):
        count = struct.unpack(">I", data[4:8])[0]
        out = []
        pos = 8
        for _ in range(count):
            if magic == FAT_MAGIC:
                cpu, sub, off, size, _align = struct.unpack(">iiIII", data[pos:pos + 20])
                pos += 20
            else:
                cpu, sub, off, size, _align, _res = struct.unpack(">iiQQII", data[pos:pos + 32])
                pos += 32
            out.append((f"cpu {cpu & 0xFFFFFF}/{sub & 0xFFFFFF}", off, size))
        return out
    le = struct.unpack("<I", data[:4])[0]
    if le in (MH_MAGIC, MH_MAGIC_64):
        return [("thin", 0, len(data))]
    return []


def check_image(data, label):
    problems = []
    magic = struct.unpack("<I", data[:4])[0]
    is64 = magic == MH_MAGIC_64
    ptr = 8 if is64 else 4
    header = 32 if is64 else 28
    _m, _cpu, _sub, filetype, ncmds, _size, _flags = struct.unpack("<IiiIIII", data[:28])
    if filetype == 1:  # MH_OBJECT: no LINKEDIT to check
        return problems
    blobs = []
    linkedit = None
    pos = header
    for _ in range(ncmds):
        cmd, cmdsize = struct.unpack("<II", data[pos:pos + 8])
        body = data[pos:pos + cmdsize]
        if cmd == LC_SEGMENT_64:
            name = body[8:24].rstrip(b"\0").decode()
            fileoff, filesize = struct.unpack("<QQ", body[40:56])
            if name == "__LINKEDIT":
                linkedit = (fileoff, fileoff + filesize)
        elif cmd == LC_SEGMENT:
            name = body[8:24].rstrip(b"\0").decode()
            fileoff, filesize = struct.unpack("<II", body[32:40])
            if name == "__LINKEDIT":
                linkedit = (fileoff, fileoff + filesize)
        elif cmd == LC_SYMTAB:
            symoff, nsyms, stroff, strsize = struct.unpack("<IIII", body[8:24])
            if nsyms:
                blobs.append(("symbol table", ptr, symoff, nsyms * (16 if is64 else 12)))
            if strsize:
                # dyld's own table says 1, but newer iOS releases refuse a
                # string pool off pointer alignment ("mis-aligned LINKEDIT
                # string pool"): hold it to that.
                blobs.append(("string pool", ptr, stroff, strsize))
        elif cmd == LC_DYSYMTAB:
            f = struct.unpack("<18I", body[8:80])
            indirectoff, nindirect = f[12], f[13]
            extreloff, nextrel, locreloff, nlocrel = f[14], f[15], f[16], f[17]
            if nlocrel:
                blobs.append(("local relocations", ptr, locreloff, nlocrel * 8))
            if nextrel:
                blobs.append(("external relocations", ptr, extreloff, nextrel * 8))
            if nindirect:
                blobs.append(("indirect symbol table", 4, indirectoff, nindirect * 4))
        elif cmd in (LC_DYLD_INFO, LC_DYLD_INFO_ONLY):
            f = struct.unpack("<10I", body[8:48])
            for name, off, size in (("rebase opcodes", f[0], f[1]), ("bind opcodes", f[2], f[3]),
                                    ("weak bind opcodes", f[4], f[5]), ("lazy bind opcodes", f[6], f[7]),
                                    ("exports trie", f[8], f[9])):
                if size:
                    blobs.append((name, ptr, off, size))
        elif cmd in LINKEDIT_DATA:
            off, size = struct.unpack("<II", body[8:16])
            if size:
                blobs.append((LINKEDIT_DATA[cmd], ptr, off, size))
        pos += cmdsize
    if linkedit is None:
        return [f"{label}: no __LINKEDIT segment"]
    start, end = linkedit
    prev_end, prev_name = start, "the start of __LINKEDIT"
    for name, align, off, size in sorted(blobs, key=lambda b: b[2]):
        if off < prev_end:
            problems.append(f"{label}: {name} at {off:#x} overlaps {prev_name}")
        if off + size > end:
            problems.append(f"{label}: {name} at {off:#x} runs past the end of __LINKEDIT")
        if off % align:
            problems.append(f"{label}: {name} at {off:#x} isn't {align}-byte aligned (dyld: mis-aligned LINKEDIT {name})")
        prev_end, prev_name = off + size, name
    return problems


def files(paths):
    for path in paths:
        if os.path.isdir(path):
            for dirpath, _dirs, names in os.walk(path):
                for name in names:
                    full = os.path.join(dirpath, name)
                    if not os.path.islink(full):
                        yield full
        else:
            yield path


def main():
    if len(sys.argv) < 2:
        print(__doc__.strip().split("\n\n")[1], file=sys.stderr)
        return 2
    problems = []
    checked = 0
    for path in files(sys.argv[1:]):
        try:
            with open(path, "rb") as f:
                data = f.read()
        except OSError:
            continue
        for arch, off, size in slices(data):
            checked += 1
            label = path if arch == "thin" else f"{path} ({arch})"
            try:
                problems += check_image(data[off:off + size], label)
            except struct.error:
                problems.append(f"{label}: truncated Mach-O")
    for p in problems:
        print(p)
    if not problems:
        print(f"{checked} Mach-O image(s) load cleanly.")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
