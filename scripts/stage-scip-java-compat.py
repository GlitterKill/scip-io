"""Stage the Kotlin compatibility distribution from a reviewed sbt pack; never publish."""
import argparse
import hashlib
from pathlib import Path
import re
import runpy
import shutil
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]
VERSION = "scip-java-v0.12.3-scip-io.2"
BOOTSTRAP_SHA256 = "76b02676b1fceea4627faca0c236213c98595f425af9052f1b0cfb04d2b4d0e2"
LAUNCHER_SHA256 = "843319e0a3c57e588a0dd25edd2fee4621ec9cd152741d3128a5f5366264a593"
PATCHES = [
    "scip-java-gradle-paths.patch", "semanticdb-kotlinc-2.3.21.patch",
    "semanticdb-kotlinc-source-ranges.patch", "semanticdb-kotlinc-scripts.patch",
    "scip-java-gradle-kotlin-dsl.patch", "local/scip-java-kotlin-2.3.21-scripts.patch",
    "scip-java-bundled-kotlin.patch",
]


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def verify(path, expected):
    if digest(path) != expected:
        raise ValueError(f"SHA-256 mismatch: {path}")


def build_payload(bootstrap, pack, output):
    verify(bootstrap, BOOTSTRAP_SHA256)
    jars = sorted((pack / "lib").glob("*.jar"))
    if not jars:
        raise ValueError("Missing sbt pack libraries")
    prefix = "coursier/bootstrap/launcher/"
    # Retain the verified native shell/Java bootstrap; replace its whole classpath.
    with zipfile.ZipFile(bootstrap) as original:
        offset = min(info.header_offset for info in original.infolist())
        with bootstrap.open("rb") as source:
            output.write_bytes(source.read(offset))
        with zipfile.ZipFile(output, "a", compression=zipfile.ZIP_STORED) as archive:
            def write(name, data):
                info = zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0))
                info.external_attr = 0o644 << 16
                archive.writestr(info, data)

            for name in original.namelist():
                if name.startswith(prefix + "jars/") or name == prefix + "bootstrap-jar-resources":
                    continue
                write(name, original.read(name))
            write(prefix + "bootstrap-jar-resources", "\n".join(p.name for p in jars).encode())
            for jar in jars:
                write(prefix + "jars/" + jar.name, jar.read_bytes())
    runpy.run_path(str(ROOT / "scripts/check-scip-java-compat.py"))["check"](output)
    return jars


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--launcher-dir", type=Path, required=True, help="Verified .1 release pair")
    parser.add_argument("--pack", type=Path, required=True, help="Rebuilt sbt pack containing bundled plugin resources")
    parser.add_argument("--output", type=Path, default=ROOT / "target" / VERSION)
    args = parser.parse_args()
    if args.output.exists():
        raise FileExistsError(args.output)
    verify(args.launcher_dir / "scip-java.bat", LAUNCHER_SHA256)
    pins = (ROOT / "crates/scip-io-core/src/indexer/scip_java.rs").read_text()
    expected = re.search(r'PAYLOAD_SHA256: &str =\s*"([a-f0-9]{64})"', pins).group(1)
    if VERSION not in pins or LAUNCHER_SHA256 not in pins:
        raise ValueError("Installer and staging pins differ")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    # Publish the local staging directory only after every check succeeds.
    with tempfile.TemporaryDirectory(dir=args.output.parent) as scratch:
        stage = Path(scratch) / VERSION
        stage.mkdir()
        jars = build_payload(args.launcher_dir / "scip-java", args.pack, stage / "scip-java")
        verify(stage / "scip-java", expected)
        shutil.copyfile(args.launcher_dir / "scip-java.bat", stage / "scip-java.bat")
        shutil.copyfile(args.launcher_dir / "LICENSE-scip-java.txt", stage / "LICENSE-scip-java.txt")
        for name in PATCHES:
            shutil.copyfile(ROOT / "docs/patches" / name, stage / Path(name).name)
        (stage / "PACK-SHA256SUMS.txt").write_text(
            "".join(f"{digest(jar)}  {jar.name}\n" for jar in jars), encoding="utf-8", newline="\n")
        (stage / "README.md").write_text(
            f"# {VERSION}\n\n"
            "Source-built scip-java v0.12.3 with the Windows path, Kotlin 2.3.21, "
            "source-range, and Gradle Kotlin DSL backports. The exact tested scripts4 "
            "SemanticDB plugin is embedded in both CLI and Gradle resources; no unpublished "
            "Maven artifact or local repository setting is needed to load it.\n\n"
            "SCIP-IO verifies both installer assets against compiled-in SHA-256 pins. "
            "PACK-SHA256SUMS.txt identifies every input jar. Source patches and the "
            "upstream Apache-2.0 license accompany these assets. This is a separate "
            "distribution; the .1 release remains unchanged.\n\n"
            "Verified compiler scope: project Kotlin 2.3.21 and Gradle 9.5.1's Kotlin "
            "2.3.20. Other compiler versions require separate qualification.\n",
            encoding="utf-8", newline="\n")
        files = sorted(stage.iterdir())
        (stage / "SHA256SUMS.txt").write_text(
            "".join(f"{digest(p)}  {p.name}\n" for p in files), encoding="utf-8", newline="\n")
        stage.rename(args.output)
    print(f"Staged {VERSION} at {args.output}; nothing published")


if __name__ == "__main__":
    main()
