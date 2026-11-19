#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Aditya Telange
"""Write the license notices of every Rust crate compiled into Linshot.

The MIT/Apache/BSD/MPL licenses of the dependencies require their copyright
and license text to accompany binary copies, so the .deb ships this file.
Usage: third-party-licenses.py [target-triple] > THIRD-PARTY-LICENSES
"""
import json
import os
import re
import subprocess
import sys

LICENSE_FILE = re.compile(r"^(licen[cs]e|copying|notice|unlicense|copyright)", re.I)


def main():
    cmd = ["cargo", "metadata", "--format-version", "1", "--locked"]
    if len(sys.argv) > 1:
        cmd += ["--filter-platform", sys.argv[1]]
    meta = json.loads(subprocess.run(cmd, check=True, capture_output=True, text=True).stdout)
    used = {node["id"] for node in meta["resolve"]["nodes"]}
    crates = sorted(
        (p for p in meta["packages"] if p["id"] in used and p["source"] is not None),
        key=lambda p: (p["name"], p["version"]),
    )
    out = sys.stdout
    out.write("Third-party licenses\n====================\n\n")
    out.write("Linshot is licensed under GPL-3.0-or-later. It includes the following\n")
    out.write(f"{len(crates)} Rust crates, distributed under their own licenses below.\n")
    out.write("Their source code is available from https://crates.io.\n\n")
    for p in crates:
        out.write(f"  {p['name']} {p['version']}  ({p.get('license') or 'see below'})\n")
    missing = []
    for p in crates:
        root = os.path.dirname(p["manifest_path"])
        files = sorted(f for f in os.listdir(root) if LICENSE_FILE.match(f) and os.path.isfile(os.path.join(root, f)))
        if p.get("license_file"):
            lf = os.path.normpath(os.path.join(root, p["license_file"]))
            if os.path.isfile(lf) and os.path.basename(lf) not in files:
                files.append(os.path.relpath(lf, root))
        out.write("\n" + "-" * 78 + "\n")
        out.write(f"{p['name']} {p['version']}\n")
        out.write(f"License: {p.get('license') or 'see files'}\n")
        if p.get("repository"):
            out.write(f"Source: {p['repository']}\n")
        if not files:
            missing.append(p["name"])
            out.write("(no license file shipped with this crate)\n")
        for f in files:
            with open(os.path.join(root, f), encoding="utf-8", errors="replace") as fh:
                out.write(f"\n--- {f} ---\n\n{fh.read().rstrip()}\n")
    if missing:
        print(f"warning: no license file in: {', '.join(missing)}", file=sys.stderr)


if __name__ == "__main__":
    main()
