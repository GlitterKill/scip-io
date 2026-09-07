# Backport: escape paths in generated Gradle initialization scripts

On Windows, scip-java v0.12.3 writes filesystem paths directly into quoted
Groovy strings. A generated line such as `classpath(files("C:\Users\test\gradle-plugin.jar"))`
fails during initialization-script compilation, before project compilation.
The attached backport serializes all five paths with the existing JSON encoder
and escapes dollar signs for Groovy, preserving their literal values.

Base: `4486a05471000ba4e784b72395ebf84207f24af8` (v0.12.3).
Attachment: `scip-java-gradle-paths.patch`.

## Changes

- Apply one quoting helper to the embedded Gradle plugin, SemanticDB plugin,
  javac agent, target root, and dependency output paths.
- Add a standalone regression that parses the complete generated script with
  Gradle's Groovy compiler and checks all five values, plus six escaping cases.
- Document the regression command and its scope.

The failure was reproduced by inspecting the actual generated file, including
its literal backslashes. `OS=Windows_NT` remained present during testing, so
the separate legacy Windows launcher SHIFT-loop defect was not involved.

## Test plan

Run on Windows with Java, Python, the v0.12.3 companion launcher, and Gradle 9.5.1:

```powershell
$env:OS = 'Windows_NT'
./tests/gradle-init-script/run.ps1 -Launcher <release-companion-launcher> -GradleHome <gradle-home> -Original
./tests/gradle-init-script/run.ps1 -Launcher <release-companion-launcher> -GradleHome <gradle-home>
```

Recorded results: the original fails Groovy script compilation; the patched
version passes all five path checks and six escaping cases. Cases include
backslashes, spaces, apostrophes, double quotes, dollar interpolation, and
control characters. Double quotes are tested as strings because Windows
filenames cannot contain them. The source-built `cli/pack` also passed.

With the separate Kotlin compatibility patch and a locally resolved versioned
plugin, Java and Kotlin generation each succeeded on unchanged Moshi commit
`889013ec2edb8d8034902662a1dc8c4f3b3f8111`. Each output contained 218 documents
and 79,704 occurrences with valid source ranges, including 14,590 references
resolved to definitions in other files. No generator failures occurred.

## Scope

The parser regression is standalone, not part of sbt's default test task.
The full Moshi result requires the separate Kotlin compatibility work; this
path fix alone does not make the old Kotlin plugin compatible with Kotlin 2.3.21.
No dependency pin, environment workaround, fallback behavior, or SCIP-IO change
is included. Hosted CI and application to current main have not been verified.
