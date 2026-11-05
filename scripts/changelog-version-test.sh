#!/usr/bin/env bash
#
# Tests scripts/changelog-version.sh against fixture changelogs built in a
# temporary directory. Run from the repository root:
#   bash scripts/changelog-version-test.sh

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT="$HERE/changelog-version.sh"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/changelog-test.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

failures=0
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1${2:+: $2}"; failures=$((failures + 1)); }

# new_case NAME: makes a fresh git repo in $WORK/NAME and prints its path.
new_case() {
    local dir="$WORK/$1"
    mkdir -p "$dir"
    git -C "$dir" init -q
    printf '%s' "$dir"
}

# run_script DIR ARGS...: runs the script in DIR. Sets OUT (stdout and stderr)
# and RC (exit code).
run_script() {
    local dir="$1"
    shift
    set +e
    OUT=$(cd "$dir" && CHANGELOG_FILE="$dir/CHANGELOG.md" bash "$SCRIPT" "$@" 2>&1)
    RC=$?
    set -e
}

HEADER=$'# Changelog\n'

case_unreleased_only() {
    local name="only unreleased"
    local d
    d=$(new_case unreleased)
    printf '%s\n## [Unreleased]\n\n### Added\n\n- A thing.\n' "$HEADER" > "$d/CHANGELOG.md"

    run_script "$d" check
    if [ "$RC" -eq 0 ] && [[ "$OUT" == *"changelog: no release yet"* ]]; then
        pass "$name: check passes"
    else
        fail "$name: check passes" "rc=$RC out=$OUT"
    fi

    run_script "$d" version
    if [ "$RC" -eq 3 ] && [ -z "$OUT" ]; then
        pass "$name: version exits 3"
    else
        fail "$name: version exits 3" "rc=$RC out=$OUT"
    fi
}

case_no_headings() {
    local d
    d=$(new_case noheadings)
    printf '%s\nNothing here yet.\n' "$HEADER" > "$d/CHANGELOG.md"

    run_script "$d" check
    if [ "$RC" -eq 1 ] && [[ "$OUT" == *"no '## [Unreleased]'"* ]]; then
        pass "no headings: check fails"
    else
        fail "no headings: check fails" "rc=$RC out=$OUT"
    fi
}

case_invalid_date() {
    local d
    d=$(new_case baddate)
    printf '%s\n## [Unreleased]\n\n## [0.9.0] - 2026-11-31\n\n- A thing.\n\n[0.9.0]: https://example.com/0.9.0\n' \
        "$HEADER" > "$d/CHANGELOG.md"

    run_script "$d" check
    if [ "$RC" -ne 0 ] && [[ "$OUT" == *"invalid date"* ]]; then
        pass "invalid date: check fails"
    else
        fail "invalid date: check fails" "rc=$RC out=$OUT"
    fi
}

case_missing_link() {
    local d
    d=$(new_case nolink)
    printf '%s\n## [Unreleased]\n\n## [0.9.0] - 2026-11-20\n\n- A thing.\n' \
        "$HEADER" > "$d/CHANGELOG.md"

    run_script "$d" check
    if [ "$RC" -ne 0 ] && [[ "$OUT" == *"no '[0.9.0]: <url>' link reference"* ]]; then
        pass "missing link reference: check fails"
    else
        fail "missing link reference: check fails" "rc=$RC out=$OUT"
    fi
}

case_behind_tag() {
    local d
    d=$(new_case behindtag)
    printf '%s\n## [Unreleased]\n\n## [0.9.0] - 2026-11-20\n\n- A thing.\n\n[0.9.0]: https://example.com/0.9.0\n' \
        "$HEADER" > "$d/CHANGELOG.md"
    git -C "$d" -c user.name=t -c user.email=t@example.com -c commit.gpgsign=false \
        -c tag.gpgsign=false commit -q --allow-empty -m "Initial"
    git -C "$d" -c tag.gpgsign=false tag v0.10.0

    run_script "$d" check
    if [ "$RC" -ne 0 ] && [[ "$OUT" == *"behind tag"* ]]; then
        pass "behind tag: check fails"
    else
        fail "behind tag: check fails" "rc=$RC out=$OUT"
    fi
}

case_version_and_notes() {
    local d
    d=$(new_case notes)
    cat > "$d/CHANGELOG.md" <<'FIXTURE'
# Changelog

## [Unreleased]

## [0.9.0] - 2026-11-20

### Added

- Nine point oh.

## [0.8.0] - 2026-10-01

- Eight point oh.

[Unreleased]: https://example.com/compare/v0.9.0...HEAD
[0.9.0]: https://example.com/compare/v0.8.0...v0.9.0
[0.8.0]: https://example.com/releases/tag/v0.8.0
FIXTURE

    run_script "$d" version
    if [ "$RC" -eq 0 ] && [ "$OUT" = "0.9.0" ]; then
        pass "version and notes: version prints 0.9.0"
    else
        fail "version and notes: version prints 0.9.0" "rc=$RC out=$OUT"
    fi

    run_script "$d" notes 0.9.0
    if [ "$RC" -eq 0 ] && [[ "$OUT" == *"Nine point oh."* ]] \
        && [[ "$OUT" != *"Eight point oh."* ]] && [[ "$OUT" != *"https://"* ]]; then
        pass "version and notes: notes prints the body only"
    else
        fail "version and notes: notes prints the body only" "rc=$RC out=$OUT"
    fi
}

case_malformed_heading() {
    local label heading d
    for heading in '## [0.9.0] - 2026-11-5' '## [v0.9.0] - 2026-11-20' '## [0.9.0] - 2026-11-20 '; do
        label="malformed heading '$heading'"
        d=$(new_case malformed)
        printf '%s\n## [Unreleased]\n\n%s\n\n- A thing.\n\n[0.9.0]: https://example.com/0.9.0\n' \
            "$HEADER" "$heading" > "$d/CHANGELOG.md"

        run_script "$d" check
        if [ "$RC" -eq 1 ] && [[ "$OUT" == *"malformed release heading"* ]]; then
            pass "$label: check fails"
        else
            fail "$label: check fails" "rc=$RC out=$OUT"
        fi

        run_script "$d" version
        if [ "$RC" -eq 1 ]; then
            pass "$label: version exits 1, not 3"
        else
            fail "$label: version exits 1, not 3" "rc=$RC out=$OUT"
        fi
    done
}

case_unreleased_only
case_no_headings
case_malformed_heading
case_invalid_date
case_missing_link
case_behind_tag
case_version_and_notes

if [ "$failures" -ne 0 ]; then
    echo "$failures case(s) failed"
    exit 1
fi
echo "all cases passed"
