# Submission patches and local verification

The upstream backports are:

- `scip-java-gradle-paths.patch`: Windows Gradle path serialization, based on scip-java v0.12.3.
- `semanticdb-kotlinc-2.3.21.patch`: compiler compatibility and formatter/CI updates, based on scip-kotlin v0.5.0.

`local/scip-java-kotlin-2.3.21.patch` is **verification only; exclude it from
upstream submissions**. It pins an unpublished plugin snapshot and the matching
Kotlin compiler together. Apply it only in the disposable scip-java checkout
after staging the snapshot as described in `../windows-scip-java-gradle.md`.

Do not submit a compiler-only bump while retaining the old plugin: their
compiler APIs must match. A consumer dependency update can be prepared when
an upstream artifact is available; it must use that published coordinate.
The local verification patch is retained to reproduce the successful SCIP runs.

Prepared submission bodies, exact bases, validation scope, and the archived
Kotlin repository's routing constraint are in [submission drafts](submissions/README.md).
The shareable archive is `target/upstream-backport-submissions.zip`.

## Follow-up source-range repair

Apply `semanticdb-kotlinc-source-ranges.patch` after the compatibility patch.
It selects the identifier inside generic or qualified type references and omits
synthetic delegated-property accessor references whose source belongs to the
delegate expression. Its compiler regression preserves real `lazy()` and property
references. The patch was checked against the compatibility-patched v0.5.0 base.

This follow-up is not in the previously submitted archive. It does not add
Gradle Kotlin DSL script indexing. SDLBench's strict coverage gate must continue
to reject missing `.gradle.kts` documents.

The new disposable plugin coordinate is
`com.sourcegraph:semanticdb-kotlinc:0.5.1-kotlin-2.3.21-ranges2-SNAPSHOT`.
Its JAR SHA-256 is
`6b60a046592de6676e75c4542d9bb78a9efe680674ae6923286b15c0c5cdc80c`.
Pin that version in the local scip-java build after applying the existing local
integration patch; keep Kotlin at 2.3.21. This snapshot pin remains verification
only and must not enter an upstream submission.

The follow-up regression also asserts declaration-name ranges for expression-bodied
functions. This prevents the type-name helper from selecting an identifier in a
function body. Earlier `ranges` artifacts are superseded by `ranges2`.

## Gradle Kotlin DSL follow-up

`scip-java-gradle-kotlin-dsl.patch` adds `scipCompileKotlinDsl` to the root
`scipCompileAll` task. It obtains Gradle's script models, then compiles each script
with the embedded compiler, standalone script template, resolved classpath,
implicit imports, assignment plugin, SAM-with-receiver plugin, and SemanticDB.
The pass compiles scripts without evaluating them again. It removes each prior
SemanticDB file before compilation and fails on compiler errors or missing output,
including when Gradle already has cached compiled scripts.

`semanticdb-kotlinc-scripts.patch` applies after the compatibility and source-range
patches. It gives script members an owner derived from the relative script path
and filters compiler-generated script parameters/results at the shared emitter,
including their accessors. It emits compiler kinds for local script variables and functions, whose identifiers lack descriptor suffixes. It emits no artificial script-root definitions.

Both patches are follow-ups, not part of the previously uploaded submission
archive. The scip-java patch applies to v0.12.3 alongside the Windows-path patch;
the Kotlin patch applies to compatibility- and range-patched v0.5.0. Both additive
patches pass `git apply --check` against those bases.

For local verification, use `local/scip-java-kotlin-2.3.21-scripts.patch` **instead
of** the earlier local version patch. It selects
`com.sourcegraph:semanticdb-kotlinc:0.5.1-kotlin-2.3.21-scripts4-SNAPSHOT` with
Kotlin 2.3.21. The JAR SHA-256 is
`79e73306593b97ac87ab656ed072e3c3548514d32bfa1b7d8da35c056238c751`.
This unpublished coordinate must remain outside upstream submission patches.

The integration regression is included in the scip-java patch:

```powershell
./tests/gradle-kotlin-dsl/run.ps1 -Pack ./scip-java/target/pack -GradleHome <Gradle-9.5.1> -MavenRepository <maven-scripts4>
```

It checks settings/root/child script documents, resolved Gradle API calls,
source definition/reference pairs, distinct identities for same-named script
members, and the absence of synthetic script fields. It repeats with a warm
Gradle script cache, then verifies invalid script compilation fails without SCIP.
The fixture path includes Windows backslashes, spaces, and an apostrophe.
The existing Kotlin suite passes 59 tests with 23 skipped.

The tested integration is Windows, Gradle 9.5.1 (embedded Kotlin 2.3.20), and
project Kotlin 2.3.21. Other Gradle/compiler versions and precompiled or included-build
script variants are not yet verified. Unsupported script compilation fails;
it does not fall back to fabricated facts or relaxed readiness checks.

The consumer integration requires the compatible script-capable Kotlin plugin.
Before upstream merge, replace the local verification pin with a published
coordinate containing these fixes; the integration patch alone does not upgrade
the old embedded plugin. The standalone Scala helper also passes formatting
and compilation checks with untracked sources included (`project.git = false`
in a temporary formatter config).

## Managed distribution assembly

The `.2` managed scip-java distribution consumes the exact tested `scripts4`
plugin JAR rather than resolving its unpublished Maven coordinate at index time.
`scripts/stage-scip-java-compat.py` embeds the JAR with SHA-256
`79e73306593b97ac87ab656ed072e3c3548514d32bfa1b7d8da35c056238c751` while it
stages the rebuilt scip-java v0.12.3 pack. Its payload SHA-256 is
`6a348ada3570002344305e3cf20bc61c94e88a2f6220c298ef81b3980ab1f662`; the batch
launcher retains SHA-256
`843319e0a3c57e588a0dd25edd2fee4621ec9cd152741d3128a5f5366264a593`.

`scip-java-bundled-kotlin.patch` makes the two bundled plugin resources
authoritative for Gradle source compilation, Gradle script compilation, and CLI
dependency compilation. It removes unpublished plugin resolution only. Projects
can still resolve their ordinary Gradle dependencies from their configured
repositories. The staging directory includes the patches, Apache-2.0 license,
`PACK-SHA256SUMS.txt`, and `SHA256SUMS.txt`. This packaging step does not change
the upstream patch scope or publish the local coordinate.

Final SDLBench verification (`moshi-jvm-2026-09-08T12-23-18-911Z`) freshly generated Java, Kotlin, and merged SCIP in both disposable Moshi tasks with generated-index caching disabled. All six artifacts validate: 231 documents, including all 13 Gradle scripts, 80,852 occurrences, and 14,596 cross-file resolved references each. The first task reports 156/156 provider-primary files and zero uncovered, fallback, incomplete-call-proof, or generator failures, and passes its task verifier. The second passes indexing preflight but later times out in agent verification with an `EBUSY` read error; the overall benchmark is therefore failed. This does not establish a passing full benchmark. Detailed evidence and commands are in `F:/Claude/projects/sdl-mcp/sdl-mcp/sdlbench/docs/index-preflight.md`, under the Gradle Kotlin DSL follow-up.

The follow-up rerun `moshi-jvm-2026-09-09T12-45-46-953Z` passes both Moshi tasks and both verifiers. The remaining defects were in SDL runtime output-drain timeout handling and SDLBench source snapshots reading ignored, locked build caches. Both are repaired with regressions. Each task now reports 156/156 provider-primary files, zero uncovered/fallback/incomplete-call-proof files, zero generator failures, and `semanticDeferred:false`. All six fresh SCIP artifacts validate with the counts above; generated-index caching remains disabled. The final evidence and hashes are in the SDLBench report's 2026-09-09 section. This verification does not publish or change the upstream patch scope.
