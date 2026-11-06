# Build image for ClusterFuzzLite (see .github/workflows/cflite_pr.yml and
# cflite_batch.yml). Pinned by digest; Dependabot raises bumps.
#
# Resolved on 2026-11-06 from gcr.io/oss-fuzz-base/base-builder-rust:latest.
#
# The base image sets RUSTUP_TOOLCHAIN to its own nightly, which rustup honours
# ahead of both rust-toolchain.toml files. That nightly (1.91) is older than the
# workspace rust-version (1.92), so `cargo fuzz build` would refuse to build.
# Install our dated nightly and make it win instead. The date below must match
# fuzz/rust-toolchain.toml and fuzz/README.md. It is bumped by hand; Dependabot
# does not see it.
FROM gcr.io/oss-fuzz-base/base-builder-rust:latest@sha256:3428d0a90b1e7efbe2ba947bd1d0c9a70594986b4b6553804547f1fb24cfc92f

# Same profile and component the base image installs for its own nightly
# (install_rust.sh). cargo-fuzz in the image is a plain cargo binary with no
# toolchain of its own, so it runs under RUSTUP_TOOLCHAIN.
RUN rustup toolchain install nightly-2026-10-23 --profile minimal \
    && rustup component add rust-src --toolchain nightly-2026-10-23
ENV RUSTUP_TOOLCHAIN=nightly-2026-10-23

COPY . $SRC/vimmed
WORKDIR $SRC/vimmed
COPY .clusterfuzzlite/build.sh $SRC/
