"""Fetch any crates locked in Cargo.lock that are not in the local Cargo cache.

This is a build-environment helper for slow/flaky crates.io connectivity.
It only writes verified .crate archives into Cargo's cache directory.
"""

from __future__ import annotations

import hashlib
import os
import sys
import tomllib
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LOCK = ROOT / "src-tauri" / "Cargo.lock"
CACHE = Path(os.environ["USERPROFILE"]) / ".cargo" / "registry" / "cache" / "index.crates.io-1949cf8c6b5b557f"
MIRRORS = [
    "https://mirrors.ustc.edu.cn/crates.io/api/v1/crates/{name}/{version}/download",
    "https://rsproxy.cn/api/v1/crates/{name}/{version}/download",
    "https://static.crates.io/crates/{name}/{name}-{version}.crate",
]


def download(package: dict) -> tuple[str, str | None]:
    name = package["name"]
    version = package["version"]
    checksum = package.get("checksum")
    target = CACHE / f"{name}-{version}.crate"
    label = f"{name} {version}"
    if target.exists():
        return label, None
    last_error = None
    for template in MIRRORS:
        url = template.format(name=name, version=version)
        try:
            with urllib.request.urlopen(url, timeout=60) as response:
                data = response.read()
            digest = hashlib.sha256(data).hexdigest()
            if checksum and digest != checksum:
                last_error = f"checksum mismatch from {url}"
                continue
            temp = target.with_suffix(".crate.part")
            temp.write_bytes(data)
            temp.replace(target)
            return label, None
        except Exception as error:  # noqa: BLE001 - report network failure
            last_error = f"{url}: {error}"
    return label, last_error or "unknown download failure"


def main() -> int:
    CACHE.mkdir(parents=True, exist_ok=True)
    lock = tomllib.loads(LOCK.read_text(encoding="utf-8"))
    packages = [
        package
        for package in lock.get("package", [])
        if str(package.get("source", "")).startswith("registry+")
    ]
    missing = [
        package
        for package in packages
        if not (CACHE / f"{package['name']}-{package['version']}.crate").exists()
    ]
    print(f"locked registry packages: {len(packages)}; cached: {len(packages) - len(missing)}; missing: {len(missing)}")
    if not missing:
        return 0

    failures = 0
    with ThreadPoolExecutor(max_workers=4) as executor:
        futures = [executor.submit(download, package) for package in missing]
        for future in as_completed(futures):
            label, error = future.result()
            if error:
                failures += 1
                print(f"FAIL {label}: {error}")
            else:
                print(f"OK   {label}")

    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
