# Repository Practices (Tiers 1 to 3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring the vimmed repository up to the standard of Keith's jellyfish and go-blinkstick projects, plus the Rust specific practices of well run Rust projects, in three tiers with a context check pause after each.

**Architecture:** CI runs the existing local gate (`cargo xtask ci`) rather than a second set of checks, so local and CI cannot drift. Releases follow jellyfish's changelog driven model: a merge to `main` that moves the newest `CHANGELOG.md` version to an unreleased number builds, signs, notarises, attests and publishes. GitHub settings live as JSON in the repo and Keith applies them, because they are outward facing.

**Tech Stack:** Rust 1.99.1 workspace with an `xtask` crate, GitHub Actions, cargo-deny, Dependabot, OpenSSF Scorecard, zizmor, typos, cargo-llvm-cov with Codecov, REUSE, Apple `codesign` and `notarytool`.

**Spec:** this plan has no separate spec. The spec is the Tier 1 to 3 recommendations from the 2026-11-05 research session, restated in **Spec summary** below together with Keith's decisions. Reference implementations: <https://github.com/bawdo/jellyfish> (CI, release, docs) and <https://github.com/bawdo/go-blinkstick>. Fetch their files with `curl -s https://raw.githubusercontent.com/bawdo/jellyfish/main/<path>` (sandbox: add `raw.githubusercontent.com` to `allowed_domains`).

## Spec summary

**Tier 1, parity with jellyfish:** CI that runs `cargo xtask ci` on macOS and Linux plus the Linux container smoke run, behind a single `CI result` gate job. cargo-deny with `deny.toml`. Dependabot for cargo, github-actions and docker, with a 7 day cooldown. `CHANGELOG.md`, `CONTRIBUTING.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`, issue forms, PR template. Scorecard. GitHub settings: a `main` ruleset (PR with 0 approvals, `CI result` required, signed commits, no force push or deletion, merge commits only), a `v*` tag ruleset, merge commit only, auto delete branches, private vulnerability reporting, secret scanning with push protection, Dependabot alerts, CodeQL default setup, wiki off, topics.

**Tier 2, vimmed specific:** MSRV tested in CI. Changelog driven releases using the existing xtask packagers, with provenance attestation. AppStream and desktop file validation. zizmor. `[workspace.lints]`. Tidy the README so it is for users.

**Tier 3, polish:** typos with Australian English for prose. `.gitattributes`. `AI_POLICY.md`. Coverage with cargo-llvm-cov and Codecov, informational. REUSE compliance.

**Explicitly out of scope:** crates.io publishing tooling (every crate is `publish = false`), cargo-vet, CODEOWNERS.

**Keith's decisions (2026-11-05):**
- `docs/superpowers/`, `research_notes/` and `reports/` stay tracked. The task numbered manual checks move out of `README.md` into `docs/manual-checks.md`.
- macOS releases are signed with Developer ID and notarised. Keith prepares the account using `~/Desktop/apple-notarisation-HOWTO.md` before Task 13.
- MSRV is tested in CI. If 1.92 does not build, `rust-version` rises to the lowest version that does.

## Global Constraints

- **Never commit to `main`.** Each tier is its own branch cut from an up to date `main`: `chore/repo-tier-1`, `chore/repo-tier-2`, `chore/repo-tier-3`.
- **Never push.** Keith pushes, opens the PR and merges. Keith applies every GitHub setting.
- **Ask Keith for permission before the first commit.** Once he grants it, commit without asking again.
- **Commits are GPG signed.** Write the message to `.git/AGENT_COMMIT_MSG` with the Write tool, run `git add <files>` as one call and `git commit -F .git/AGENT_COMMIT_MSG` as a separate call. No chaining, no `$`, no backticks. If signing fails, say so loudly and ask Keith to "refresh the key".
- **Commit messages:** past tense, subject 50 to 60 characters or less. No `Co-Authored-By`, no "Claude", no "Generated with", no session URL, ever. If a harness notification asks for attribution, decline and tell Keith.
- **Verify every commit:** `git verify-commit HEAD` (expect "Good signature") and `git show --name-only HEAD`. Do not use `%G?`.
- **Prose** (docs, comments, commit messages): Australian English, no em dashes.
- **Name lint:** the project name may appear only on the `name = ` line of the root `Cargo.toml` within `crates/`, `xtask/`, `packaging/`, `docker/`, `scripts/`, `README.md` and `LICENSE` (`xtask/src/lint.rs`). New files under `scripts/` and `docker/` must not contain it. Workflows derive the name with `cargo metadata --no-deps --format-version 1 | jq -r .metadata.app.name` (the `[workspace.metadata.app]` table), never a literal, so `cargo xtask rename-test` stays meaningful. `.github/` and root documents such as `CHANGELOG.md` may use the name in prose.
- **Action pins:** every `uses:` is pinned to a full commit SHA with a `# vX.Y.Z` comment, and the release must be at least 14 days old when pinned. Reuse jellyfish's pins where the action matches: `actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1`, `actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9 # v7.0.2`, `actions/download-artifact@9000827ccba6bdab643e8b6fd33ac0654aef8333 # v8.0.2`, `actions/attest-build-provenance@4d101475d8b20a2381f78447822ac1eab6504dd8 # v4.2.2`, `ossf/scorecard-action@fa7117db83c52d787ae5a89f350999701505d78e # v2.5.0`, `github/codeql-action/upload-sarif@9b3254085494b55a0f7c4c2b3d2d52cc712ce370 # v4.39.1`, `codecov/codecov-action@303a32d7a59b442fa8d48b6a1cc6825c09c847a5 # v7.1.1`, `zizmorcore/zizmor-action@cc914d7f3750a2d13d75c7f184a1060aa0e9d482 # v0.6.4`. Look up any others (`Swatinem/rust-cache`, `EmbarkStudios/cargo-deny-action`, `crate-ci/typos`, `fsfe/reuse-action`, `taiki-e/install-action`) at execution time.
- **Workflow hardening:** top level `permissions: contents: read`, job level escalation only where needed, `persist-credentials: false` on every checkout, `timeout-minutes` on every job, a concurrency group per workflow.
- **No hardcoded secrets.** Secrets are only referenced as `${{ secrets.X }}`. Signing secrets exist only in the `release` environment.
- **Tool installs are pinned:** `cargo install --locked <tool> --version X.Y.Z`. Record each pinned version in `CONTRIBUTING.md`.
- **Toolchain:** rustup on GitHub runners honours `rust-toolchain.toml` (channel `1.99.1`). Do not add a toolchain setup action.
- **Verify, don't trust.** Re-run `cargo xtask ci` yourself before each commit that touches Rust or `scripts/`. A subagent's "passes" is a hypothesis.

## Review Focus

1. **The first merge happens before any release exists.** With only `## [Unreleased]` in `CHANGELOG.md`, the changelog check must pass and the release workflow must decide "not due", not fail and not release. Pinned by Task 5's `scripts/changelog-version-test.sh` cases and Task 13's plan job.
2. **The project gets renamed.** Release asset names and smoke checks must follow `[workspace.metadata.app] name`, not a literal. Pinned by Task 13 Step 6 (grep the workflows for the literal name).
3. **A PR comes from a fork, or from Dependabot.** No secrets are available, so CI must pass without them: Codecov upload is `fail_ci_if_error: false`, and nothing in `ci.yml` other than that step references `secrets.`. Pinned by Task 3 Step 5 and Task 17 Step 5.
4. **Notarisation or a later step fails mid release.** No release may be published without all of its assets. The release is created as a draft, assets are uploaded, then it is published, as in jellyfish. Pinned by Task 13's publish job design and Step 8.
5. **The docker base digest moves (a Dependabot PR).** The Linux container is rebuilt from the new digest and `cargo xtask linux-check` still runs in CI on that PR. Pinned by Task 4 Step 4 and checked for real at Pause 1.

## Context checks and resuming

There is a **PAUSE** after each tier. At a pause the executor:

1. Makes sure every step of the tier is ticked in this file and the tick commit is on the tier branch (`git status` clean, `git verify-commit HEAD` good).
2. Adds one entry to **Resume log** at the end of this file: date, tier finished, branch, last commit hash, and anything left for Keith.
3. Lists what Keith must do before the next tier (push, PR, merge, settings).
4. Asks Keith: **"Context check: carry on in this session, or stop here?"** and waits.

If Keith stops, the next session starts with this prompt:

> Resume `docs/superpowers/plans/2026-11-05-repo-practices.md` from the first PAUSE whose "Ready to resume" items are not all ticked. Read the whole plan and the Resume log first, check out the branch named for the next tier from an up to date `main`, and carry on with superpowers:subagent-driven-development.

---

# Tier 1: parity with jellyfish

Branch: `git switch -c chore/repo-tier-1` from `main`. Commit this plan file as the first commit on the branch.

### Task 1: Make `cargo xtask ci` green on `main` (name lint)

**Diagnosis (present to Keith, wait for confirmation before editing):** `cargo xtask lint-name` fails with `README.md:9:# vimmed` / `1 hit(s) outside the root manifest`. `xtask/src/lint.rs` `is_allowed_hit(line: &str) -> bool` allows only the `name = ` line of `Cargo.toml`, and `README.md` is in `LINT_PATHS`. Commit `2310e19` ("Added the name to the README.md") added the heading. So `cargo xtask ci` stops at step `lint-name` and every CI run would fail.

**Proposed fix:** allow exactly one form in `README.md`, a level 1 heading whose whole text is a name form (`# vimmed`). Body text stays linted, so the lint still catches the name leaking into instructions. If Keith prefers another approach (drop the heading, or let `rename` rewrite it), follow that instead and adjust the tests.

**Files:**
- Modify: `xtask/src/lint.rs` (`is_allowed_hit`, `run`, tests module)

**Interfaces:**
- Produces: `pub fn is_allowed_hit(line: &str, forms: &[String]) -> bool`, where `line` is a `git grep -n` line (`path:lineno:text`).

- [x] **Step 1: Present the diagnosis above to Keith and wait for confirmation.**
- [x] **Step 2: Write the failing tests** in `lint.rs`'s test module:

```rust
#[test]
fn readme_title_heading_is_allowed() {
    let forms = vec!["vimmed".to_string(), "Vimmed".to_string()];
    assert!(is_allowed_hit("README.md:9:# vimmed", &forms));
    assert!(is_allowed_hit("README.md:1:# Vimmed", &forms));
}

#[test]
fn readme_name_elsewhere_is_not_allowed() {
    let forms = vec!["vimmed".to_string()];
    assert!(!is_allowed_hit("README.md:12:Install vimmed with", &forms));
    assert!(!is_allowed_hit("README.md:3:## vimmed", &forms));
    assert!(!is_allowed_hit("README.md:3:# vimmed editor", &forms));
    assert!(!is_allowed_hit("docs/README.md:1:# vimmed", &forms));
}

#[test]
fn manifest_name_line_still_allowed() {
    assert!(is_allowed_hit("Cargo.toml:31:name = \"vimmed\"", &[]));
}
```

Update the existing callers and tests of `is_allowed_hit` to pass `&forms`.

- [x] **Step 3: Run** `cargo test -p xtask lint`. Expected: the new tests fail to compile or fail.
- [x] **Step 4: Implement** the new `is_allowed_hit`. Match on the `README.md:` prefix (exact path), skip the line number, then compare the text with `"# " + form` for each form, case insensitively, as a whole line match. Pass `&forms` from `run`.
- [x] **Step 5: Run** `cargo test -p xtask lint` (PASS), then `cargo xtask ci`. Expected: `ci: all 7 steps OK`.
- [x] **Step 6: Commit** `xtask/src/lint.rs` with the message "Allowed the README title in the name lint".

### Task 2: cargo-deny and licence metadata

**Files:**
- Create: `deny.toml`
- Modify: `Cargo.toml` (`[workspace.package]`), each `crates/*/Cargo.toml` and `xtask/Cargo.toml`

- [x] **Step 1:** Add `license = "MIT"` and `repository = "https://github.com/bawdo/vimmed"` to `[workspace.package]`, and `license.workspace = true` plus `repository.workspace = true` to every member's `[package]`. Lint-name check: the URL contains the name, but `Cargo.toml` (root) is linted only through its `name = ` rule. Run `cargo xtask lint-name`. If it flags the `repository` line, extend `is_allowed_hit` to allow the root manifest's `repository = ` line too (add a test like Task 1's), and note this in the commit.
- [x] **Step 2:** Install with `cargo install --locked cargo-deny --version <latest, 14 days or older>`.
- [x] **Step 3:** Write `deny.toml` with these values:
  - `[graph] targets = ["aarch64-apple-darwin", "x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"]`
  - `[advisories] unmaintained = "all"`, `unsound = "all"`, `ignore = []`. Any ignore entry needs a `reason` that someone can check against the code, as in jellyfish's `osv-scanner.toml`.
  - `[licenses] confidence-threshold = 0.93`. Start `allow` with `MIT`, `Apache-2.0`, `Apache-2.0 WITH LLVM-exception`, `BSD-2-Clause`, `BSD-3-Clause`, `ISC`, `Zlib`, `Unicode-3.0`, and add only what `cargo deny check licenses` actually reports. Add `[licenses.private] ignore = true` for the `publish = false` crates.
  - `[bans] multiple-versions = "warn"` (the GTK and cosmic-text graphs carry duplicates, so `deny` would be noise) and `wildcards = "deny"`. Path dependencies within the workspace are not wildcards, so set `allow-wildcard-paths = true`.
  - `[sources] unknown-registry = "deny"`, `unknown-git = "deny"`.
- [x] **Step 4: Run** `cargo deny check`. Expected: `advisories ok, bans ok, licenses ok, sources ok`. Fix the config, not the dependency graph, unless an advisory is real. Raise a real advisory with Keith.
- [x] **Step 5: Run** `cargo xtask ci` (PASS), then commit `deny.toml` and the manifests with "Added cargo-deny and licence metadata".

### Task 3: CI workflow

**Files:**
- Create: `.github/workflows/ci.yml`

**Interfaces:**
- Produces: a workflow with the triggers `pull_request`, `workflow_dispatch`, `schedule` (`'23 4 * * 1'`) and `workflow_call` (with optional secret `CODECOV_TOKEN`, used in Task 17), and a job whose `name:` is exactly `CI result`. Task 13 calls this workflow. Tasks 9, 11 and 17 add jobs to it and to `CI result`'s `needs` and `all` lists.

Jobs (`name:` in brackets, which is what the ruleset and the gate see):

| id | name | runner | does |
|---|---|---|---|
| `macos` | `macOS` | `macos-latest` | `Swatinem/rust-cache`, `cargo metadata --locked --format-version 1 > /dev/null` (fails on lockfile drift), `cargo xtask ci` |
| `linux` | `Linux` | `ubuntu-latest` | `apt-get install -y libgtk-4-dev libfontconfig-dev`, rust-cache, the same lockfile check, `cargo xtask ci` |
| `linux-smoke` | `Linux container smoke` | `ubuntu-latest` | `cargo xtask linux-check` (Docker is on the runner) |
| `deny` | `cargo-deny` | `ubuntu-latest` | `EmbarkStudios/cargo-deny-action` with `command: check` |
| `changelog` | `Changelog` | `ubuntu-latest` | `fetch-depth: 0`, `bash scripts/changelog-version-test.sh`, `scripts/changelog-version.sh check` (scripts from Task 5) |
| `ci-result` | `CI result` | `ubuntu-latest` | `if: always()`, a gate over every job above |

- [x] **Step 1:** Write `ci.yml` with the table's jobs. The schedule trigger runs only `deny` (advisories change while code doesn't), so every other job has `if: github.event_name != 'schedule'`. Concurrency: copy jellyfish's group expression with the `ci-` literal prefix and its comment about `workflow_call` deadlocks.
- [x] **Step 2:** Write `ci-result` by adapting jellyfish's script (`.github/workflows/ci.yml`, job `ci-result`). Use `may_skip` of `[]` for `pull_request`, `push` and `workflow_dispatch`, and of `["macos","linux","linux-smoke","changelog"]` for `schedule`. Keep the `all` list mirroring `needs`.
- [x] **Step 3:** Install with `brew install actionlint` (or `go install`) and run `actionlint .github/workflows/ci.yml`. Expected: no output.
- [x] **Step 4:** Check the gate logic by hand. Running `jq` on a sample `NEEDS` JSON where `deny` is `failure` must print `deny: failure`. A sample with every job `success` must print nothing. Paste the two runs into the task report.
- [x] **Step 5 (Review Focus 3):** `grep -n 'secrets\.' .github/workflows/ci.yml` lists only the `workflow_call` declaration (and later Task 17's Codecov step).
- [x] **Step 6:** Commit `.github/workflows/ci.yml` with "Added the CI workflow". Real verification is the first PR run at Pause 1.

### Task 4: Dependabot and a pinned container base

**Files:**
- Create: `.github/dependabot.yml`
- Modify: `docker/Dockerfile.check` (the `FROM` line)

- [x] **Step 1:** Write `dependabot.yml` from jellyfish's (weekly, `day: monday`, `timezone: Australia/Sydney`, `cooldown: default-days: 7` with its comment, a group of `["*"]`, `commit-message: prefix: "Bumped"`, `labels: [dependencies]`) with three entries: `cargo` (directory `/`, group `cargo`), `github-actions` (directory `/`, group `actions`), and `docker` (directory `/docker`, group `docker`).
- [x] **Step 2:** Resolve the current digest of `rust:1-trixie` with `docker buildx imagetools inspect rust:1-trixie` (index digest) and change the line to `FROM rust:1-trixie@sha256:<digest>`. Keep the tag so Dependabot knows which track to follow.
- [x] **Step 3:** Validate the YAML with `python3 -c 'import sys,yaml; yaml.safe_load(open(sys.argv[1]))' .github/dependabot.yml` (no output). Then `cargo xtask lint-name` (PASS; no name in the Dockerfile).
- [x] **Step 4 (Review Focus 5):** `cargo xtask linux-check` builds from the pinned digest and prints the `linux-check: build OK, ...` summary.
- [x] **Step 5:** Commit both files with "Added Dependabot and pinned the check image".

### Task 5: Changelog and community documents

**Files:**
- Create: `CHANGELOG.md`, `scripts/changelog-version.sh`, `scripts/changelog-version-test.sh`, `CONTRIBUTING.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`, `.github/ISSUE_TEMPLATE/bug_report.yml`, `.github/ISSUE_TEMPLATE/feature_request.yml`, `.github/ISSUE_TEMPLATE/config.yml`, `.github/PULL_REQUEST_TEMPLATE.md`

**Interfaces:**
- Produces: `scripts/changelog-version.sh version | notes X.Y.Z | check`, jellyfish's contract with one change. With no released heading, `version` prints nothing and exits **3** (meaning "no release yet"), and `check` passes, printing `changelog: no release yet`, provided there is an `## [Unreleased]` heading. A missing `[Unreleased]` with no released heading still fails. Task 13 relies on exit code 3.

- [x] **Step 1: Write the failing test** `scripts/changelog-version-test.sh` (bash, `set -euo pipefail`). It builds fixtures in `mktemp -d`, runs the script with `CHANGELOG_FILE=` and runs `git init` in the temp dir for the tag cases. Cases:
  - Only `## [Unreleased]`: `check` exits 0 and prints `changelog: no release yet`. `version` exits 3.
  - No headings at all: `check` exits non-zero.
  - `## [0.9.0] - 2026-11-31`: `check` fails with `invalid date`.
  - `## [0.9.0] - 2026-11-20` with no `[0.9.0]: https://...` reference: `check` fails with `no '[0.9.0]: <url>' link reference`.
  - A valid `0.9.0` with a `v0.10.0` tag in the temp repo: `check` fails with `behind tag`.
  - A valid `0.9.0` and `0.8.0`: `version` prints `0.9.0`. `notes 0.9.0` prints the 0.9.0 body only, not the link block.

  Print `PASS <case>` and `FAIL <case>` lines and exit non-zero on any failure.
- [x] **Step 2: Run** `bash scripts/changelog-version-test.sh`. Expected: FAIL (the script doesn't exist).
- [x] **Step 3:** Copy jellyfish's `scripts/changelog-version.sh` and add the no-release behaviour from the Interfaces block. It must not contain the project name.
- [x] **Step 4: Run** the test (all PASS), then `cargo xtask lint-name` (PASS).
- [x] **Step 5:** Write `CHANGELOG.md` with jellyfish's header (Keep a Changelog 1.1.0, SemVer 2.0.0). Add an `## [Unreleased]` section with `### Added` entries for the user visible 0.9 feature set. Take the summary from `docs/superpowers/specs/2026-11-03-vimmed-v0.9-features-design.md`, at 10 lines or fewer. No released heading yet: Keith decides when 0.9.0 ships.
- [x] **Step 6:** Write `CONTRIBUTING.md` modelled on jellyfish's. Before you start: open an issue for anything non trivial, security goes through SECURITY.md, follow the code of conduct. Making a change: fork and branch, `cargo xtask ci` (and `cargo xtask linux-check` with Docker for Linux affecting changes), changelog line under `[Unreleased]`. PRs land as merge commits with signed commits, the `CI result` check must pass, and there is no reviewer requirement. Commits: past tense, about 50 characters, Australian English in prose. A "Tool versions" list (cargo-deny so far). Include the licence line from go-blinkstick. The "How releases happen" section is written in Task 13.
- [x] **Step 7:** Write `SECURITY.md` from jellyfish's, with these sections. Scope: vimmed opens and writes local files, reads a config file and spawns its own window process. Bugs that let a crafted markdown file or config execute code, write outside the opened file, or crash in a way an attacker controls are security bugs. Supported versions: the newest release only. Reporting: the private advisory link `https://github.com/bawdo/vimmed/security/advisories/new`, acknowledgement within 7 days. "Verifying a release" is added in Task 13.
- [x] **Step 8:** Copy `CODE_OF_CONDUCT.md` from jellyfish verbatim, changing only the enforcement contact if it names jellyfish.
- [x] **Step 9:** Write the issue forms. `bug_report.yml`: what happened (required), how to reproduce with a sample markdown file (required), OS and version (required, placeholder `macOS 27.2, Fedora 43`), Linux session (dropdown: Wayland, X11, not Linux), desktop environment (input), the output of `<name> --version` (required, with the field label "vimmed version"), config file contents (textarea, `render: toml`), anything else. `feature_request.yml` from jellyfish's, reworded for an editor. `config.yml`: blank issues off, plus the security advisory contact link. `PULL_REQUEST_TEMPLATE.md`: jellyfish's, with `cargo xtask ci` in place of `make pre-ci`.
- [x] **Step 10:** Validate the YAML with the same `python3` yaml check for each `.yml`. Run `cargo xtask ci` (PASS).
- [x] **Step 11:** Commit with "Added the changelog and community documents".

### Task 6: Scorecard workflow

**Files:**
- Create: `.github/workflows/scorecard.yml`

- [x] **Step 1:** Copy jellyfish's `.github/workflows/scorecard.yml` verbatim (pins are in Global Constraints).
- [x] **Step 2:** `actionlint .github/workflows/scorecard.yml` prints nothing.
- [x] **Step 3:** Commit with "Added the OpenSSF Scorecard workflow".

### Task 7: GitHub settings as code

**Files:**
- Create: `.github/rulesets/main.json`, `.github/rulesets/tags.json`, `.github/settings.md`

- [x] **Step 1:** Write `main.json`:

```json
{
  "name": "main",
  "target": "branch",
  "enforcement": "active",
  "conditions": { "ref_name": { "include": ["~DEFAULT_BRANCH"], "exclude": [] } },
  "bypass_actors": [],
  "rules": [
    { "type": "deletion" },
    { "type": "non_fast_forward" },
    { "type": "required_signatures" },
    { "type": "pull_request", "parameters": {
        "required_approving_review_count": 0,
        "dismiss_stale_reviews_on_push": false,
        "require_code_owner_review": false,
        "require_last_push_approval": false,
        "required_review_thread_resolution": false,
        "allowed_merge_methods": ["merge"] } },
    { "type": "required_status_checks", "parameters": {
        "strict_required_status_checks_policy": false,
        "required_status_checks": [ { "context": "CI result" } ] } }
  ]
}
```

- [x] **Step 2:** Write `tags.json`: `"target": "tag"`, include `["refs/tags/v*"]`, rules `deletion`, `non_fast_forward` and `update`. Do not use `creation`: the release workflow creates tags.
- [x] **Step 3:** Write `.github/settings.md`, a short runbook of the exact commands Keith runs, in order, and why each matters:

```sh
gh repo edit bawdo/vimmed --enable-wiki=false --enable-merge-commit \
  --enable-squash-merge=false --enable-rebase-merge=false --delete-branch-on-merge \
  --add-topic rust,markdown,editor,vim,gtk4,macos,linux
gh api -X PUT repos/bawdo/vimmed/private-vulnerability-reporting
gh api -X PATCH repos/bawdo/vimmed --input - <<'JSON'
{"security_and_analysis":{"secret_scanning":{"status":"enabled"},"secret_scanning_push_protection":{"status":"enabled"}}}
JSON
gh api -X PUT repos/bawdo/vimmed/vulnerability-alerts
gh api -X PATCH repos/bawdo/vimmed/code-scanning/default-setup -f state=configured
# Only after the Tier 1 PR has merged, so the "CI result" check exists on main:
gh api -X POST repos/bawdo/vimmed/rulesets --input .github/rulesets/main.json
gh api -X POST repos/bawdo/vimmed/rulesets --input .github/rulesets/tags.json
```

Add a note that rulesets are updated with `PUT repos/bawdo/vimmed/rulesets/<id>` (find the id with `gh api repos/bawdo/vimmed/rulesets`), so the JSON in the repo stays the source of truth.
- [x] **Step 4:** Check both JSON files parse with `python3 -m json.tool .github/rulesets/main.json > /dev/null` (and the same for `tags.json`).
- [x] **Step 5:** Commit with "Added the GitHub settings runbook and rulesets".

## PAUSE 1: end of Tier 1

- [x] Every Task 1 to 7 box above is ticked, the ticks are committed, `git status` is clean, and `git verify-commit HEAD` is good.
- [x] Resume log entry added.
- [x] Keith told what to do: push `chore/repo-tier-1`, open the PR, watch CI. Expect `macOS`, `Linux`, `Linux container smoke`, `cargo-deny`, `Changelog` and `CI result` to go green. Merge with a merge commit, then run `.github/settings.md` top to bottom, rulesets last.
- [x] **Context check asked; Keith's answer recorded in the Resume log.**

**Ready to resume Tier 2 when:**
- [x] The Tier 1 PR is merged and `main` CI is green. (PR #2 merged as 019888f with every check green. `main` itself has no CI run yet: `ci.yml` has no push trigger until `release.yml` calls it in Task 13.)
- [x] `gh api repos/bawdo/vimmed/rulesets` lists `main` and the tag ruleset (Keith runs it, or pastes the output). (main id 26606401, tags id 26606413.)
- [x] The first Dependabot PRs (if any) ran CI green (Review Focus 5). (None yet; Keith handles them separately.)

---

# Tier 2: vimmed specific

Branch: `git switch main && git pull` is Keith's to run. Then `git switch -c chore/repo-tier-2`.

### Task 8: README for users, manual checks moved

**Files:**
- Create: `docs/manual-checks.md`
- Modify: `README.md`

- [x] **Step 1:** Move everything from `## Manual checks` to the end of the file (the "Test file", "Mouse (Task 38b)", "Editing and file state", "Config and theme", "Linux", "Checkpoint 3" and "Fonts on macOS" sections) into `docs/manual-checks.md` under a short intro saying these are the hand checks to run before a release. Keep the text unchanged except for heading levels.
- [x] **Step 2:** In `README.md`, replace the moved block with three short sections: `## Contributing` (link to CONTRIBUTING.md and docs/manual-checks.md), `## Security` (link to SECURITY.md) and `## Licence` (MIT, link to LICENSE; the bundled test fonts are under the SIL OFL 1.1, with links to the `crates/editor-render/tests/fonts/*-OFL.txt` files). No name literal in body text.
- [x] **Step 3:** `cargo xtask lint-name` (PASS). Check every relative link resolves with `grep -o '](\([^)]*\))' README.md docs/manual-checks.md`, inspecting each target.
- [x] **Step 4:** Commit with "Moved the manual checks out of the README".

### Task 9: MSRV tested in CI

**Files:**
- Modify: `Cargo.toml` (`rust-version`, only if needed), `.github/workflows/ci.yml`

- [x] **Step 1:** Run `rustup toolchain install 1.92 --profile minimal`, then `cargo +1.92 check --workspace --all-targets --locked`.
- [x] **Step 2:** If it fails, try successive minor versions up to 1.99 and find the lowest N that passes. Set `rust-version = "1.N"` and record the first error 1.92 hit in the commit body. If 1.92 passes, leave it.
- [x] **Step 3:** Add a job `msrv` (name `MSRV`) to `ci.yml` with matrix `os: [macos-latest, ubuntu-latest]` (ubuntu installs `libgtk-4-dev libfontconfig-dev`). It reads the version with `cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].rust_version'`, installs it with rustup, and runs `cargo +$MSRV check --workspace --all-targets --locked`. Skip it on `schedule`. Add `msrv` to `ci-result`'s `needs`, to `all`, and to the schedule `may_skip`.
- [x] **Step 4:** `actionlint` prints nothing. Re-run the Task 3 Step 4 jq gate check with `msrv` in the sample.
- [x] **Step 5:** Commit with "Tested the minimum supported Rust in CI" (or "Raised the MSRV to 1.N and tested it in CI").

### Task 10: Workspace lints

**Files:**
- Modify: `Cargo.toml`, every member's `Cargo.toml`, and Rust sources only where a new lint fires

- [x] **Step 1:** Add to the root manifest:

```toml
[workspace.lints.rust]
unsafe_op_in_unsafe_fn = "deny"
unused_qualifications = "warn"

[workspace.lints.clippy]
dbg_macro = "warn"
todo = "warn"
undocumented_unsafe_blocks = "warn"
```

and `[lints] workspace = true` to every member.
- [x] **Step 2:** Run `cargo clippy --workspace --all-targets -- -D warnings` on macOS, and in the container via `cargo xtask linux-check`. Fix the hits. A `// SAFETY:` comment must state the real invariant, never filler. If `undocumented_unsafe_blocks` fires more than 30 times, stop and ask Keith whether to drop it or take the time. (0 hits: every unsafe block already had a SAFETY comment. 33 `unused_qualifications` hits fixed. The container clippy run is proven by the PR's Linux jobs, since Docker is unreachable from the sandbox.)
- [x] **Step 3:** `cargo xtask ci` (PASS) and `cargo xtask linux-check` (PASS).
- [x] **Step 4:** Commit with "Added workspace lints".

### Task 11: zizmor

**Files:**
- Modify: `.github/workflows/ci.yml`, and any workflow zizmor flags

- [x] **Step 1:** Install with `cargo install --locked zizmor --version <latest, 14 days or older>` (add it to the Tool versions in CONTRIBUTING.md) and run `zizmor --persona regular .github/workflows`.
- [x] **Step 2:** Fix each finding. Where one is a deliberate choice, suppress it inline with `# zizmor: ignore[<rule>]` plus a reason on the same line.
- [x] **Step 3:** Add a job `zizmor` (name `zizmor`) using `zizmorcore/zizmor-action` (pin in Global Constraints) with `permissions: contents: read, security-events: write`. It runs on every trigger, so it is not in any `may_skip` list. Add it to `needs` and `all`.
- [x] **Step 4:** `zizmor .github/workflows` reports no findings. `actionlint` prints nothing.
- [x] **Step 5:** Commit with "Added zizmor workflow checks".

### Task 12: Validate the Linux packaging metadata

**Files:**
- Create: `xtask/src/validate_packaging.rs`
- Modify: `xtask/src/main.rs` (subcommand `validate-packaging`, usage string), `docker/Dockerfile.check` (add `appstream desktop-file-utils` to the `apt-get install` list), `xtask/src/ci.rs` (`linux_check` runs `cargo xtask validate-packaging` in the container after the build)

**Interfaces:**
- Consumes: `templates::render_file(root: &Path, file: &str, n: &Names) -> Result<String, String>` and `templates::current_names() -> Names`.
- Produces: `pub fn run(root: &Path) -> Result<(), String>`, which renders `packaging/templates/app.desktop.in` and `metainfo.xml.in` to a temp dir as `<BUNDLE_ID>.desktop` and `<BUNDLE_ID>.metainfo.xml` and runs `desktop-file-validate <file>` and `appstreamcli validate --no-net --explain <file>`. Also `pub fn rendered_names(n: &Names) -> (String, String)` for the two file names.

- [x] **Step 1: Write the failing test** in `validate_packaging.rs`: `rendered_names_use_bundle_id` builds `Names::from_slug("abc_def", "com.example")` and asserts the two names are `format!("{}.desktop", n.bundle_id)` and `format!("{}.metainfo.xml", n.bundle_id)`. Use the real field name `Names` has; check `crates/app-meta/src/derive.rs`.
- [x] **Step 2: Run** `cargo test -p xtask validate_packaging`. Expected: FAIL.
- [x] **Step 3:** Implement `rendered_names` and `run`. If either validator is missing, `run` returns `Err("validate-packaging: <tool> not found; run it in the check container")`. Wire up the subcommand and the `linux_check` call.
- [x] **Step 4:** Run `cargo test -p xtask` (PASS), then `cargo xtask linux-check`. Expected: validator output with no errors, then the usual summary. Negative check: temporarily delete the `<summary>` element from `metainfo.xml.in`, confirm `linux-check` fails naming it, then restore it. (Not run: no Docker or validators in the sandbox. The PR's `Linux container smoke` job runs the validators; the negative check is Keith's to run locally.)
- [x] **Step 5:** `cargo xtask ci` (PASS, including `lint-name`). Commit with "Validated the desktop and AppStream files".

### Task 13: Changelog driven, signed and attested releases

**Prerequisite:** Keith has finished `~/Desktop/apple-notarisation-HOWTO.md` and `gh secret list --env release -R bawdo/vimmed` shows the five secrets. Ask before starting. The workflow can be written without them, but it must not be merged without them.

**Files:**
- Create: `.github/workflows/release.yml`, `scripts/release-commit.sh`
- Modify: `CONTRIBUTING.md` ("How releases happen", from jellyfish), `SECURITY.md` ("Verifying a release"), `README.md` (macOS install: the notarised `.app` download as an alternative to building)

**Interfaces:**
- Consumes: `.github/workflows/ci.yml` via `workflow_call`, `scripts/changelog-version.sh` (exit 3 means no release yet), `cargo xtask bundle-mac --release` (writes `target/dist/<BundleName>.app`), and `cargo xtask package-linux` (stages `.deb`, `.rpm` and `arch/` with the PKGBUILD and its sources under the target directory; read `xtask/src/package_linux.rs` for the exact output paths).
- Produces: release assets `<slug>_<ver>_macos_arm64.zip` (notarised and stapled `.app`), `<slug>_<ver>_amd64.deb`, `<slug>_<ver>_x86_64.rpm`, `<slug>_<ver>_PKGBUILD.tar.gz`, `checksums.txt` and `<slug>_<ver>_provenance.intoto.jsonl`. Here `<slug>` comes from `[workspace.metadata.app] name`. The macOS build is arm64 only: macOS 26 was the last release to support Intel Macs.

Jobs, each with jellyfish's comments on why it is split this way:

1. `ci`: `uses: ./.github/workflows/ci.yml`.
2. `plan` (ubuntu, needs `ci`): `fetch-depth: 0`. Run `scripts/changelog-version.sh version`. **Exit 3 sets `due=false`** (Review Focus 1). Otherwise use jellyfish's logic: `release-commit.sh`, the published release check, notes (changelog section, verify commands built from `$GITHUB_REPOSITORY`, compare link), and `slug` output from `cargo metadata`.
3. `build-mac` (`macos-latest`, `due == 'true'`, read only, **no cache**, no secrets): check out `commit`, run `cargo xtask bundle-mac --release`, run the smoke check (`<App>.app/Contents/MacOS/<slug> --version` contains the version), and upload the unsigned `.app` as an artifact (`ditto -c -k --keepParent`).
4. `build-linux` (`ubuntu-latest`, same restrictions): build the check image from `docker/Dockerfile.check` and run `cargo xtask package-linux` inside it the way `cargo xtask linux-check` runs its container (reuse `ci::run_in_container` through a new `--package` path if that is cleaner, or a `docker run` mirroring it). Smoke the `.deb` with `dpkg-deb -I` and the `.rpm` with `rpm -qip` if available. Rename the outputs to the asset names and upload them.
5. `sign-mac` (`macos-latest`, `environment: release`, **no checkout, runs no project code**): download the `.app`. Make a temporary keychain with a password from `openssl rand -base64 24`, import `MACOS_CERTIFICATE_P12` with `MACOS_CERTIFICATE_PASSWORD`, and run `security set-key-partition-list`. Run `codesign --force --options runtime --timestamp --sign "Developer ID Application: <name> (${{ vars.APPLE_TEAM_ID }})"` on the inner binaries first (`Contents/MacOS/*`), then the bundle. Then `codesign --verify --strict --deep`, zip with `ditto`, `xcrun notarytool submit --wait` using the `.p8` written to `$RUNNER_TEMP` (deleted in an `if: always()` step along with the keychain), `xcrun stapler staple`, `spctl --assess --type execute -vv` (expect `source=Notarized Developer ID`), re-zip and upload. On failure, print `notarytool log <id>`.
6. `publish` (ubuntu, needs `plan, build-linux, sign-mac`; `contents: write`, `id-token: write`, `attestations: write`; no checkout): download the assets, write `checksums.txt` (`sha256sum`), attest with `subject-checksums`, write the bundle as `.intoto.jsonl` (`jq -c`, one line), then `gh release create --draft`, upload everything and `gh release edit --draft=false --latest`. Copy jellyfish's stale draft handling as is (Review Focus 4).

- [x] **Step 1:** Copy jellyfish's `scripts/release-commit.sh` verbatim. Check it has no project name in it, then `cargo xtask lint-name`.
- [x] **Step 2:** Write `release.yml` with the six jobs. Top level `permissions: contents: read`, `on: push: branches: [main]`, concurrency `cancel-in-progress: false`.
- [x] **Step 3:** Confirm `ci-result` treats `push` (the `workflow_call` path from `release.yml`) like `pull_request`, with nothing skipped (set up in Task 3 Step 2).
- [x] **Step 4:** `actionlint` and `zizmor .github/workflows` both report nothing.
- [x] **Step 5:** Check the plan logic locally: in a scratch clone with only `[Unreleased]`, `scripts/changelog-version.sh version; echo $?` prints `3`.
- [x] **Step 6 (Review Focus 2):** `grep -rniw vimmed .github/workflows` prints nothing.
- [x] **Step 7:** Write the docs. In `CONTRIBUTING.md`, "How releases happen" (jellyfish's text, plus: run `docs/manual-checks.md` before renaming `[Unreleased]`). In `SECURITY.md`, "Verifying a release": the `gh attestation verify` commands with `--signer-workflow`, `--source-ref refs/heads/main`, `--deny-self-hosted-runners` and the `--bundle` and offline variants from jellyfish, plus `spctl --assess --type execute -vv <App>.app` for the notarisation. In `README.md`, macOS install from a release.
- [x] **Step 8 (Review Focus 4):** In the commit body, state the order in which assets become public: nothing is published until `publish` uploads every asset to a draft.
- [x] **Step 9:** `cargo xtask ci` (PASS). Commit with "Added the signed and attested release workflow".

## PAUSE 2: end of Tier 2

- [x] Every Task 8 to 13 box is ticked and committed, `git status` is clean, and `git verify-commit HEAD` is good.
- [x] Resume log entry added.
- [x] Keith told what to do: push `chore/repo-tier-2`, open the PR, CI green (now including `MSRV` and `zizmor`), merge. Merging does **not** release, because `CHANGELOG.md` has only `[Unreleased]`. To cut 0.9.0, open a PR that renames `[Unreleased]` to `[0.9.0] - <date>` with the compare link, and merge it. Then check the release page, download the zip and run the `SECURITY.md` verify commands.
- [x] **Context check asked; Keith's answer recorded in the Resume log.**

**Ready to resume Tier 3 when:**
- [x] The Tier 2 PR is merged and the `main` release run shows `plan` with `due=false` (or a successful release, if Keith cut 0.9.0). (PR #4 merged as 813b2df. Release run 42028549524 succeeded: every CI job and `plan` green, build-mac, build-linux, sign-mac and publish skipped.)
- [x] If a release was cut: `spctl` on the downloaded `.app` says `Notarized Developer ID`, and `gh attestation verify` passes. (Not applicable: no release was cut.)

---

# Tier 3: polish

Branch: `chore/repo-tier-3` from an up to date `main`.

### Task 14: typos, Australian English in prose

**Files:**
- Create: `typos.toml`
- Modify: `.github/workflows/ci.yml`, and whatever files contain real typos

- [x] **Step 1:** Write `typos.toml` with no `locale` under `[default]`, because code uses US API names such as `Color` from dependencies. Add `[type.md] extend-glob = ["*.md"]` with `locale = "en-au"`. Add `[files] extend-exclude = ["Cargo.lock", "*.svg", "packaging/icons/**", "docs/superpowers/**", "research_notes/**", "reports/**", "bench-results/**"]`.
- [x] **Step 2:** Install with `cargo install --locked typos-cli --version <latest, 14 days or older>` and run `typos`. Fix real typos. Add genuine words (crate names, the project name) to `[default.extend-words]` with the word mapped to itself. Never add a misspelling.
- [x] **Step 3:** Add a `typos` job (`crate-ci/typos` pinned) to `ci.yml`, skipped on `schedule`, and add it to `needs`, `all` and the schedule `may_skip`.
- [x] **Step 4:** `typos` prints nothing. `actionlint` and `zizmor` are clean. `cargo xtask ci` (PASS).
- [x] **Step 5:** Commit with "Added typos checks with Australian English prose".

### Task 15: .gitattributes

**Files:**
- Create: `.gitattributes`

- [x] **Step 1:** Write it with these lines: `* text=auto eol=lf`, `*.icns binary`, `*.png binary`, `*.gif binary`, `*.ttf binary`, `Cargo.lock linguist-generated=true`, `docs/superpowers/** linguist-documentation`, `research_notes/** linguist-documentation`, `reports/** linguist-documentation`.
- [x] **Step 2:** `git add --renormalize .` followed by `git status` shows no changed files other than `.gitattributes`. If files do change, stop and show Keith the list.
- [x] **Step 3:** `git check-attr -a packaging/icons/AppIcon.icns` shows `binary: set`. Commit with "Added .gitattributes".

### Task 16: AI policy

**Files:**
- Create: `AI_POLICY.md`
- Modify: `CONTRIBUTING.md` (link it), `.github/PULL_REQUEST_TEMPLATE.md` (one checkbox)

- [x] **Step 1:** Draft `AI_POLICY.md` from ripgrep's points: AI tools for code are fine, the contributor is responsible for every line and must understand it, and the maintainer is accountable for releases. Fit the human-written-communication point to Keith's own practice: an AI-assisted PR must say so in its description. **Show Keith the draft and wait for his wording before committing.** It's his stance, not ours. (Keith approved the draft after deleting its disclosure paragraph, so the policy has no PR-description disclosure rule.)
- [x] **Step 2:** Add the link in CONTRIBUTING.md and the checkbox "I have read AI_POLICY.md" to the PR template.
- [x] **Step 3:** `typos` clean. Commit with "Added an AI contribution policy".

### Task 17: Coverage, informational

**Files:**
- Create: `.codecov.yml`
- Modify: `xtask/src/ci.rs`, `xtask/src/main.rs`, `.github/workflows/ci.yml`, `CONTRIBUTING.md` (tool versions)

**Interfaces:**
- Produces: `pub fn run(root: &Path, linux: bool, coverage: bool) -> Result<(), String>` in `xtask/src/ci.rs`. When `coverage` is true, the `test` step runs `cargo llvm-cov --workspace --lcov --output-path target/lcov.info` in place of `cargo test --workspace`, so CI tests once rather than twice. `cargo xtask ci --coverage` parses the flag in `main.rs`.

- [x] **Step 1: Write the failing test** in `ci.rs`: `test_step_args(coverage: bool) -> Vec<&'static str>` returns `["test", "--workspace"]` for false and `["llvm-cov", "--workspace", "--lcov", "--output-path", "target/lcov.info"]` for true. Assert both.
- [x] **Step 2: Run** `cargo test -p xtask ci::`. Expected: FAIL.
- [x] **Step 3:** Implement `test_step_args`. Thread `coverage` through `run`, so the step list picks the test args at run time (the `STEPS` const becomes a function or the test step reads a flag). Update the `main.rs` parsing and usage string.
- [x] **Step 4:** Install with `cargo install --locked cargo-llvm-cov --version <latest, 14 days or older>` and add `llvm-tools-preview` to `rust-toolchain.toml` components. Run `cargo xtask ci --coverage`: `target/lcov.info` exists and all steps are OK. `cargo xtask ci` still passes.
- [x] **Step 5 (Review Focus 3):** In `ci.yml`, the `linux` job runs `cargo xtask ci --coverage` (installing cargo-llvm-cov with `taiki-e/install-action`, pinned), then `codecov/codecov-action` with `files: target/lcov.info`, `token: ${{ secrets.CODECOV_TOKEN }}` and `fail_ci_if_error: false`. Pass `CODECOV_TOKEN` through `workflow_call` from `release.yml`. `grep -n 'secrets\.' .github/workflows/ci.yml` shows only the declaration and this step.
- [x] **Step 6:** Write `.codecov.yml` from jellyfish's with every status `informational: true`, no 90% floor (`target: auto`), and `ignore: ["xtask/**", "crates/layout-bench/**", "**/tests/**"]`.
- [x] **Step 7:** `actionlint` and `zizmor` clean, `cargo xtask ci` (PASS). Commit with "Added informational coverage via cargo-llvm-cov".
- [x] **Step 8:** Tell Keith to add `CODECOV_TOKEN` as a repository secret (`gh secret set CODECOV_TOKEN -R bawdo/vimmed`) after enabling the repo at codecov.io. (Told at PAUSE 3. Review found `docker/Dockerfile.check` installed only rustfmt and clippy, so the image now installs `llvm-tools-preview` too.)

### Task 18: REUSE compliance

**Files:**
- Create: `REUSE.toml`, `LICENSES/MIT.txt`, `LICENSES/OFL-1.1.txt`
- Modify: `.github/workflows/ci.yml`, `CONTRIBUTING.md` (one line: new files are covered by `REUSE.toml` globs or need an SPDX header)

- [ ] **Step 1:** Install with `pipx install reuse==<latest, 14 days or older>` and run `reuse lint` to record the baseline failure count.
- [ ] **Step 2:** Run `reuse download MIT OFL-1.1`. Keep the root `LICENSE` for GitHub's licence detection; it is identical in substance to `LICENSES/MIT.txt`.
- [ ] **Step 3:** Write `REUSE.toml` (version 1) with these annotations:
  - `**` under `MIT`, `SPDX-FileCopyrightText = "2026 Keith Bawden"`.
  - `crates/editor-render/tests/fonts/*.ttf` under `OFL-1.1`, with the copyright line taken from each font's `*-OFL.txt` (first line).
  - `crates/editor-render/tests/fonts/*-OFL.txt` under `OFL-1.1`.

  Use `precedence = "override"` for the font entries.
- [ ] **Step 4:** `reuse lint` ends with `Congratulations! Your project is compliant`.
- [ ] **Step 5:** Add a `reuse` job (`fsfe/reuse-action` pinned) to `ci.yml`, skipped on `schedule`, and add it to `needs`, `all` and the schedule `may_skip`. `actionlint` and `zizmor` clean.
- [ ] **Step 6:** Commit with "Made the repository REUSE compliant".

### Task 19: Whole branch review

- [ ] **Step 1:** Run superpowers:requesting-code-review over `main..chore/repo-tier-3`, and over the merged Tier 1 and 2 changes if Keith asks.
- [ ] **Step 2:** Re-run `cargo xtask ci` and `cargo xtask linux-check` yourself and paste the summary lines.
- [ ] **Step 3:** Update the CONTRIBUTING.md Tool versions list against what was actually installed.

## PAUSE 3: end of Tier 3

- [ ] Every Task 14 to 19 box is ticked and committed, `git status` is clean, and `git verify-commit HEAD` is good.
- [ ] Resume log entry added.
- [ ] Keith told what to do: push `chore/repo-tier-3`, open the PR, CI green (now including `typos` and `reuse`), merge, set `CODECOV_TOKEN`. Scorecard results appear at `https://scorecard.dev/viewer/?uri=github.com/bawdo/vimmed` after the next weekly run or push to `main`.
- [ ] **Context check asked; Keith's answer recorded in the Resume log.**

---

## Resume log

| Date | Tier | Branch | Last commit | Notes for Keith |
|---|---|---|---|---|
| 2026-11-05 | 1 | chore/repo-tier-1 | ae14cc7 (this log entry follows it) | Push the branch, open the PR, expect macOS, Linux, Linux container smoke, cargo-deny, Changelog and CI result green, merge with a merge commit, then run .github/settings.md top to bottom (rulesets last). Local `cargo xtask ci` fails at fmt only because of the gitignored crates/editor-render/tests/demo_reel.rs: run `cargo fmt --all` locally. The git-guard reference-transaction hook blocks cargo-deny's advisory-db self-update (use `cargo deny check --offline` after the first fetch). RUSTSEC-2026-0192 (ttf-parser unmaintained) is ignored in deny.toml with a reason. Docker is unreachable from the sandbox, so `cargo xtask linux-check` with the pinned digest is first proven by the PR's container smoke job. Subagents no longer commit; the controller does (Keith, 2026-11-05). |
| 2026-11-05 | 1 (context check) | chore/repo-tier-1 | c1837c3, merged as 019888f | Keith: stop here, resume in a fresh session. Tier 2 resumed on chore/repo-tier-2 from 019888f. |
| 2026-11-05 | 2 | chore/repo-tier-2 | 110327a (this log entry follows it) | Push the branch, open the PR, expect macOS, Linux, Linux container smoke, MSRV (macOS and Ubuntu), cargo-deny, zizmor, Changelog and CI result green, merge with a merge commit. The merge runs release.yml for the first time: expect `plan` with due=false. Before cutting 0.9.0, add a deployment branch rule limiting the `release` environment to `main` (`gh api repos/bawdo/vimmed/environments/release` to check). To release, bump `[workspace.package] version` with the changelog heading (CI now fails a mismatch). Run `cargo xtask linux-check` locally once, plus Task 12's negative check (delete `<summary>` from metainfo.xml.in, expect failure, restore). zizmor is 1.30.1 because the pinned zizmor-action supports nothing newer. Inside the sandbox rustup needs `RUSTUP_USE_CURL=1`. |
| 2026-11-05 | 2 (context check) | chore/repo-tier-2 | 925f74a, then 7599044, merged as 813b2df | Keith: stop here, resume in a fresh session. Tier 3 resumed on chore/repo-tier-3 from 813b2df. Still owed before 0.9.0: the `release` environment deployment branch rule (main only), and Task 12's negative check run locally. |
