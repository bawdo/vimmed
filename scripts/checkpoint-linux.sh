#!/usr/bin/env bash
# Linux measurement kit for a checkpoint. Run from the repository root on the
# reference laptop (Wayland session, for wtype):
#
#   scripts/checkpoint-linux.sh <checkpoint-number>
#
# Results land in target/checkpoint-<N>/ (one directory per measurement, each
# with the bench's own env.txt). The script prints the directories to paste back.
set -euo pipefail

if [ "$#" -ne 1 ] || ! [[ "$1" =~ ^[0-9]+$ ]]; then
    echo "usage: $0 <checkpoint-number>" >&2
    exit 2
fi
n="$1"

cd "$(dirname "$0")/.."

# The environment variable prefix is derived from the workspace metadata name
# (upper case, trailing underscore), so no name is written in this file.
slug="$(sed -n '/^\[workspace\.metadata\.app\]/,/^\[/{s/^name *= *"\(.*\)"/\1/p;}' Cargo.toml | head -n 1)"
if [ -z "$slug" ]; then
    echo "could not read [workspace.metadata.app] name from Cargo.toml" >&2
    exit 2
fi
prefix="$(printf '%s' "$slug" | tr '[:lower:]' '[:upper:]')_"
strip_var="${prefix}STRIP_PX"

missing=0
for tool in wtype rustup cargo; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "missing required tool: $tool" >&2
        missing=1
    fi
done
if [ "$missing" -ne 0 ]; then
    echo "install the missing tools and rerun" >&2
    exit 3
fi

out="target/checkpoint-$n"
mkdir -p "$out"
dirs=()

bench() {
    # bench <label> <args...>: runs a bench into $out/<label> and records the dir.
    local label="$1"
    shift
    local dir="$out/$label"
    echo "== $label: cargo xtask bench $* --out $dir"
    cargo xtask bench "$@" --out "$dir"
    dirs+=("$dir")
}

echo "== 1. release build and fixture"
cargo build --release
cargo xtask fixture

echo "== 2. cold start, 20 runs per renderer"
bench start-default start --runs 20 --renderer default
bench start-cairo start --runs 20 --renderer cairo

echo "== 3. first run (empty font cache)"
bench start-first-run start --first-run --runs 1

echo "== 4. manual typing, 500 keys per renderer (type into the window; it closes itself)"
bench type-default type --manual --keys 500 --renderer default
bench type-cairo type --manual --keys 500 --renderer cairo

echo "== 5. strip height, wtype, 300 keys at 64, 128 and 256 ($strip_var)"
for px in 64 128 256; do
    echo "-- $strip_var=$px"
    env "$strip_var=$px" cargo xtask bench type --wtype --keys 300 --out "$out/type-strip-$px"
    dirs+=("$out/type-strip-$px")
done

echo "== 6. optional Sysprof capture of start-up"
if command -v sysprof-cli >/dev/null 2>&1; then
    echo "run this and attach the .syscap file:"
else
    echo "sysprof-cli not found; where it is installed, run this and attach the .syscap file:"
fi
echo "(the sysprof-cli flags below are unverified: check them against sysprof-cli --help)"
printf '  sysprof-cli --force "%s/startup.syscap" -- target/release/window target/fixture/test.md\n' "$out"

echo
echo "Done. Paste back the contents of these directories (each has an env.txt):"
for d in "${dirs[@]}"; do
    echo "  $d"
done
