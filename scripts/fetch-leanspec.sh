#!/usr/bin/env bash
#
# fetch-leanspec.sh — materialise the recorded leanSpec revision at leanSpec-pq-devnet-4/.
#
# Conformance work cites the specification as `leanSpec@<rev>:<path>:<line>`. A
# citation of that shape is only checkable when the exact revision is on disk;
# without it every spec citation is unverifiable, and a reviewer must either
# take it on trust or report it as unconfirmed on every single pass. This script
# is what makes the citation resolvable, so it is a build-out of the evidence
# base, not a convenience.
#
# `README.md`'s interop table is the single place this repository records which
# revision it targets, so the revision is READ from that table rather than
# restated here. A second copy would be free to drift, and a spec pin that drifts
# is worse than none: the report would name one revision and the reader's
# checkout would hold another. For the same reason LEANSPEC_REV is deliberately
# NOT an environment override — pointing a run at an unrecorded revision produces
# findings that say nothing about the client this repository ships.
#
# The checkout is detached on purpose. A branch moves between two runs and
# quietly changes what was cited, and the HEAD assertion at the end is what makes
# the result evidence rather than an assumption.
#
# Re-running is cheap and offline once the revision is present: the commit is
# already in the object store, so no fetch is attempted and the checkout is a
# no-op. That is what allows this to be a precondition of other targets.
#
# Usage:   scripts/fetch-leanspec.sh
# Env:     LEANSPEC_DIR — checkout path (default `leanSpec-pq-devnet-4`)
#          LEANSPEC_URL — clone source (default: the URL in the same table row;
#                         override only to point at a mirror of the same history)
# Exit:    0 = checkout is detached at the recorded revision
#          1 = the revision could not be read, fetched, or asserted
# Tests:   scripts/tests/test-fetch-leanspec.sh

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

README="README.md"
LEANSPEC_DIR="${LEANSPEC_DIR:-leanSpec-pq-devnet-4}"

# The full 40-character hash is required. README.md states why: a prefix can
# become ambiguous as an upstream repository grows, and a prefix that resolves
# today can resolve to a different commit later, which is precisely the silent
# drift the pin exists to prevent.
REV_HEX_LEN=40

# The row label is matched in full and anchored to the leading table pipe. The
# table also carries `leanSig revision` and `leanMetrics revision` rows holding
# their own 40-character hashes, so an unscoped hash match would happily pin the
# spec checkout to the signature scheme's commit and fail far away from here.
ROW_LABEL="leanSpec revision"
ROW_RE="^[[:space:]]*\\|[[:space:]]*${ROW_LABEL}[[:space:]]*\\|"

fail() { printf 'fetch-leanspec: %s\n' "$1" >&2; exit 1; }

[ -r "$README" ] || fail "missing or unreadable required file: $README — the recorded revision is read from its interop table"

# A guard that stops asserting is worse than no guard, so both "no row" and
# "more than one row" are failures. Two rows mean the table has been reshaped and
# no single value can be called the recorded one.
row_count="$(grep -cE "$ROW_RE" "$README" || true)"
[ "$row_count" = "1" ] ||
    fail "expected exactly one '| $ROW_LABEL |' row in $README, found $row_count — the interop table is the single record of the audited revision"

row="$(grep -E "$ROW_RE" "$README")"

# The value cell is taken whole, between its backticks, and validated after
# extraction. Matching 40 hex characters directly would accept a 41-character
# value by matching a substring of it, which is the off-by-one that lets a
# malformed pin through.
rev="$(printf '%s\n' "$row" | sed -n 's/^[^|]*|[^|]*|[[:space:]]*`\([^`]*\)`[[:space:]]*|.*/\1/p')"
[ -n "$rev" ] ||
    fail "could not read a backtick-quoted value from the '$ROW_LABEL' row of $README"

printf '%s' "$rev" | grep -Eq "^[0-9a-f]{${REV_HEX_LEN}}$" ||
    fail "recorded revision '$rev' is not a full ${REV_HEX_LEN}-character lowercase commit hash — $README is the only place this value is written down, so it is required to be exact"

# The source cell's first markdown link is the upstream. Reading it here keeps
# the URL and the revision in one row: a mirror override stays possible, but the
# default can never point at a repository the table does not name.
url_default="$(printf '%s\n' "$row" | sed -n 's/[^(]*(\(https:[^)]*\)).*/\1/p')"
LEANSPEC_URL="${LEANSPEC_URL:-$url_default}"
[ -n "$LEANSPEC_URL" ] ||
    fail "could not read an upstream URL from the '$ROW_LABEL' row of $README, and LEANSPEC_URL is not set"

printf 'leanSpec checkout — source of truth: %s interop table\n' "$README"
printf '  rev   %-12s %s\n' "recorded" "$rev"
printf '  url   %-12s %s\n' "upstream" "$LEANSPEC_URL"

# An existing non-repository at the target path is a failure rather than
# something to clone over or delete: it is not this script's business to remove
# a directory it did not create, and `git clone` into it would fail later with a
# message that does not say why.
if [ -e "$LEANSPEC_DIR" ] && [ ! -d "$LEANSPEC_DIR/.git" ]; then
    fail "$LEANSPEC_DIR exists but is not a git repository — remove it and re-run, or set LEANSPEC_DIR elsewhere"
fi

if [ ! -d "$LEANSPEC_DIR/.git" ]; then
    mkdir -p "$(dirname "$LEANSPEC_DIR")"
    # --no-checkout because the default branch tip is not what is wanted and
    # writing it out only to overwrite it costs a full working-tree pass.
    git clone --quiet --no-checkout "$LEANSPEC_URL" "$LEANSPEC_DIR" ||
        fail "could not clone $LEANSPEC_URL into $LEANSPEC_DIR"
    printf '  ok    %-12s %s\n' "clone" "$LEANSPEC_DIR"
else
    printf '  ok    %-12s %s\n' "reuse" "$LEANSPEC_DIR"
fi

# Fetch only when the commit is absent, so a repeat run needs no network. The
# by-revision fetch is tried first because it transfers one commit's history
# instead of every ref; not all remotes serve it, so a full fetch is the
# fallback rather than the first move.
if ! git -C "$LEANSPEC_DIR" cat-file -e "${rev}^{commit}" 2>/dev/null; then
    git -C "$LEANSPEC_DIR" fetch --quiet origin "$rev" 2>/dev/null ||
        git -C "$LEANSPEC_DIR" fetch --quiet --tags origin '+refs/heads/*:refs/remotes/origin/*' 2>/dev/null ||
        true
fi

git -C "$LEANSPEC_DIR" cat-file -e "${rev}^{commit}" 2>/dev/null ||
    fail "revision $rev is not present in $LEANSPEC_URL — either the recorded revision is wrong or the upstream does not carry that history"

# --force discards a dirty tree from an interrupted earlier run. Nothing in the
# checkout is authored here, so there is no local work to lose, and a checkout
# that silently declined to move would leave the HEAD assertion below reporting
# a revision nobody selected. This is also what carries a tree left over from
# the previous pin onto the newly recorded revision without a manual clean.
git -C "$LEANSPEC_DIR" checkout --quiet --detach --force "$rev" ||
    fail "could not check out $rev in $LEANSPEC_DIR"

head="$(git -C "$LEANSPEC_DIR" rev-parse HEAD)"
[ "$head" = "$rev" ] ||
    fail "$LEANSPEC_DIR is at $head, not the recorded $rev"

printf '  ok    %-12s %s\n' "HEAD" "$head (detached)"
printf '\nleanSpec %s is checked out at %s.\n' "$rev" "$LEANSPEC_DIR"
printf 'Spec citations of the form `leanSpec@%s:<path>:<line>` resolve against it.\n' "${rev:0:7}"
