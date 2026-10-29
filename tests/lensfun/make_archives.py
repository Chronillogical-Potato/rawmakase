#!/usr/bin/env python3
"""Writes the synthetic Lensfun database archives the installer's tests read
(src/lensfun_install/tests.rs): made-up lenses in Lensfun's format, packed as
Lensfun publishes its database (a flat .tar.bz2 whose files are dated with the
database's timestamp), and archives that must be refused.

    python3 tests/lensfun/make_archives.py
"""
import io
import tarfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
OLD, NEW = 1793180205, 1800000000


def lens(k1):
    return f"""<!DOCTYPE lensdatabase SYSTEM "lensfun-database.dtd">
<lensdatabase version="2">
<mount><name>Canon EF</name></mount>
<camera><maker>Canon</maker><model>Canon EOS 5D Mark III</model><mount>Canon EF</mount><cropfactor>1</cropfactor></camera>
<lens><maker>Canon</maker><model>Canon EF 70-200mm f/2.8L IS II USM</model><mount>Canon EF</mount><cropfactor>1</cropfactor>
<calibration><distortion model="poly3" focal="70" k1="{k1}"/><distortion model="poly3" focal="200" k1="0.01"/></calibration></lens>
</lensdatabase>
""".encode()


CAMERAS = b"""<lensdatabase version="2"><camera><maker>Nikon Corporation</maker><model>Nikon D6</model><mount>Nikon F AF</mount><cropfactor>1</cropfactor></camera></lensdatabase>
"""


def write(name, entries):
    """entries: (name, bytes | None, mtime, kind) with kind "file", "symlink" or "dir"."""
    out = io.BytesIO()
    with tarfile.open(fileobj=out, mode="w:bz2", format=tarfile.USTAR_FORMAT) as t:
        for entry, data, mtime, kind in entries:
            info = tarfile.TarInfo(entry)
            info.mtime = mtime
            info.uid = info.gid = 0
            info.mode = 0o644
            if kind == "symlink":
                info.type = tarfile.SYMTYPE
                info.linkname = "/etc/passwd"
                t.addfile(info)
            elif kind == "dir":
                info.type = tarfile.DIRTYPE
                t.addfile(info)
            else:
                info.size = len(data)
                t.addfile(info, io.BytesIO(data))
    (HERE / name).write_bytes(out.getvalue())


def main():
    good = [("slr-canon.xml", lens(-0.02), OLD, "file"), ("slr-nikon.xml", CAMERAS, OLD, "file")]
    write("good.tar.bz2", good)
    write("newer.tar.bz2", [("slr-canon.xml", lens(-0.05), NEW, "file"), ("slr-nikon.xml", CAMERAS, NEW, "file")])
    write("stamped.tar.bz2", good + [("timestamp.txt", f"{OLD}\n".encode(), OLD, "file")])
    write("stamp-differs.tar.bz2", good + [("timestamp.txt", f"{NEW}\n".encode(), OLD, "file")])
    write("symlink.tar.bz2", good + [("link.xml", None, OLD, "symlink")])
    write("traversal.tar.bz2", good + [("../escaped.xml", lens(0.0), OLD, "file")])
    write("absolute.tar.bz2", good + [("/tmp/absolute.xml", lens(0.0), OLD, "file")])
    write("folder.tar.bz2", [("db/slr-canon.xml", lens(-0.02), OLD, "file")])
    write("directory.tar.bz2", good + [("db", None, OLD, "dir")])
    write("other-file.tar.bz2", good + [("README.md", b"hello", OLD, "file")])
    write("not-lensfun.tar.bz2", good + [("broken.xml", b"<lensdatabase version='2'><lens>", OLD, "file")])
    write("no-lenses.tar.bz2", [("slr-nikon.xml", CAMERAS, OLD, "file")])
    # 40 MB of zeros packs into a few hundred bytes: more than any database.
    write("too-large.tar.bz2", good + [("huge.xml", bytes(40 << 20), OLD, "file")])
    (HERE / "not-bzip2.tar.bz2").write_bytes(b"<lensdatabase/>")
    # Past the limits by the number of entries, with a damaged file at the end that
    # a reader stopping at its limit would never see.
    stamps = [("timestamp.txt", f"{OLD}\n".encode(), OLD, "file")] * 70000
    write("many-entries.tar.bz2", good + stamps + [("broken.xml", b"<lensdatabase version='2'><lens>", OLD, "file")])
    # Cut short: the last byte of a whole archive missing.
    whole = (HERE / "good.tar.bz2").read_bytes()
    (HERE / "truncated.tar.bz2").write_bytes(whole[:-1])


main()
