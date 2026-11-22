"""Small, synthetic Android artifacts for testing check_artifact.py.

Each is a well-formed ELF64 shared library (or an APK/bundle holding them)
with one thing set right or wrong on purpose, so the verifier's tests can
prove it refuses each failure, not just that it passes a good build.
llvm-objdump reads them the way it reads a real library (the test checks).

The real artifact is still the authority: the first Android build is run
through the verifier, and through the NDK's own tools, before any rule here
is trusted on it (DESIGN.md, "The artifact gate").
"""
import io
import struct
import zipfile

from check_artifact import (ABI_MACHINE, EM_AARCH64, PT_DYNAMIC, PT_GNU_RELRO, PT_LOAD, PT_NOTE,
                            SHT_DYNAMIC, SHT_DYNSYM, SHT_NOTE)

PAGE = 16384
SHT_PROGBITS = 1
SHT_STRTAB = 3
PF_X, PF_W, PF_R = 1, 2, 4


def _pad(blob, align):
    return blob + b"\0" * (-len(blob) % align)


def _round_up(value, page):
    return (value + page - 1) // page * page


def build_so(machine=EM_AARCH64, align=PAGE, api=24, note=True,
             needed=("libc.so", "libm.so", "libdl.so"), exports=(),
             debug=False, relro_page=PAGE, home_path=False, elf_type=3):
    """A shared library. relro_page: the page size its RELRO end is padded
    to (4096 makes one that a 16 KiB phone would fault on)."""
    # String table for the dynamic symbols and DT_NEEDED.
    dynstr = b"\0"
    offsets = {}
    for name in list(needed) + list(exports) + ["malloc"]:
        offsets[name] = len(dynstr)
        dynstr += name.encode() + b"\0"

    sections = []  # (name, type, data, entsize, link)
    if note:
        desc = struct.pack("<I", api) + b"r28c".ljust(64, b"\0") + b"13676358".ljust(64, b"\0")
        body = struct.pack("<III", 8, len(desc), 1) + b"Android\0" + desc
        sections.append((".note.android.ident", SHT_NOTE, body, 0, 0))
    sections.append((".dynstr", SHT_STRTAB, dynstr, 0, 0))
    # Section header indices are the list's plus one: header 0 is null.
    dynstr_index = len(sections)
    # Symbol 0 is the null symbol; exports are defined in .text (the section
    # after .dynsym), "malloc" is an import (undefined), which must not count.
    text_index = len(sections) + 2
    syms = struct.pack("<IBBHQQ", 0, 0, 0, 0, 0, 0)
    for name in exports:
        syms += struct.pack("<IBBHQQ", offsets[name], (1 << 4) | 2, 0, text_index, 0x100, 4)
    syms += struct.pack("<IBBHQQ", offsets["malloc"], (1 << 4) | 2, 0, 0, 0, 0)
    sections.append((".dynsym", SHT_DYNSYM, syms, 24, dynstr_index))
    text = b"\xc0\x03\x5f\xd6" * 16
    if home_path:
        text += b"/Users/someone/.cargo/registry/src/lib.rs\0"
    sections.append((".text", SHT_PROGBITS, text, 0, 0))
    dynamic = b"".join(struct.pack("<qQ", 1, offsets[n]) for n in needed) + struct.pack("<qQ", 0, 0)
    sections.append((".dynamic", SHT_DYNAMIC, dynamic, 16, dynstr_index))
    if debug:
        sections.append((".debug_info", SHT_PROGBITS, b"\0" * 32, 0, 0))

    names = b"\0"
    name_at = []
    for name, *_ in sections + [(".shstrtab",)]:
        name_at.append(len(names))
        names += name.encode() + b"\0"
    sections.append((".shstrtab", SHT_STRTAB, names, 0, 0))

    phnum = 4 + (1 if note else 0)
    body = b""
    placed = []  # (offset, size) per section
    start = 64 + phnum * 56
    for _name, _type, data, _ent, _link in sections:
        body = _pad(body, 8)
        placed.append((start + len(body), len(data)))
        body += data
    shoff = _round_up(start + len(body), 8)
    body = _pad(body, 8)

    dyn_index = next(i for i, s in enumerate(sections) if s[0] == ".dynamic")
    dyn_off, dyn_size = placed[dyn_index]
    rw_vaddr = dyn_off + PAGE  # congruent with its offset modulo any page size
    relro_end = _round_up(rw_vaddr + dyn_size, relro_page)
    rw_memsz = relro_end - rw_vaddr + 0x100  # writable .data after RELRO

    phdrs = [
        (PT_LOAD, PF_R | PF_X, 0, 0, dyn_off, dyn_off, align),
        (PT_LOAD, PF_R | PF_W, dyn_off, rw_vaddr, dyn_size, rw_memsz, align),
        (PT_DYNAMIC, PF_R | PF_W, dyn_off, rw_vaddr, dyn_size, dyn_size, 8),
        (PT_GNU_RELRO, PF_R, dyn_off, rw_vaddr, dyn_size, relro_end - rw_vaddr, 1),
    ]
    if note:
        note_off, note_size = placed[0]
        phdrs.append((PT_NOTE, PF_R, note_off, note_off, note_size, note_size, 4))

    shdrs = struct.pack("<IIQQQQIIQQ", 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
    for i, (name, stype, data, entsize, link) in enumerate(sections):
        offset, size = placed[i]
        addr = rw_vaddr if name == ".dynamic" else offset
        shdrs += struct.pack("<IIQQQQIIQQ", name_at[i], stype, 2, addr, offset, size, link,
                             1 if stype == SHT_DYNSYM else 0, 8, entsize)
    shnum = len(sections) + 1
    ehdr = b"\x7fELF" + bytes([2, 1, 1, 0]) + b"\0" * 8 + struct.pack(
        "<HHIQQQIHHHHHH", elf_type, machine, 1, 0, 64, shoff, 0, 64, 56, phnum, 64, shnum,
        shnum - 1)
    out = ehdr + b"".join(struct.pack("<IIQQQQQQ", t, f, o, v, v, fs, ms, a)
                          for t, f, o, v, fs, ms, a in phdrs)
    return out + body + shdrs


def build_zip(entries, page=PAGE):
    """entries: (name, data, stored, aligned). Aligned stored entries get
    their bytes on a page boundary, as zipalign -P 16 puts them."""
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as z:
        for name, data, stored, aligned in entries:
            info = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_STORED if stored else zipfile.ZIP_DEFLATED
            header = buf.tell() + 30 + len(name.encode())
            if stored:
                # zipalign's extra field (id 0xD935): 2+2+2 bytes, then padding;
                # one byte more than aligned puts the data off its page.
                pad = -(header + 6) % page + (0 if aligned else 1)
                info.extra = struct.pack("<HHH", 0xD935, 2 + pad, page) + b"\0" * pad
            z.writestr(info, data)
    return buf.getvalue()


def tunnel_exports():
    """The tunnel's exports, as the verifier expects them by default."""
    import os

    from check_artifact import (APP, BOOTSTRAP_EXPORTS, FRB_CORE_EXPORTS,
                                generated_exports)
    return (FRB_CORE_EXPORTS + generated_exports(os.path.join(APP, "lib", "tunnel", "frb_generated.io.dart"))
            + BOOTSTRAP_EXPORTS)


def good_libs(release=True, exports=None, machine=EM_AARCH64):
    """A good ABI folder's libraries, by name."""
    libs = {
        "libclaktory_tunnel.so": build_so(machine=machine,
                                          exports=tunnel_exports() if exports is None else exports),
        "libflutter.so": build_so(machine=machine,
                                  needed=("libc.so", "libm.so", "libdl.so", "liblog.so", "libandroid.so")),
    }
    if release:
        libs["libapp.so"] = build_so(machine=machine, note=False, needed=())
    return libs


def good_apk(abis=("arm64-v8a",), libs=None):
    entries = [("AndroidManifest.xml", b"<manifest/>", False, False),
               ("classes.dex", b"dex\n035\0", False, False)]
    for abi in abis:
        machine = ABI_MACHINE[abi]
        for name, data in (libs or good_libs(machine=machine)).items():
            entries.append((f"lib/{abi}/{name}", data, True, True))
    return build_zip(entries)
