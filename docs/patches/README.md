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
