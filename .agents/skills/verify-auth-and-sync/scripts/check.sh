#!/usr/bin/env bash
set -euo pipefail

skill_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cache_dir="${TLC_CACHE_DIR:-$HOME/.cache/slippy-tlc}"
jar="${TLA2TOOLS_JAR:-$cache_dir/tla2tools-1.7.4.jar}"
java_bin="${JAVA:-$cache_dir/temurin-21-jre/Contents/Home/bin/java}"

if [[ ! -r "$jar" || ! -x "$java_bin" ]]; then
    printf 'Set TLA2TOOLS_JAR and JAVA to TLC 1.7.4 and a Java executable.\n' >&2
    exit 2
fi

scratch="$(mktemp -d "${TMPDIR:-/tmp}/hex-auth-and-sync.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT

for module in TeamAuthRefresh SlippySyncCursor; do
    printf '\n=== TLC: %s ===\n' "$module"
    workdir="$scratch/$module"
    mkdir -p "$workdir"
    cp "$skill_dir/spec/$module.tla" "$skill_dir/spec/$module.cfg" "$workdir"
    (
        cd "$workdir"
        "$java_bin" -XX:+UseParallelGC -cp "$jar" tlc2.TLC \
            -workers 1 -fp 0 -metadir "$workdir/meta" "$module"
    )
done
