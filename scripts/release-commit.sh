#!/usr/bin/env bash
# Print the commit a release of TAG should be built from, given HEAD, the
# main commit the release workflow is running for.
#
#   scripts/release-commit.sh v1.2.3 "$GITHUB_SHA"
#
# If TAG exists, its commit is used, but only when that commit is HEAD or an
# ancestor of it. A tag on a commit that never reached main (a side branch, a
# local commit pushed only as a tag) would otherwise be built, released and
# attested as if it came from main, without the review or CI main gets.
# If TAG does not exist, HEAD is used and the release will create the tag.
#
# Needs full history (fetch-depth: 0) to answer the ancestry question.
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <tag> <head-commit>" >&2
  exit 2
fi
tag=$1
head=$2

if commit=$(git rev-parse -q --verify "refs/tags/$tag^{commit}"); then
  if ! git merge-base --is-ancestor "$commit" "$head"; then
    echo "error: $tag points at $commit, which is not on main (not an ancestor of $head)." >&2
    echo "Refusing to build or release it. Delete or re-point the tag to a main commit." >&2
    exit 1
  fi
  echo "$tag exists at $commit, which is on main" >&2
else
  commit=$(git rev-parse --verify "$head^{commit}")
  echo "$tag is not tagged; it will be created at $commit" >&2
fi
echo "$commit"
