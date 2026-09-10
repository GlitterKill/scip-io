# Windows scip-java Gradle investigation

## Managed distribution in SCIP-IO 0.2.1

SCIP-IO 0.2.1 automatically installs the path-serialization
repair as `scip-java-v0.12.3-scip-io.1` from
[`GlitterKill/scip-io`](https://github.com/GlitterKill/scip-io). SCIP-IO v0.2.0 binaries still use their existing
`scip-java` behavior.

The managed payload is portable: non-Windows platforms install
`bin/scip-java-v0.12.3-scip-io.1/scip-java`, while Windows also installs
`bin/scip-java-v0.12.3-scip-io.1/scip-java.bat`. SCIP-IO downloads the same
pinned payload for native platforms and WSL, verifies the payload SHA-256
`76b02676b1fceea4627faca0c236213c98595f425af9052f1b0cfb04d2b4d0e2`, and on
Windows verifies the launcher SHA-256
`843319e0a3c57e588a0dd25edd2fee4621ec9cd152741d3128a5f5366264a593`. It checks
those hashes both after download and whenever it reuses the managed cache.

For the default command, SCIP-IO does not fall back to an older unmanaged cache
entry or a `scip-java` found on `PATH`. An explicit CLI configured `binary` still
overrides the managed repair. The release contains only the original Windows
path-serialization repair. It does not distribute the later Kotlin
compatibility, source-range, or Gradle-script patches, and this release
does not establish a new full-project benchmark result. See the
[scip-java distribution guide](scip-java-distribution.md) for installation
behavior.

## Historical investigation and verification

The later [Gradle Kotlin DSL coverage follow-up](patches/README.md#gradle-kotlin-dsl-follow-up)
adds compiler-backed build/settings scripts, source-range repairs, and typed script
locals. The historical results below retain their original verification context;
they do not describe the managed path-only payload.

## Reproduction

Verified on Windows with scip-io 0.1.9, scip-java 0.12.3, Gradle 9.5.1 and
Eclipse Adoptium JDK 21.0.11. Moshi is pinned to
`889013ec2edb8d8034902662a1dc8c4f3b3f8111`; its version catalog selects Kotlin
2.3.21. All indexing runs use a disposable clone. Its tracked files remain
unchanged.

Preserve `OS=Windows_NT` in the child environment. Without it, the batch
launcher enters its legacy argument-shifting loop, which is a separate earlier
failure. Do not change TEMP, normalize the repository path, or treat fallback
graphs as successful generation.

## Ownership and data flow

1. SCIP-IO's `crates/scip-io-core/src/indexer/registry.rs` maps Java to
   `scip-java` and Kotlin to the same runnable entry, with default arguments
   `index`. `indexer/runner.rs::run_indexer_to_temp_output_with_args` prepares
   the command, passes arguments individually, and rejects nonzero generator
   exits before publishing output.
2. The Windows `scip-java.bat` launcher invokes its companion Coursier payload.
   In upstream `scip-java`, `GradleBuildTool.generateSemanticdb` calls
   `TemporaryFiles.withDirectory`, which uses `Files.createTempDirectory`.
   `Embedded.gradlePluginJar`, `semanticdbJar` and `agentJar` copy embedded jars
   into that directory.
3. `GradleBuildTool.initScript` interpolates those paths, `targetroot`, and
   `targetroot.resolve("dependencies.txt")` into double-quoted Groovy strings
   and writes UTF-8 bytes. `runCompileCommand` passes the resulting file to
   Gradle using `--init-script`.

The defect belongs to **upstream scip-java's source serialization**, not the
SCIP-IO process boundary or Gradle's parser. The upstream base is tag `v0.12.3`,
commit `4486a05471000ba4e784b72395ebf84207f24af8`.

The actual saved file, inspected directly rather than through JSON logs,
contains this line before the fix:

```groovy
classpath(files("C:\Users\glitt\AppData\Local\Temp\scip-java4780957634735325984\gradle-plugin.jar"))
```

Those single backslashes are Groovy escape prefixes. Gradle reports
`Unexpected character: '"' @ line 3, column 26`. Spaces alone do not require
extra escaping inside a string; backslashes, double quotes, control characters,
and dollar interpolation do. See the [Groovy string syntax documentation](https://github.com/apache/groovy/blob/master/src/spec/doc/core-syntax.adoc).

## Patch and focused regression

[scip-java-gradle-paths.patch](patches/scip-java-gradle-paths.patch) contains:

- `scip-java/src/main/scala/com/sourcegraph/scip_java/buildtools/GradleBuildTool.scala`:
  encode all five paths with the existing `ujson.write` serializer, then escape
  dollar signs to prevent Groovy interpolation. No new production dependency.
- `tests/gradle-init-script/Regression.scala`: evaluate the complete generated
  script with Gradle's Groovy compiler and a stub Gradle DSL; compare all five
  resulting path values, then check six synthetic escaping cases.
- `tests/gradle-init-script/run.ps1`: compile against the dependencies embedded
  in the release launcher, using Scala 2.13.13 and Gradle's Groovy jar.
- `tests/gradle-init-script/README.md`: instructions and the Moshi limitation.

Apply the patch in a separate upstream checkout:

```powershell
git clone --depth 1 --branch v0.12.3 https://github.com/sourcegraph/scip-java.git scip-java-fix
git -C scip-java-fix apply F:/Claude/projects/scip-io/docs/patches/scip-java-gradle-paths.patch
Set-Location scip-java-fix
$launcher = "$env:LOCALAPPDATA/scip-io/bin/scip-java"
$gradleHome = 'C:/path/to/gradle-9.5.1'
./tests/gradle-init-script/run.ps1 -Launcher $launcher -GradleHome $gradleHome -Original
./tests/gradle-init-script/run.ps1 -Launcher $launcher -GradleHome $gradleHome
```

The original-release test fails on the generated Windows classpath string.
The patched-source test passes: all five path values and six escaping cases,
including spaces, both quote types, dollar signs, UNC/trailing backslashes and
control characters. Double quotes are tested without filesystem operations
because Windows forbids them in filenames. This is a standalone runnable
regression; it is not wired into sbt's default test task.

## Path-fix-only Moshi verification

The test artifact recompiles only `GradleBuildTool.scala` against the 0.12.3
release dependencies and replaces that class in a disposable copy of the
Coursier launcher. The embedded Gradle plugin and all dependency versions stay
unchanged. This is not a complete sbt release build.

The CLI in 0.1.9 ignores `indexer.<language>.binary` during execution:
`crates/scip-io-cli/src/cli/index.rs` calls `entry.ensure_installed()` directly.
Consequently, the configured disposable binary did not affect the first CLI
probe. For the definitive two runs, the companion payload was temporarily
substituted with a backup and `finally` restoration. The installed launcher
was restored and verified against its original SHA-256:
`2d4d8a31333dfa0daf3aa0381a51de465e40b0dac5622e49363786a65f743f34`.

The tested patched artifact SHA-256 is
`76b02676b1fceea4627faca0c236213c98595f425af9052f1b0cfb04d2b4d0e2`.

Both commands were invoked with Node `spawnSync`, `shell:false`, a scrubbed
environment preserving `OS=Windows_NT`, and a 660-second outer timeout:

```text
scip-io index --path <disposable-moshi> --lang java --no-merge --timeout 600 --format json
scip-io index --path <disposable-moshi> --lang kotlin --no-merge --timeout 600 --format json
```

| Check | Result |
| --- | --- |
| Original `scip-java index --no-cleanup` | Same initialization-script compilation failure |
| Patched direct scip-java | Script compiles; fails at `:moshi:compileKotlin` |
| Patched through scip-io, Java | Exit 2; 1 failed language; 0 successful outputs |
| Patched through scip-io, Kotlin | Exit 2; 1 failed language; 0 successful outputs |
| Usable SCIP files in disposable Moshi | None |

At this stage, the remaining failure was:

```text
AbstractMethodError: Missing implementation of resolved method
'abstract java.lang.String getPluginId()'
of abstract class org.jetbrains.kotlin.compiler.plugin.CompilerPluginRegistrar.
```

`semanticdb-gradle-plugin/src/main/scala/SemanticdbGradlePlugin.scala` selects
`com.sourcegraph:semanticdb-kotlinc:0.5.0` through generated `BuildInfo` from
`build.sbt`. `javap` confirms that the installed plugin's `AnalyzerRegistrar`
does not implement `getPluginId`. Both language selections build the mixed
Moshi project, so both hit the Kotlin compiler failure. This signature is also
reported in [upstream issue 864](https://github.com/scip-code/scip-java/issues/864).

The Maven Central `com.sourcegraph:semanticdb-kotlinc` metadata reports 0.6.0
as the latest release at verification time. A compatible replacement has not
been established; no dependency upgrade, compiler downgrade, skipped compile
task, or silent fallback was applied.

Local evidence is retained under
`C:/Users/glitt/AppData/Local/Temp/scip-java-windows-fix/.verification/`
(`runner-red.log`, `runner-green.log`, `moshi-results.log`, and
`verify-moshi.mjs`) and
`C:/Users/glitt/AppData/Local/Temp/moshi-scip-windows-repro/`
(`original-index.log`, `patched-direct.log`, `verified-java.json`, and
`verified-kotlin.json`). The upstream patch in this repository preserves the
fix and regression independently of these temporary artifacts.

## Kotlin compatibility fix and final verification

[semanticdb-kotlinc-2.3.21.patch](patches/semanticdb-kotlinc-2.3.21.patch) applies
to the `sourcegraph/scip-kotlin` tag `v0.5.0`, commit
`408df420cbf877c3babc3d7c73d54299f02c038d`. That repository supplied the plugin
bundled by scip-java 0.12.3. Its current development has moved into scip-java;
the newest release checked during this follow-up, scip-java 0.13.1, still uses
Kotlin 2.2.0 and lacks the registrar plugin ID. A release upgrade alone was
therefore not used as the fix.

The compatibility patch changes five production Kotlin files:

- `AnalyzerRegistrar.kt`: supply the plugin ID required by Kotlin 2.3.
- `AnalyzerCheckers.kt`: use context-parameter checker signatures and the
  current containing-file, containing-class and named-function APIs.
- `SymbolsCache.kt`: update containing-symbol lookup and preserve the earlier
  local/global classification, including global anonymous-object descriptors.
- `SemanticdbTextDocumentBuilder.kt`: update override lookup, handle property
  names without the nullable callable ID, close source streams, normalize
  rendered documentation to LF, and use the mapped end line for ranges.
- `LineMap.kt`: calculate an element's end line and column independently.
  Treating a multi-line companion declaration as one long line produced
  invalid ranges despite successful generation.

The patch also updates the three Gradle build scripts for Kotlin 2.3.21 and the
`compilerOptions` DSL, updates the existing test compiler dependency and helper
to the Kotlin 2.3 API, adds a regression to `AnalyzerTest.kt`, and updates the
upstream README. No existing behavioral assertions were removed. The full
suite exposed open source streams that prevented Windows temporary-directory
cleanup; closing the streams fixed that cause rather than disabling cleanup.

### Build and regression commands

Run from the patched scip-kotlin checkout with JDK 17+ for Gradle and the
existing JDK 8 compilation toolchain:

```powershell
$env:OS = 'Windows_NT'
./gradlew.bat --no-daemon '-Porg.gradle.java.installations.paths=C:/path/to/jdk8' :semanticdb-kotlinc:test :semanticdb-kotlinc:shadowJar
```

The full Gradle build and test suite pass: **81 tests, 58 passed, 23 existing
skips, zero failures or errors**. The new regression invokes the real compiler
and checks definitions, references and multi-line ranges. An independent
two-file compiler fixture also passed; the original plugin failed at loading,
and the initial compatibility-only patch failed the multi-line range check.

The final Moshi runs use the normal `shadowJar` build, not the earlier
class-overlay prototype. Its SHA-256 is
`7a1eb3478a56c72ddae1aa04e3bd2114b2f18749d6893c6235890e3c9108358e`.
The same Java and Kotlin scip-io commands shown above ran with `shell:false`,
`OS=Windows_NT`, Gradle 9.5.1 and the pinned, unchanged Moshi tracked sources.
Both report **exit 0, one successful output, zero failed languages**.
Both associated Gradle daemon logs report `BUILD SUCCESSFUL` and contain no
SemanticDB plugin exception or internal compiler error markers.

### SCIP content validation

Both language selections index the mixed JVM build, so each output contains
the same Java and Kotlin document set. They are not disjoint language shards.

| Content in each output | Java | Kotlin |
| --- | ---: | ---: |
| Documents | 57 | 161 |
| Symbol information records | 3,569 | 9,927 |
| Occurrences | 27,638 | 52,066 |
| Definitions | 3,568 | 10,051 |
| References | 24,070 | 42,015 |

The validator decodes the SCIP protobuf, checks unique repository-relative
document paths against existing source files, checks every occurrence range
against source line/column bounds, and requires nonempty symbols, definitions
and references in both languages. It also verifies **14,590 reference
occurrences resolve to definitions in another source file**. It does not
require external-library references to resolve inside Moshi.

The validated outputs, compiled plugin, full test log, invocation results and
`ValidateScip.java` are retained in the ignored local artifact directory
[`../target/moshi-kotlin-compat/`](../target/moshi-kotlin-compat/). To rerun the
validator, use scip-java 0.12.3's extracted dependency jars as the classpath:

```powershell
javac -cp '<release-jars>/*' target/moshi-kotlin-compat/ValidateScip.java
java -cp 'target/moshi-kotlin-compat;<release-jars>/*' ValidateScip '<disposable-moshi>' target/moshi-kotlin-compat/java.scip target/moshi-kotlin-compat/kotlin.scip
```

For isolated testing, the patched launcher and the cached Kotlin plugin jar
were temporarily substituted with backups and restored in `finally` blocks.
The restored Kotlin plugin SHA-256 is
`cd1fb423fae36244f72727875a72bcb4fcda217de5f71614aae3ebef4c51e7ef`;
the restored launcher hash matches the original recorded above. The harness
is retained at
`C:/Users/glitt/AppData/Local/Temp/semanticdb-kotlinc-windows-fix/.verification/verify-moshi.mjs`.

Compatibility is verified for Kotlin **2.3.21**, not every Kotlin release.
Do not replace the published Kotlin 2.1.20 artifact globally with this build.
At that stage the scip-io custom-binary override limitation remained separate.
The follow-up below fixes it. No persistent installation change, commit, push
or remote publication was made.

## Versioned integration without cache substitution

The follow-up fixes the formatter and connects the compatibility artifact to
scip-java through its normal dependency resolution:

1. `semanticdb-kotlinc-2.3.21.patch` now pins Spotless 7.0.4 and ktfmt 0.55,
   using Kotlin style because modern ktfmt removed Dropbox style. Formatting
   changes outside the compatibility methods are formatter output. Upstream CI
   and release workflows install JDK 8 and 21, run Gradle on 21, and CI runs
   `spotlessCheck test`. The patch applies cleanly to the same v0.5.0 base.
2. The local-only `patches/local/scip-java-kotlin-2.3.21.patch` changes `V.semanticdbKotlin` to
   `0.5.1-kotlin-2.3.21-SNAPSHOT` and `V.kotlinVersion` to `2.3.21`. This updates
   generated BuildInfo for both the CLI and embedded Gradle plugin, and keeps
   the bundled compiler aligned with the plugin. For disposable verification,
   apply it alongside `scip-java-gradle-paths.patch` to scip-java v0.12.3.
   Exclude this local dependency pin from upstream submissions: fresh users
   cannot resolve the unpublished snapshot. Keep the compiler/plugin bump
   together until a published artifact is available. See [patch scope](patches/README.md).
3. SCIP-IO native CLI indexing now honors the existing `binary` config field.
   Relative paths resolve from the loaded config root; missing files fail
   explicitly. JSON and text dry-run commands display that same configured
   executable, quoted to distinguish paths containing spaces. Relative paths
   use the loaded config root even when indexing nested projects. The regression
   covers execution selection, dry-run display, indexer-name lookup, the default
   executable, and rejection of missing files.

The new artifact is staged locally using the existing unsigned snapshot task:

```powershell
# In the patched scip-kotlin checkout (JDK 21 runs Gradle).
$env:OS = 'Windows_NT'
./gradlew.bat --no-daemon '-Porg.gradle.java.installations.paths=C:/path/to/jdk8' '-Pversion=0.5.1-kotlin-2.3.21-SNAPSHOT' spotlessCheck test
./gradlew.bat --no-daemon '-Porg.gradle.java.installations.paths=C:/path/to/jdk8' '-Pversion=0.5.1-kotlin-2.3.21-SNAPSHOT' '-Dmaven.repo.local=C:/path/to/disposable-maven' :semanticdb-kotlinc:publishShadowPublicationToMavenLocal

# In the patched scip-java checkout, using sbt 1.11.3.
java -jar .verification/sbt-launch.jar 'cli/pack'
```

No signing key is needed for the snapshot; no remote publish task runs. The
full scip-java build produces `scip-java/target/pack/bin/scip-java.bat` and its
library directory. No embedded class or jar was manually replaced in this run.
The staged plugin SHA-256 is
`94ae99ffbb72cf9a6436839db564573faf153a200608e630cfc43465d3304929`.

In the disposable pinned Moshi checkout, both `[indexer.java]` and
`[indexer.kotlin]` select that absolute `.bat` path and use
`args = ['index', '--no-cleanup']`. The locally built SCIP-IO CLI runs:

```powershell
$env:OS = 'Windows_NT'
$env:JAVA_TOOL_OPTIONS = '-Dmaven.repo.local=C:/path/to/disposable-maven'
scip-io index --path <disposable-moshi> --lang java --no-merge --timeout 600 --format json
scip-io index --path <disposable-moshi> --lang kotlin --no-merge --timeout 600 --format json
```

The retained harness invokes the CLI through Node `spawnSync` with `shell:false`
and the scrubbed environment plus those two explicit variables. The Maven
property directs the existing `mavenLocal()` resolver to the disposable
repository. Gradle daemon logs confirm the property and successful compilation;
neither log contains SemanticDB plugin exceptions or compiler failure markers.
Old output files are removed before each generation to exclude stale success.

Both commands report `successful_outputs: 1`, `failed_languages: 0`, and
`partial: false`. Fresh protobuf validation passes for both outputs, each with
57 Java documents, 161 Kotlin documents, 79,704 occurrences, and 14,590 cross-file
resolved references. All occurrence ranges are within the source files.
Moshi remains at the pinned commit with no tracked changes. Before/after hashes
of the installed launcher and cached `semanticdb-kotlinc:0.5.0` match the original
hashes above: neither file was written during this verification.

Focused checks: 29 SCIP-IO CLI tests pass; the Kotlin plugin suite has 58 passing
and 23 existing skipped tests, with zero failures. Formatter checks and the
source-built scip-java `cli/pack` pass. GitHub-hosted CI has not been run.
Evidence, the harness, validator and fresh outputs are retained under
`target/moshi-kotlin-compat/versioned/`. This is a local Kotlin 2.3.21
compatibility build, not a release for every Kotlin version. Existing released
artifacts remain available for Kotlin 2.1.20.

## Managed compatibility distribution follow-up

The versioned local verification above uses a disposable Maven repository to
let the rebuilt scip-java pack resolve the unpublished `scripts4` plugin. That
is a build-time dependency for the local reproduction, not a requirement for
the current checkout's managed distribution.

`scip-java-v0.12.3-scip-io.2` packages the rebuilt v0.12.3 pack with the exact
tested `semanticdb-kotlinc:0.5.1-kotlin-2.3.21-scripts4-SNAPSHOT` JAR. Its
SHA-256 is `79e73306593b97ac87ab656ed072e3c3548514d32bfa1b7d8da35c056238c751`.
The installed payload resolves that embedded JAR directly, so users do not need
an unpublished Maven repository or `JAVA_TOOL_OPTIONS` for the plugin at index
time. The indexed project can still resolve its own Gradle dependencies.

The `.2` package retains the original `scip-java` and `scip-java.bat` installer
names, but it has a new versioned cache directory and release tag. It is staged
by `scripts/stage-scip-java-compat.py`; its payload SHA-256 is
`6a348ada3570002344305e3cf20bc61c94e88a2f6220c298ef81b3980ab1f662`, while the
batch-launcher SHA-256 stays
`843319e0a3c57e588a0dd25edd2fee4621ec9cd152741d3128a5f5366264a593`. The
released `.1` distribution remains immutable and path-only.
