"""Check the shipped payload, including nested compiler resources, without Maven."""
import hashlib
import io
from pathlib import Path
import sys
import zipfile

PLUGIN_SHA256 = "79e73306593b97ac87ab656ed072e3c3548514d32bfa1b7d8da35c056238c751"


def check(payload):
    with zipfile.ZipFile(payload) as archive:
        prefix = "coursier/bootstrap/launcher/"
        assert not archive.read(prefix + "bootstrap-jar-urls").strip(), "Remote bootstrap dependencies"
        names = archive.read(prefix + "bootstrap-jar-resources").decode().splitlines()
        assert "kotlin-compiler-embeddable-2.3.21.jar" in names, "Compiler must match Kotlin 2.3.21"
        cli_name = next(n for n in names if n.startswith("scip-java_2.13-"))
        with zipfile.ZipFile(io.BytesIO(archive.read(prefix + "jars/" + cli_name))) as cli:
            plugin = cli.read("semanticdb-kotlinc.jar")
            assert hashlib.sha256(plugin).hexdigest() == PLUGIN_SHA256, "Untested Kotlin plugin"
            with zipfile.ZipFile(io.BytesIO(cli.read("gradle-plugin.jar"))) as gradle:
                assert gradle.read("semanticdb-kotlinc.jar") == plugin, "Gradle and CLI plugins differ"
                assert "com/sourcegraph/gradle/semanticdb/EmbeddedKotlinPlugin$.class" in gradle.namelist()
                assert "ScipGradleScript.kt" in gradle.namelist(), "Missing script compiler helper"
        for name in names:
            assert prefix + "jars/" + name in archive.namelist(), f"Missing embedded dependency: {name}"
    print("PASS: embedded Kotlin 2.3.21 compiler, exact tested plugin, CLI/Gradle parity, no remote bootstrap jars")


if __name__ == "__main__":
    check(Path(sys.argv[1]))
