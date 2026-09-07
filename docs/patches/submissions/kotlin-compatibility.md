# Backport: support Kotlin 2.3.21 in the SemanticDB compiler plugin

The v0.5.0 SemanticDB plugin targets Kotlin 2.1.20 compiler internals. Loading
it with Kotlin 2.3.21 fails on the registrar contract and changed FIR APIs.
This backport adds the required plugin ID, adapts the checker and symbol APIs,
and preserves the existing symbol assertions while producing valid multiline
source ranges on Windows.

Base: `408df420cbf877c3babc3d7c73d54299f02c038d` (scip-kotlin v0.5.0).
Attachment: `semanticdb-kotlinc-2.3.21.patch`.
Routing: historical backport for the active scip-java maintainers; the original
scip-kotlin repository is archived. A maintenance destination must be selected.

## Changes

- Update compiler and test dependencies, registrar identity, FIR checker context
  parameters, containing-symbol lookup, and override lookup for Kotlin 2.3.21.
- Preserve the previous local/global symbol categories, close source streams,
  map end positions independently for multiline ranges, and normalize rendered
  documentation to LF on Windows.
- Add a real compiler regression for symbols, references, and multiline ranges.
- Update Spotless/ktfmt and use the supported Kotlin style. Additional source
  formatting is mechanical. CI installs JDK 8 and 21 and checks formatting and
  tests while retaining the existing Java 8 output target.

## Test plan

Run Gradle 8.10.2 on JDK 21 with a JDK 8 toolchain available:

```powershell
$env:OS = 'Windows_NT'
./gradlew.bat --no-daemon '-Porg.gradle.java.installations.paths=C:/path/to/jdk8' spotlessCheck test :semanticdb-kotlinc:shadowJar
```

Recorded results: formatting and the build pass; 81 tests comprise 58 passes,
23 existing skips, zero failures, and zero errors. Existing behavioral
assertions were retained. The formatter regression originally failed on the
new context-parameter declarations and now passes with the updated toolchain.

Integration used a distinct snapshot staged in a disposable Maven repository,
resolved through scip-java's existing `mavenLocal()` support. A source-built
scip-java launcher was selected explicitly through the corrected SCIP-IO CLI
binary option. Node invoked both Java and Kotlin checks with `shell:false`,
`OS=Windows_NT`, and an explicit local Maven repository property. Installed
launchers and cached release jars were not replaced; before/after hashes match.

Against unchanged Moshi commit `889013ec2edb8d8034902662a1dc8c4f3b3f8111`,
Kotlin 2.3.21 and Gradle 9.5.1, both generation checks reported one successful
output and zero failures. Each mixed-language SCIP output contained 57 Java
documents, 161 Kotlin documents, and 79,704 occurrences. Protobuf validation
checked source existence, unique paths, range bounds, definitions, references,
and 14,590 reference occurrences resolved across files.

## Compatibility and release boundary

This is a Kotlin 2.3.21 compatibility build, not a plugin verified across all
Kotlin versions. Keep the published 0.5.0 artifact available for Kotlin 2.1.20.
No scip-java snapshot dependency pin is attached. A consumer update must follow
an available published plugin coordinate and update its bundled compiler too.
The release version and maintenance destination are intentionally not invented
by this backport. Hosted CI and current-main integration remain unverified.
