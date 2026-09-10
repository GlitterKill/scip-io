# SCIP-IO scip-java distributions

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

## Kotlin compatibility distribution

SCIP-IO 0.2.2 automatically installs the published
`scip-java-v0.12.3-scip-io.2` distribution. It rebuilds the upstream v0.12.3
pack with the Windows path repair, Kotlin 2.3.21 compiler compatibility,
SemanticDB source-range repair, and Gradle Kotlin DSL script-indexing patches.
It retains the same installer asset names, `scip-java` and `scip-java.bat`, but
uses its own versioned directory and tag.

| Asset | SHA-256 |
| --- | --- |
| `scip-java` | `6a348ada3570002344305e3cf20bc61c94e88a2f6220c298ef81b3980ab1f662` |
| `scip-java.bat` | `843319e0a3c57e588a0dd25edd2fee4621ec9cd152741d3128a5f5366264a593` |

The package embeds the tested
`semanticdb-kotlinc:0.5.1-kotlin-2.3.21-scripts4-SNAPSHOT` JAR with SHA-256
`79e73306593b97ac87ab656ed072e3c3548514d32bfa1b7d8da35c056238c751`.
The plugin therefore does not use `mavenLocal()` or need `JAVA_TOOL_OPTIONS` to
resolve that unpublished snapshot at index time. Gradle can still resolve the
indexed project's ordinary dependencies from its configured repositories. The
source patches remain separate upstream-submission candidates.

The compatibility evidence covers Moshi with project Kotlin 2.3.21 and Gradle
9.5.1. It does not certify every Kotlin or Gradle version, and it does not make
a claim about a completed benchmark. `.1` remains immutable and continues to
serve released SCIP-IO 0.2.1 binaries.

## Installation and migration

Released SCIP-IO 0.2.1 binaries select `.1`; SCIP-IO 0.2.2 selects the
published `.2` payload. Native installation and Linux backend downloads share
the selected payload and pin. Version resolution does not follow the
application's latest GitHub release. Explicit configured CLI indexer binaries
continue to bypass automatic installation.

Files live under `<managed-bin>/scip-java-v0.12.3-scip-io.1/` for 0.2.1 or
`<managed-bin>/scip-java-v0.12.3-scip-io.2/` for 0.2.2. The old unversioned
cache and system PATH cannot satisfy either pin. They remain untouched. Windows
requires both the companion payload and batch launcher. Downloads are staged
together, checked against the compiled-in hashes, then published with a
directory rename. Failed downloads do not publish a partial installation.
Subsequent native installations verify the cached bytes before execution.

A checksum mismatch is an error, never a fallback to the old launcher. If a
versioned cache was manually changed or truncated, restore the exact release
files or remove that version directory and retry. Automatic uninstall removes
the new managed version only; it does not delete the retained unversioned copy.

## Staging

The `.1` repair script and its command below are historical. They stage the
released path-only payload:

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

The current `.2` staging command uses the reviewed `.1` launcher pair as the
Coursier bootstrap and the rebuilt pack as its classpath:

```powershell
python scripts/stage-scip-java-compat.py --launcher-dir target/scip-java-v0.12.3-scip-io.1 --pack target/scip-java-distribution-src/scip-java/target/pack
```

The script refuses an unexpected `.1` bootstrap, pack, or installer pin. It
stages the flat source patches, `LICENSE-scip-java.txt`, `PACK-SHA256SUMS.txt`,
and `SHA256SUMS.txt` with the installer pair. Restaging a reviewed pack produces
the reviewed `.2` payload. A source rebuild can produce different JAR bytes, so
review the pack and update the checksums and pins before staging it.

## Rebuild the pack

Start with a clean scip-java v0.12.3 source checkout. Apply the Windows path,
Gradle Kotlin DSL, and local `scripts4` integration patches, then apply
`scip-java-bundled-kotlin.patch`. Verify the `scripts4` JAR hash above and copy
it into both bundled-resource locations before creating the pack:

```powershell
git -C <scip-java> apply <scip-io>/docs/patches/scip-java-gradle-paths.patch
git -C <scip-java> apply <scip-io>/docs/patches/scip-java-gradle-kotlin-dsl.patch
git -C <scip-java> apply <scip-io>/docs/patches/local/scip-java-kotlin-2.3.21-scripts.patch
git -C <scip-java> apply <scip-io>/docs/patches/scip-java-bundled-kotlin.patch
Copy-Item <scripts4-jar> <scip-java>/scip-java/src/main/resources/semanticdb-kotlinc.jar
Copy-Item <scripts4-jar> <scip-java>/semanticdb-gradle-plugin/src/main/resources/semanticdb-kotlinc.jar
Set-Location <scip-java>
sbt 'set ThisBuild / version := "0.12.3-scip-io.2"' 'cli/pack'
```

The bundled-Kotlin patch removes unpublished plugin resolution from Gradle
source compilation, Gradle script compilation, and CLI dependency compilation.
The resulting pack reports the `.2` version string. It removes only the plugin
resolution requirement; Gradle still resolves the indexed project's ordinary
dependencies normally.

## Fresh validation

The reviewed pack was staged twice. The re-staged `scip-java` hash matches the
`.2` payload pin above, and the packaged launcher reports a `.2` version. Five
installer tests include real staged assets. Focused Rust validation also passes
17 registry tests, 12 managed-indexer tests, and 29 CLI tests, for 63 tests in
total. `cargo fmt --all --check` and `git diff --check` pass.

Run the current installer smoke check after staging the default `.2` directory:

```powershell
$env:SCIP_JAVA_REPAIR_TEST_ASSETS = (Resolve-Path target/scip-java-v0.12.3-scip-io.2).Path
cargo test -p scip-io-core indexer::scip_java::tests -- --include-ignored
python scripts/check-scip-java-compat.py target/scip-java-v0.12.3-scip-io.2/scip-java
```

The packaged launcher passes the existing fresh-cache, warm-cache, and invalid
Gradle Kotlin DSL script regression with an empty Maven repository:

```powershell
./target/scip-java-distribution-src/tests/gradle-kotlin-dsl/run.ps1 -Pack target/scip-java-compat-test-pack -GradleHome <Gradle-9.5.1> -MavenRepository target/scip-java-compat-empty-maven
```

The test pack's `bin` directory contains the staged installer pair. Its `lib`
directory supplies `scip-java-proto` and protobuf JARs only to the validator.
The empty Maven directory remains empty after the run.

Fresh SCIP-IO Java and Kotlin runs on the disposable pinned Moshi checkout both
succeed with zero failed languages and `partial: false`. Each output has 231
documents, including 13 Gradle scripts, 80,852 occurrences, and 14,596
cross-file resolved references. All 183 archived Moshi source files remain
unchanged. The validator confirms document paths and occurrence ranges. Retained
logs are under
`target/scip-java-compat-{installer,scripts,moshi-java,moshi-kotlin}` and
`target/scip-java-compat-validation/{results,counts}.log`.

This validation does not establish a completed benchmark.

The published `.2` assets use the separate
`scip-java-v0.12.3-scip-io.2` tag. Do not use the application release workflow
for this tag. Do not replace an existing tag's payload with different bytes;
use a new distribution version and update the pins instead.
