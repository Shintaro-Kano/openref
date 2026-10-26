#!/usr/bin/env python3
"""Verify licenses of all Cargo (aarch64-apple-darwin) and npm dependencies offline.
Fails with exit code 1 if any GPL/LGPL/AGPL dependency is found in bundled/linked packages.
"""
import glob
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CARGO = Path.home() / ".cargo" / "bin" / "cargo"

FORBIDDEN_TOKENS = ("GPL", "LGPL", "AGPL")


def check_cargo() -> list[str]:
    base_cmd = [
        str(CARGO if CARGO.exists() else "cargo"),
        "metadata",
        "--filter-platform",
        "aarch64-apple-darwin",
        "--format-version=1",
        "--locked",
    ]
    try:
        out = subprocess.check_output([*base_cmd[:2], "--offline", *base_cmd[2:]], cwd=ROOT, stderr=subprocess.DEVNULL)
    except subprocess.CalledProcessError:
        out = subprocess.check_output(base_cmd, cwd=ROOT)
    meta = json.loads(out)
    by_lic: dict[str, list[str]] = {}
    violations: list[str] = []
    for pkg in meta["packages"]:
        lic = pkg.get("license") or pkg.get("license_file") or "UNKNOWN"
        entry = f"{pkg['name']} {pkg['version']}"
        by_lic.setdefault(lic, []).append(entry)
        if any(tok in lic.upper() for tok in FORBIDDEN_TOKENS) or lic == "UNKNOWN":
            violations.append(f"cargo: {entry} -> {lic}")
    print(f"[cargo] checked {sum(len(v) for v in by_lic.values())} packages across {len(by_lic)} license expressions")
    for k, v in sorted(by_lic.items()):
        print(f"  {k}: {len(v)} pkg(s)")
    return violations


def check_npm() -> list[str]:
    patterns = [
        str(ROOT / "app/node_modules/*/package.json"),
        str(ROOT / "app/node_modules/@*/*/package.json"),
    ]
    files = sorted({p for pat in patterns for p in glob.glob(pat)})
    by_lic: dict[str, list[str]] = {}
    violations: list[str] = []
    for pj in files:
        data = json.loads(Path(pj).read_text(encoding="utf-8"))
        lic = str(data.get("license") or "UNKNOWN")
        entry = f"{data.get('name')} {data.get('version')}"
        by_lic.setdefault(lic, []).append(entry)
        if any(tok in lic.upper() for tok in FORBIDDEN_TOKENS) or lic == "UNKNOWN":
            violations.append(f"npm: {entry} -> {lic}")
    print(f"[npm] checked {len(files)} packages across {len(by_lic)} license expressions")
    for k, v in sorted(by_lic.items()):
        print(f"  {k}: {len(v)} pkg(s)")
    return violations


def main() -> int:
    violations = check_cargo() + check_npm()
    if violations:
        print("\nLICENSE CHECK FAILED:", file=sys.stderr)
        for v in violations:
            print(f"  - {v}", file=sys.stderr)
        return 1
    print("\nOK: All Cargo and npm dependencies are compatible with MIT.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
