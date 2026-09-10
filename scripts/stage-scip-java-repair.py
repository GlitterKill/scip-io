"""Stage the reviewed, hash-pinned path backport; never upload or publish it."""
import argparse
import hashlib
from pathlib import Path
import shutil
import subprocess

VERSION = "scip-java-v0.12.3-scip-io.1"
HASHES = {
    "scip-java": "76b02676b1fceea4627faca0c236213c98595f425af9052f1b0cfb04d2b4d0e2",
    "scip-java.bat": "843319e0a3c57e588a0dd25edd2fee4621ec9cd152741d3128a5f5366264a593",
}
BASE = "4486a05471000ba4e784b72395ebf84207f24af8"
ROOT = Path(__file__).resolve().parents[1]


def verify(path, expected):
    with path.open("rb") as source:
        actual = hashlib.file_digest(source, "sha256").hexdigest()
    if actual != expected:
        raise ValueError(f"SHA-256 mismatch: {path}: {actual}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--launcher-dir", type=Path, required=True)
    parser.add_argument("--upstream-checkout", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=ROOT / "target" / VERSION)
    args = parser.parse_args()
    # Reject mismatched inputs before creating any release files.
    rust_pins = (ROOT / "crates/scip-io-core/src/indexer/scip_java.rs").read_text()
    for value in [VERSION, *HASHES.values()]:
        if value not in rust_pins:
            raise ValueError("Installer pins and release staging pins differ")
    for name, checksum in HASHES.items():
        verify(args.launcher_dir / name, checksum)
    license_bytes = subprocess.check_output(
        ["git", "-C", str(args.upstream_checkout), "show", f"{BASE}:LICENSE"]
    )
    args.output.mkdir(parents=True, exist_ok=False)
    for name, checksum in HASHES.items():
        shutil.copyfile(args.launcher_dir / name, args.output / name)
        verify(args.output / name, checksum)
    (args.output / "LICENSE-scip-java.txt").write_bytes(license_bytes)
    shutil.copyfile(ROOT / "docs/patches/scip-java-gradle-paths.patch",
                    args.output / "scip-java-gradle-paths.patch")
    (args.output / "README.md").write_text(
        f"# {VERSION}\n\n"
        f"Path-serialization backport of scip-java v0.12.3, base `{BASE}`.\n\n"
        "Only the embedded GradleBuildTool class changes. The batch launcher and "
        "other embedded dependencies are unchanged. This is not a complete sbt rebuild "
        "and does not include Kotlin compiler compatibility fixes.\n\n"
        "The source change and regression are in [the patch](scip-java-gradle-paths.patch). "
        "See [the license](LICENSE-scip-java.txt) and [checksums](SHA256SUMS.txt).\n\n"
        f"[Distribution and verification guide](https://github.com/GlitterKill/scip-io/blob/{VERSION}/docs/scip-java-distribution.md).\n",
        encoding="utf-8",
    )
    files = sorted(args.output.iterdir())
    with (args.output / "SHA256SUMS.txt").open("w", encoding="utf-8", newline="\n") as sums:
        for path in files:
            with path.open("rb") as source:
                checksum = hashlib.file_digest(source, "sha256").hexdigest()
            sums.write(f"{checksum}  {path.name}\n")
    print(f"Staged {VERSION} at {args.output}; nothing published")


if __name__ == "__main__":
    main()
