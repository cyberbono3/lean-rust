#!/usr/bin/env bash
#
# check-changelog.sh — assert a pull request records its change in CHANGELOG.md.
#
# The changelog is written by the PR that makes the change, before it merges. There
# is no release-time write-up step: a change that merges without its entry is a
# change nobody reconstructs later, which is the state this repository was in for
# its first 151 commits. This guard is the mechanical half of that rule; the
# CHANGELOG.md header comment and the PR template are the human half.
#
# Scope is deliberately ONE assertion: the branch touched the file. It does not
# validate the file's structure — headings, categories, entry shape and reference
# links are review concerns. A structural parser here would have to skip the file's
# own fenced header comment (which quotes the entry shape) and would be defeated by
# a one-character edit anyway.
#
# The diff is taken against the MERGE BASE rather than the base tip, and the work
# tree rather than HEAD is its right-hand side. That choice is about LOCAL runs:
# a base-tip diff false-PASSes once the base branch has changelog commits of its own
# (the branch then differs from the base tip without having touched the file), and
# a HEAD-anchored diff cannot see an entry that is written but not yet committed.
# In CI the choice is neutral: on a pull_request event actions/checkout resolves
# refs/pull/N/merge, a merge commit whose parent IS the base commit the merge ref was
# built on, so merge-base returns that commit and the base-moved false-PASS cannot
# arise there — the merge ref has already absorbed the base's changelog commits.
#
# KNOWN GAP — this guard is not a security control. The workflow runs THIS file from
# the pull request's own checkout, so a PR that edits this script edits its own judge.
# That is deliberate and matches every sibling guard in ci.yml; the purpose is catching
# an honest omission, not resisting a determined author. Before making this job a
# required status check, either accept that or run the guard from a base-ref copy.
#
# KNOWN GAP — the label match is case-insensitive. The workflow tests the label with
# GitHub Actions' contains(), which is documented as not case sensitive, so a label
# named "No Changelog" also skips the check even though every document here pins the
# exact string "no changelog". Applying a label needs write access, so this widens the
# escape hatch rather than opening one.
#
# KNOWN GAP — stacked PRs. .github/workflows/changelog.yml triggers only on pull
# requests targeting main/master, so a PR that targets a feature branch runs no CI
# and this requirement is unenforced there. It first fires when the stack's base PR
# opens against main. A consequence to weigh before making this job a required
# status check: on such a PR the check shows as never-run rather than skipped, which
# blocks merge. Widening the trigger branches is a separate change with its own cost.
#
# Usage:   BASE_REF=<base> [NO_CHANGELOG_LABEL=true|false] scripts/check-changelog.sh [CHANGELOG_FILE...]
#          scripts/check-changelog.sh --help
# Exit:    0 = every required changelog was touched (or the label is set)
#          1 = a required changelog was untouched, the base could not be resolved,
#              or git itself failed
# Tests:   scripts/tests/test-check-changelog.sh

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

CHANGELOG_FILE="CHANGELOG.md"
LABEL="no changelog"

fail() { printf '[check-changelog.sh] FAIL: %s\n' "$1" >&2; exit 1; }
info() { printf '[check-changelog.sh] INFO: %s\n' "$1"; }

usage() {
    cat <<EOF
Usage: BASE_REF=<base> NO_CHANGELOG_LABEL=<true|false> $0 [CHANGELOG_FILE...]

Asserts that the current branch touches each CHANGELOG_FILE relative to its merge
base with origin/\$BASE_REF. With no arguments, ${CHANGELOG_FILE} is required.

Set NO_CHANGELOG_LABEL=true (CI passes this when the PR carries the "${LABEL}"
label) to skip the check for a trivial change.
EOF
}

# --help is answered before anything else, so it works regardless of environment.
if [ "${1:-}" = "--help" ]; then
    usage
    exit 0
fi

# Every remaining argument is a changelog path. A typo'd flag must not be silently
# treated as a required file — and must not be swallowed by the label short-circuit
# either, so this runs before it. Argument validity depends on nothing else.
for arg in "$@"; do
    case "$arg" in
        -*) fail "unknown argument: ${arg} (accepts --help or changelog paths)" ;;
    esac
done

# The label is read before the base ref is resolved, so a labelled PR passes even
# when the base ref cannot be fetched. That is what an escape hatch is for.
if [ "${NO_CHANGELOG_LABEL:-false}" = "true" ]; then
    info "\"${LABEL}\" label is set — changelog entry not required"
    exit 0
fi

# An unset BASE_REF means the harness is wrong, not that the check is inapplicable.
# Degrading to a skip here is how a guard silently stops guarding. Tested by the
# guard's own convention rather than bash's ${var:?} diagnostic, which would bypass
# the [check-changelog.sh] LABEL: prefix.
[ -n "${BASE_REF:-}" ] || fail "BASE_REF is not set — pass the PR base branch, e.g. BASE_REF=main"

BASE="refs/remotes/origin/${BASE_REF#origin/}"

git rev-parse --verify --quiet "${BASE}^{commit}" >/dev/null \
    || fail "cannot resolve ${BASE}. In CI this means the checkout was shallow — set 'fetch-depth: 0' on actions/checkout. Locally, run 'git fetch origin ${BASE_REF#origin/}'."

MERGE_BASE="$(git merge-base "${BASE}" HEAD)" \
    || fail "no merge base between ${BASE} and HEAD"

require_changelog() {
    local changelog_file="$1" rc=0

    # git diff --quiet: 0 = no difference, 1 = difference, >1 = git itself failed.
    # Treating every non-zero status as "there is a diff" would make a bad pathspec
    # or a corrupt object PASS, which is the fail-open the sibling guards forbid.
    git diff --quiet "${MERGE_BASE}" -- "${changelog_file}" || rc=$?

    case "$rc" in
        0) if ! git cat-file -e "${MERGE_BASE}:${changelog_file}" 2>/dev/null \
              && [ ! -e "${changelog_file}" ]; then
               fail "${changelog_file} does not exist at ${BASE} or in this branch. This is not a missing entry — the file itself is gone. Restore it, or pass the correct path as an argument."
           fi
           fail "this branch does not touch ${changelog_file}. The PR that makes a change writes its own entry, before it merges: add one line under the current '## vX.Y.Z (Unreleased)' section describing the effect an operator would notice, ending in ([#N](<issue-or-pull-url>)). If the change genuinely needs no entry, label the PR \"${LABEL}\"." ;;
        1) info "${changelog_file} has been updated" ;;
        *) fail "git diff failed (exit ${rc}) for ${changelog_file}" ;;
    esac
}

if [ "$#" -gt 0 ]; then
    for changelog_file in "$@"; do
        require_changelog "${changelog_file}"
    done
    exit 0
fi

require_changelog "${CHANGELOG_FILE}"
