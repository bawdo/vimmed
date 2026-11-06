#!/bin/bash -eu
# Builds the cargo-fuzz targets for ClusterFuzzLite. Runs inside the OSS-Fuzz
# builder image, which provides $SRC and $OUT.
set -euo pipefail

cd "$SRC/vimmed/fuzz"
cargo fuzz build -O

# Target names are the same as in fuzz/Cargo.toml. libFuzzer's max_len is
# 4096 for editor_keys (a key script) and 65536 for the rest.
TARGETS="markdown_incremental ex_command editor_keys config_parse state_files"

for target in $TARGETS; do
  cp "target/x86_64-unknown-linux-gnu/release/$target" "$OUT/"
  zip -j -q "$OUT/${target}_seed_corpus.zip" corpus/"$target"/*

  max_len=65536
  if [ "$target" = "editor_keys" ]; then
    max_len=4096
  fi
  printf '[libfuzzer]\nmax_len = %s\ntimeout = 10\n' "$max_len" > "$OUT/$target.options"
done
