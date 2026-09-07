# Upstream backport submission drafts

Prepared on 2026-09-07. The routing issue is now open:
[scip-code/scip-java#1012](https://github.com/scip-code/scip-java/issues/1012).
It includes the verification results and both patches in linked comments.
No PR has been opened.

| Draft | Patch | Exact base |
|---|---|---|
| [Windows Gradle paths](gradle-paths.md) | `../scip-java-gradle-paths.patch` | scip-java v0.12.3, `4486a05471000ba4e784b72395ebf84207f24af8` |
| [Kotlin 2.3.21 compatibility](kotlin-compatibility.md) | `../semanticdb-kotlinc-2.3.21.patch` | scip-kotlin v0.5.0, `408df420cbf877c3babc3d7c73d54299f02c038d` |

## Routing

GitHub API checks confirmed that [scip-code/scip-java](https://github.com/scip-code/scip-java)
is active and [sourcegraph/scip-kotlin](https://github.com/sourcegraph/scip-kotlin)
is archived. The latter's README directs contributions to scip-java. The current
scip-java main commit was `630044d4d91b7f124fdf5889ef755be5dd669ac4` at preparation.
These patches target the historical tags above, not that current main commit.
No maintenance PR base has been selected or created.

Send the backport request to the active scip-java maintainers first, using the
note below. They must identify a maintenance branch for the Gradle fix and the
destination for the historical SemanticDB plugin backport. Do not open a PR
against the archived repository or label either patch as tested on current main.

> We have two tested Windows/Kotlin backports for scip-java v0.12.3 and the
> semanticdb-kotlinc v0.5.0 codebase: safe Gradle init-script path serialization
> and Kotlin 2.3.21 compiler compatibility. Both patches and separate test plans
> are attached. Since scip-kotlin is archived, which maintenance branch and
> repository should receive these backports? The consumer snapshot pin is
> deliberately excluded; a consumer upgrade would follow a published plugin.

## Package and checks

`target/upstream-backport-submissions.zip` contains the two unchanged patches,
these drafts, and a SHA-256 manifest. Both patches passed fresh
`git apply --check --whitespace=error-all` checks against their exact clean bases.
The test results in the drafts are the recorded verification from this work;
preparing the drafts did not rerun full generation or hosted CI.

The archive excludes `patches/local/`, binaries, machine-specific logs, and the
SCIP-IO CLI changes. The Kotlin patch documents the local snapshot verification
recipe but does not change scip-java's default dependency pin. The separate
local pin must never be included in the upstream submission.

Issue #1012 was opened with explicit authorization. Both posted patches were
retrieved and their LF-normalized SHA-256 hashes verified. Opening the issue
did not create an upstream commit, branch, PR, release, or artifact publication.
