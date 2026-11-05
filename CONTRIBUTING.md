# Contributing

Thanks for taking an interest. vimmed is small and maintained by one person,
so the process is light.

## Before you start

Everyone taking part is expected to follow the
[Code of Conduct](CODE_OF_CONDUCT.md).

Open an issue for anything bigger than a typo or a one-line fix, so the
approach can be agreed before you spend time on it. Security problems go
through [SECURITY.md](SECURITY.md), not the issue tracker.

## Making a change

1. Fork and branch from `main`.
2. Run `cargo xtask ci` before pushing. It mirrors the GitHub Actions checks.
   If your change could behave differently on Linux, also run
   `cargo xtask linux-check`, which needs Docker.
3. Add a line under `[Unreleased]` in `CHANGELOG.md` for anything a user
   would notice. Follow the existing Keep a Changelog headings.
4. Open a pull request. The `CI result` check must pass before it can be
   merged, and it lands as a merge commit so signed commits keep their
   signatures. There is no reviewer requirement, but expect questions.

Commits are signed and written in the past tense with a subject of about
50 characters. Australian English in prose.

## How releases happen

Releases are cut by `.github/workflows/release.yml` when a merge to `main`
moves the newest `## [X.Y.Z] - YYYY-MM-DD` heading in `CHANGELOG.md` to a
version with no published release. While the changelog has only
`[Unreleased]`, a merge to `main` runs CI and stops. To release:

1. Run the hand checks in [docs/manual-checks.md](docs/manual-checks.md)
   on both platforms.
2. Rename `[Unreleased]` to the new version and date, and add the
   `[X.Y.Z]: compare` link at the bottom
   (`https://github.com/bawdo/vimmed/compare/vA.B.C...vX.Y.Z`, from the
   previous tag). For the first release there is no earlier tag, so link
   `https://github.com/bawdo/vimmed/releases/tag/vX.Y.Z` instead. Bump
   `version` under `[workspace.package]` in `Cargo.toml` to the same
   X.Y.Z and commit the refreshed `Cargo.lock`. The `Changelog` check
   fails a pull request whose changelog and `Cargo.toml` versions differ.
3. Start a fresh `[Unreleased]` section above it.
4. Merge. The workflow runs CI, tags the merge commit (or, if you tagged
   `vX.Y.Z` by hand, uses that commit), builds the macOS app (arm64) and
   the Linux `.deb`, `.rpm` and PKGBUILD, signs and notarises the app,
   attests the checksums and publishes the release with that changelog
   section as the notes. A draft release of the same name is replaced.

Signing and notarisation use the `release` environment's secrets
(`MACOS_CERTIFICATE_P12`, `MACOS_CERTIFICATE_PASSWORD`, `NOTARY_API_KEY_P8`,
`NOTARY_API_KEY_ID` and `NOTARY_API_ISSUER_ID`) and its `APPLE_TEAM_ID`
variable. Only the signing job can read them, and it runs none of the
project's code.

A pull request whose changelog version is behind the latest tag, or has a
malformed date or no link reference, fails the `Changelog` check.

## Tool versions

The checks use these pinned tools. Install the same versions to get the same
results as CI:

- cargo-deny: `cargo install --locked cargo-deny --version 0.20.2`
- zizmor: `cargo install --locked zizmor --version 1.30.1`

## Licence

By contributing, you agree your work is released under the [MIT licence](LICENSE).
