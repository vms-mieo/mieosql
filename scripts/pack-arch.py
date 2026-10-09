#!/usr/bin/env python3
"""Assemble an Arch Linux pacman package (.pkg.tar.zst) for MieoSQL.

Stdlib only: writes .PKGINFO + .MTREE (with md5/sha256 digests) and streams
the payload through `zstd` into `mieosql-<ver>-<rel>-x86_64.pkg.tar.zst`.

Layout inside the package:
    usr/bin/MieoSQL
    usr/share/applications/mieosql.desktop
    usr/share/icons/hicolor/1024x1024/apps/mieosql.png

Usage:
    python3 scripts/pack-arch.py --version 0.1.0 \\
        --binary target/release/MieoSQL \\
        --icon apps/mieosql-desktop/icons/icon.png \\
        --out-dir dist-arch
"""

import argparse
import hashlib
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import time

ARCH = "x86_64"
PKGNAME = "mieosql"
# Tauri 2 renders through the system webview: keep runtime deps explicit.
DEPENDS = ("gtk3", "webkit2gtk-4.1", "hicolor-icon-theme")

DESKTOP_FILE = """\
[Desktop Entry]
Categories=Development;
Comment=Multi-engine database client
Exec=MieoSQL
Icon=mieosql
Name=MieoSQL
StartupWMClass=MieoSQL
Type=Application
"""


def digest(path: str, algo: str) -> str:
    h = hashlib.new(algo)
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--version", required=True, help="upstream version, e.g. 0.1.0")
    ap.add_argument("--pkgrel", default="1")
    ap.add_argument("--binary", required=True, help="built MieoSQL executable")
    ap.add_argument("--icon", required=True, help="1024px icon.png")
    ap.add_argument("--out-dir", required=True)
    ap.add_argument("--url", default="https://github.com/vms-mieo/mieosql")
    args = ap.parse_args()

    for p in (args.binary, args.icon):
        if not os.path.isfile(p):
            print(f"error: missing input file: {p}", file=sys.stderr)
            return 1
    if shutil.which("zstd") is None:
        print("error: `zstd` is not installed", file=sys.stderr)
        return 1
    if shutil.which("tar") is None:
        print("error: `tar` is not installed", file=sys.stderr)
        return 1

    builddate = int(time.time())
    pkgver = f"{args.version}-{args.pkgrel}"
    out_name = f"{PKGNAME}-{pkgver}-{ARCH}.pkg.tar.zst"
    os.makedirs(args.out_dir, exist_ok=True)

    stage = tempfile.mkdtemp(prefix="mieosql-arch-")
    try:
        files: list[tuple[str, int]] = []  # (archive path, mode)

        def put(arcpath: str, mode: int) -> str:
            full = os.path.join(stage, arcpath)
            os.makedirs(os.path.dirname(full), exist_ok=True)
            files.append((arcpath, mode))
            return full

        shutil.copyfile(args.binary, put("usr/bin/MieoSQL", 0o755))
        shutil.copyfile(args.icon, put("usr/share/icons/hicolor/1024x1024/apps/mieosql.png", 0o644))
        with open(put("usr/share/applications/mieosql.desktop", 0o644), "w") as f:
            f.write(DESKTOP_FILE)

        # .PKGINFO — installed size = sum of payload file sizes.
        size = sum(os.path.getsize(os.path.join(stage, p)) for p, _ in files)
        pkginfo = [f"pkgname = {PKGNAME}", f"pkgver = {pkgver}",
                   "pkgdesc = Multi-engine database client (MySQL, MariaDB, PostgreSQL, SQLite)",
                   f"url = {args.url}", f"builddate = {builddate}",
                   "packager = MieoSQL CI", f"size = {size}", f"arch = {ARCH}",
                   "license = MIT", *(f"depend = {d}" for d in DEPENDS)]
        with open(os.path.join(stage, ".PKGINFO"), "w") as f:
            f.write("\n".join(pkginfo) + "\n")

        # .MTREE — directory tree + per-file size/mode/digests.
        dirs = sorted({os.path.dirname(p) for p, _ in files})
        # include every ancestor dir so mtree covers the full tree
        all_dirs = set()
        for d in dirs:
            parts = d.split("/")
            for i in range(1, len(parts) + 1):
                all_dirs.add("/".join(parts[:i]))
        lines = ["#mtree", "/set type=file uid=0 gid=0 mode=644"]
        for d in sorted(all_dirs):
            lines.append(f"./{d} type=dir mode=755 time={builddate}")
        lines.append(f"./.PKGINFO type=file mode=644 time={builddate} "
                     f"size={os.path.getsize(os.path.join(stage, '.PKGINFO'))} "
                     f"md5digest={digest(os.path.join(stage, '.PKGINFO'), 'md5')} "
                     f"sha256digest={digest(os.path.join(stage, '.PKGINFO'), 'sha256')}")
        for arcpath, mode in sorted(files):
            full = os.path.join(stage, arcpath)
            lines.append(f"./{arcpath} time={builddate} mode={mode:o} "
                         f"size={os.path.getsize(full)} "
                         f"md5digest={digest(full, 'md5')} "
                         f"sha256digest={digest(full, 'sha256')}")
        with open(os.path.join(stage, ".MTREE"), "w") as f:
            f.write("\n".join(lines) + "\n")

        out_path = os.path.abspath(os.path.join(args.out_dir, out_name))
        # Reproducible-ish archive: fixed owner/group on GNU tar; file mtimes
        # come from the just-written stage (all "now"). zstd -19 matches makepkg.
        try:
            ver = subprocess.run(["tar", "--version"], capture_output=True,
                                 text=True, timeout=30)
            is_gnu = "GNU tar" in (ver.stdout + ver.stderr)
        except (OSError, subprocess.SubprocessError):
            is_gnu = False
        repro_flags: list[str] = []
        if is_gnu:
            # NOTE: GNU tar spelling is `--sort=name` (with `=`), not `--sort-name`.
            # bsdtar (macOS) has no --sort at all, hence the is_gnu guard.
            repro_flags = ["--sort=name", "--owner=0", "--group=0", "--numeric-owner"]

        def run_tar(extra_flags: list[str]) -> int:
            cmd = ["tar", *extra_flags, "-cf", "-", "-C", stage,
                   ".PKGINFO", ".MTREE", "usr"]
            tar = subprocess.Popen(cmd, stdout=subprocess.PIPE)
            assert tar.stdout is not None
            with open(out_path, "wb") as out:
                zstd_proc = subprocess.run(["zstd", "-19", "-c"],
                                           stdin=tar.stdout, stdout=out)
            tar.stdout.close()
            rc_tar = tar.wait()
            # If zstd itself failed, surface that too.
            if zstd_proc.returncode != 0:
                print("error: zstd failed", file=sys.stderr)
                return 1
            return rc_tar

        rc = run_tar(repro_flags)
        if rc != 0 and repro_flags:
            # Old/stripped GNU tar may reject --sort; retry plainly rather
            # than hard-failing CI. Package is still valid, just unsorted.
            print("warning: tar with repro flags failed, retrying without them",
                  file=sys.stderr)
            rc = run_tar([])
        if rc != 0:
            print("error: tar failed", file=sys.stderr)
            return 1
        print(f"wrote {out_path} ({os.path.getsize(out_path)} bytes)")
        # sanity: list contents (portable across GNU/bsdtar)
        zstd_cat = subprocess.Popen(["zstd", "-d", "-c", out_path],
                                    stdout=subprocess.PIPE)
        listed = subprocess.run(["tar", "-tf", "-"], stdin=zstd_cat.stdout,
                                capture_output=True, text=True)
        zstd_cat.wait()
        if listed.returncode != 0:
            print(f"error: cannot list {out_path}:\n{listed.stderr}", file=sys.stderr)
            return 1
        print(listed.stdout.strip())
        return 0
    finally:
        shutil.rmtree(stage, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
