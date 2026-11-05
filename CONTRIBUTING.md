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

## Tool versions

The checks use these pinned tools. Install the same versions to get the same
results as CI:

- cargo-deny: `cargo install --locked cargo-deny --version 0.20.2`

## Licence

By contributing, you agree your work is released under the [MIT licence](LICENSE).
