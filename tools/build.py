#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Build against installed or explicitly selected SDK source without vendoring it."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess

root = Path(__file__).resolve().parents[1]
build = Path(os.environ.get("XR_SYSTEM_UI_BUILD_DIR", root / "build/cargo-source")).resolve()
build.mkdir(parents=True, exist_ok=True)
manifest = (root / "Cargo.toml").read_text()
for package, variable, sibling in [
    ("xr-shell-protocols", "XR_SHELL_PROTOCOLS_DIR", root.parent / "xr-shell/protocols"),
    ("xr-spatial-kit", "XR_SPATIAL_KIT_DIR", root.parent / "xr-spatial-kit"),
]:
    match = re.search(r"^" + re.escape(package) + r"\s*=\s*\{.*\}$", manifest, re.M)
    if not match:
        raise SystemExit("Missing SDK dependency: " + package)
    version = re.search(r'version\s*=\s*"=?([^\"]+)"', match.group()).group(1)
    default = sibling if (sibling / "Cargo.toml").is_file() else Path("/usr/share/cargo/registry") / (package + "-" + version)
    source = Path(os.environ.get(variable, default)).resolve()
    if not (source / "Cargo.toml").is_file():
        raise SystemExit("Provide " + variable + " or install the source SDK for " + package)
    actual = re.search(r'^version\s*=\s*"([^\"]+)"', (source / "Cargo.toml").read_text(), re.M).group(1)
    if actual != version:
        raise SystemExit(package + ": expected SDK " + version + ", found " + actual)
    replacement = re.sub(r'path\s*=\s*"[^\"]*"', "path = " + json.dumps(str(source)), match.group())
    manifest = manifest[:match.start()] + replacement + manifest[match.end():]
manifest += '\n[[bin]]\nname = "xr-system-ui"\npath = ' + json.dumps(str(root / "src/main.rs")) + "\n"
(build / "Cargo.toml").write_text(manifest)
shutil.copyfile(root / "Cargo.lock", build / "Cargo.lock")
environment = os.environ.copy()
environment.setdefault("CARGO_TARGET_DIR", str(root / "target"))
subprocess.run(["cargo", "build", "--manifest-path", str(build / "Cargo.toml"),
                "--offline", "--locked", "--release"], env=environment, check=True)
