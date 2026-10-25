#!/usr/bin/env python3
"""Verify remote Cargo reuse and invalidation across independent consumers."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


def run(arguments, environment, directory=None):
    result = subprocess.run(
        arguments, cwd=directory, env=environment, text=True,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=120,
    )
    if result.returncode:
        raise RuntimeError(f"{arguments[0]} failed:\n{result.stdout}{result.stderr}")
    return result.stdout


def check(root, environment, sccache):
    library = root / "shared"
    (library / "src").mkdir(parents=True)
    (library / "Cargo.toml").write_text(
        '[package]\nname = "idle-cache-shared"\nversion = "1.0.0"\n'
        'edition = "2021"\n[features]\nalternate = []\n'
    )
    (library / "build.rs").write_text(
        'fn main() {\n'
        '    let value = std::fs::read_to_string("input.txt").unwrap();\n'
        '    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());\n'
        '    std::fs::write(output.join("value.rs"), format!("const VALUE: u32 = {};", value)).unwrap();\n'
        '    println!("cargo:rerun-if-changed=input.txt");\n}\n'
    )
    for name in ["producer", "consumer"]:
        directory = root / name
        (directory / "src").mkdir(parents=True)
        (directory / "Cargo.toml").write_text(
            f'[package]\nname = "{name}"\nversion = "1.0.0"\nedition = "2021"\n'
            '[dependencies]\nidle-cache-shared = { path = "../shared" }\n'
            '[features]\nalternate = ["idle-cache-shared/alternate"]\n'
        )
        (directory / "src/main.rs").write_text(
            'fn main() { println!("{}", idle_cache_shared::value()); }\n'
        )
    cases = [
        ("first consumer", "producer", "7", 0, [], "", "7", 0, 1),
        ("different repository", "consumer", "7", 0, [], "", "7", 1, 0),
        ("changed generated input", "consumer", "9", 0, [], "", "9", 0, 1),
        ("changed source file", "consumer", "7", 10, [], "", "17", 0, 1),
        ("different features", "consumer", "7", 0, ["--features", "alternate"], "", "107", 0, 1),
        ("different compiler flags", "consumer", "7", 0, [], "-Copt-level=1", "7", 0, 1),
        ("restored inputs", "consumer", "7", 0, [], "", "7", 1, 0),
    ]
    results = []
    for label, name, value, adjustment, arguments, flags, expected, hits, misses in cases:
        build = root / "build"
        if build.exists():
            shutil.rmtree(build)
        (library / "input.txt").write_text(value)
        (library / "src/lib.rs").write_text(
            'include!(concat!(env!("OUT_DIR"), "/value.rs"));\n'
            f'pub fn value() -> u32 {{ VALUE + {adjustment} + if cfg!(feature = "alternate") {{ 100 }} else {{ 0 }} }}\n'
        )
        selected = dict(environment, RUSTFLAGS=flags)
        run([sccache, "--zero-stats"], selected)
        run(["cargo", "build", "--offline", *arguments], selected, root / name)
        actual = run([str(root / name / "target/debug" / name)], selected).strip()
        report = json.loads(run([sccache, "--show-stats", "--stats-format=json"], selected))
        stats = report["stats"]
        observed_hits = stats["cache_hits"]["counts"].get("Rust", 0)
        observed_misses = stats["cache_misses"]["counts"].get("Rust", 0)
        if (actual, observed_hits, observed_misses) != (expected, hits, misses):
            raise RuntimeError(f"{label}: output={actual}, hits={observed_hits}, misses={observed_misses}")
        if "webdav" not in report["cache_location"].lower():
            raise RuntimeError("the compiler did not use the shared remote cache")
        if stats["cache_read_errors"] or stats["cache_write_errors"] or stats["cache_errors"]["counts"]:
            raise RuntimeError(f"{label}: the cache reported an error")
        results.append({"case": label, "output": actual, "hits": hits, "misses": misses})
        # Restart the client between builds so only the remote store can supply a hit.
        run([sccache, "--stop-server"], selected)
    return results


def main():
    if not os.environ.get("SCCACHE_WEBDAV_ENDPOINT") or not os.environ.get("CARGO_BUILD_BUILD_DIR"):
        raise SystemExit("the runner must configure the shared Cargo cache and build directory")
    sccache = shutil.which("sccache")
    if sccache is None:
        raise SystemExit("sccache is not installed")
    with tempfile.TemporaryDirectory(prefix="idle-cargo-cache-") as temporary:
        root = Path(temporary)
        environment = dict(
            os.environ, RUSTC_WRAPPER=sccache, CARGO_INCREMENTAL="0",
            CARGO_BUILD_BUILD_DIR=str(root / "build"), SCCACHE_SERVER_UDS=str(root / "server.sock"),
        )
        environment.pop("CARGO_TARGET_DIR", None)
        environment.pop("CARGO_ENCODED_RUSTFLAGS", None)
        try:
            results = check(root, environment, sccache)
        finally:
            subprocess.run([sccache, "--stop-server"], env=environment, capture_output=True, timeout=15)
        print(json.dumps({"result": "success", "cases": results}, indent=2))


if __name__ == "__main__":
    main()
