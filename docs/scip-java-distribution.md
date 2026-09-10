# SCIP-IO scip-java path backport

## Distribution contract

SCIP-IO 0.2.1 selects `scip-java-v0.12.3-scip-io.1` from
`GlitterKill/scip-io`. This is a separate indexer release tag, not a SCIP-IO
application version. The indexer artifacts are distributed separately from the application binaries.
Existing SCIP-IO 0.2.0 binaries retain their original installation behavior.

The portable Coursier payload is the previously verified Windows path backport:

| Asset | SHA-256 |
| --- | --- |
| `scip-java` | `76b02676b1fceea4627faca0c236213c98595f425af9052f1b0cfb04d2b4d0e2` |
| `scip-java.bat` | `843319e0a3c57e588a0dd25edd2fee4621ec9cd152741d3128a5f5366264a593` |

The payload changes only the compiled `GradleBuildTool` class from upstream
scip-java v0.12.3 (commit `4486a05471000ba4e784b72395ebf84207f24af8`).
[The source patch and regression](patches/scip-java-gradle-paths.patch) serialize
all five paths as Groovy literals. This artifact is a class backport, not a
complete sbt rebuild. Its embedded upstream version still reports 0.12.3;
SCIP-IO records the distinct distribution tag in managed-install metadata.
The upstream Apache-2.0 license accompanies the release.

This release does **not** contain the later Kotlin 2.3.21 compatibility,
source-range, or Gradle Kotlin DSL patches. It fixes the initialization-script
syntax error; it does not establish successful Moshi indexing or a valid token
comparison. See [the investigation](windows-scip-java-gradle.md).

## Installation and migration

Default Java, Kotlin, and Scala indexing uses the pinned release. Native
installation and Linux backend downloads share the same payload and pin.
Version resolution does not follow the application's latest GitHub release.
Explicit configured CLI indexer binaries continue to bypass automatic installation.

Files live under `<managed-bin>/scip-java-v0.12.3-scip-io.1/`. The old unversioned
cache and system PATH cannot satisfy this default pin. They remain untouched.
Windows requires both the companion payload and batch launcher. Downloads are
staged together, checked against the compiled-in hashes, then published with a
directory rename. Failed downloads do not publish a partial installation.
Subsequent native installations verify the cached bytes before execution.

A checksum mismatch is an error, never a fallback to the old launcher. If a
versioned cache was manually changed or truncated, restore the exact release
files or remove that version directory and retry. Automatic uninstall removes
the new managed version only; it does not delete the retained unversioned copy.

## Release preparation

Run this from the SCIP-IO checkout, supplying the reviewed payload directory and
an upstream checkout containing the base commit:

```powershell
python scripts/stage-scip-java-repair.py --launcher-dir <reviewed-launcher-directory> --upstream-checkout <scip-java-checkout>
$env:SCIP_JAVA_REPAIR_TEST_ASSETS = (Resolve-Path target/scip-java-v0.12.3-scip-io.1).Path
cargo test -p scip-io-core real_artifact_download_install_and_cache_smoke -- --ignored
```

Staging refuses to overwrite an output directory or accept different payload
bytes. It includes the patch, upstream license, provenance notes, and checksums.
The smoke check serves the real assets on localhost, installs both Windows
files into a fresh cache, verifies offline reuse, and rejects a modified batch
launcher. It does not access or replace the user's installation.

To repeat source verification, apply the patch to a clean upstream base and
use its `tests/gradle-init-script/run.ps1` regression. First test the original
launcher (expected failure), then compile the patched source (expected pass).
To check the packaged artifact itself, use `-Original` with the repaired
launcher: that switch omits source recompilation and tests the embedded class.

After approval, publish all staged files under the exact separate tag above.
Do not use the application release workflow for this tag. Download the public
assets and verify their hashes before releasing the consuming SCIP-IO version.
Do not replace an existing tag's payload with different bytes; use a new
distribution version and update the pins instead.
