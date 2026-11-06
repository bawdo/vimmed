# GitHub settings runbook

Settings for `bawdo/vimmed`, applied by hand with `gh` (authenticated as a repo admin). Run top to bottom.

Merge commits only, no wiki, branches deleted after merge. Merge commit only keeps your own signed commits in history unchanged (squash and rebase replace them with new commits made by GitHub).

```sh
gh repo edit bawdo/vimmed --enable-wiki=false --enable-merge-commit \
  --enable-squash-merge=false --enable-rebase-merge=false --delete-branch-on-merge \
  --add-topic rust,markdown,editor,vim,gtk4,macos,linux
```

Private vulnerability reporting backs the advisory link in SECURITY.md.

```sh
gh api -X PUT repos/bawdo/vimmed/private-vulnerability-reporting
```

Secret scanning with push protection stops secrets landing in the first place.

```sh
gh api -X PATCH repos/bawdo/vimmed --input - <<'JSON'
{"security_and_analysis":{"secret_scanning":{"status":"enabled"},"secret_scanning_push_protection":{"status":"enabled"}}}
JSON
```

Vulnerability alerts feed Dependabot security updates.

```sh
gh api -X PUT repos/bawdo/vimmed/vulnerability-alerts
```

Code scanning runs from `.github/workflows/codeql.yml` (CodeQL advanced setup), so leave CodeQL default setup off: while it is on, GitHub rejects the results the workflow uploads. Check it reads `not-configured`.

```sh
gh api repos/bawdo/vimmed/code-scanning/default-setup --jq .state
```

Rulesets go last. The "CI result" required check must already exist on main, so run these only after the Tier 1 PR has merged.

```sh
gh api -X POST repos/bawdo/vimmed/rulesets --input .github/rulesets/main.json
gh api -X POST repos/bawdo/vimmed/rulesets --input .github/rulesets/tags.json
```

## Updating rulesets

Edit the JSON in `.github/rulesets/`, then apply it with `PUT repos/bawdo/vimmed/rulesets/<id>`. Find the id with `gh api repos/bawdo/vimmed/rulesets`. That keeps the JSON in the repo as the source of truth.
