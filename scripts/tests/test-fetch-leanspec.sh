#!/usr/bin/env bash
# test-fetch-leanspec.sh — verify the leanSpec checkout lands on the recorded revision.
# Usage: bash scripts/tests/test-fetch-leanspec.sh
#
# The checkout exists so that `leanSpec@<rev>:<path>:<line>` citations resolve.
# A checkout at the wrong revision is therefore worse than none: citations still
# resolve, they just resolve against text nobody audited. So most cases here are
# FALSE-PASS regressions — the checkout is wrong and the script must exit
# non-zero, or the recorded revision is misread and the resulting HEAD must be
# asserted, not just the exit code.
#
#   TC-1  clean clone at the recorded revision   -> pass  (+ HEAD assertion)
#   TC-2  repeat run needs no upstream           -> pass  (idempotent, offline)
#   TC-3  recorded revision absent upstream      -> fail
#   TC-4  recorded revision is 39 chars          -> fail  (length boundary)
#   TC-5  recorded revision is 41 chars          -> fail  (false-pass: substring match)
#   TC-6  recorded revision is uppercase hex     -> fail
#   TC-7  no leanSpec revision row               -> fail  (false-pass: unasserted)
#   TC-8  two leanSpec revision rows             -> fail  (reshaped table)
#   TC-9  leanSig row carries a different hash   -> pass  (+ HEAD assertion)
#   TC-10 checkout path exists, not a repository -> fail
#   TC-11 stale checkout parked on another commit-> pass  (+ HEAD assertion)
#   TC-12 README.md missing                      -> fail  (must not default to pass)
#   TC-13 value cell is not backtick-quoted      -> fail
#   TC-14 upstream URL is read from the row      -> fail  (+ output assertion)
#
# The suite is offline: the upstream is a throwaway local repository, and TC-14
# blocks every transport except `file` so the https attempt fails on protocol
# rather than on DNS.
#
# Mutation-checked, with the coverage stated exactly rather than claimed
# wholesale:
#
#   - TC-9 turns red if the row match stops being scoped to the leanSpec label,
#     and only because it asserts HEAD: the run still exits 0, pinned to the
#     signature scheme's commit.
#   - TC-11 turns red if the detach stops being forced.
#   - TC-4 and TC-6 are pinned jointly by the hex validation and the HEAD
#     assertion; removing either alone leaves them green, removing both turns
#     them red. Neither is redundant: git resolves a 39-character prefix and an
#     uppercase hash quite happily, so the shape check is the only thing that
#     rejects a malformed recorded value, and the HEAD assertion is the only
#     thing that catches a checkout that landed somewhere else.
#   - TC-8, TC-10, and TC-12 exit non-zero even with their own guard removed:
#     a two-line revision, a clone into a non-empty directory, and an absent
#     table all fail closed further down. Those guards are kept for their error
#     messages, which name the actual problem instead of reporting it as a git
#     failure three steps away.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
SCRIPT="${REPO_ROOT}/scripts/fetch-leanspec.sh"

pass_count=0
fail_count=0

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# A throwaway upstream with two commits: the recorded revision is the parent, so
# a checkout that merely clones and stops sits on the wrong one.
UPSTREAM="$WORK/upstream"
git init --quiet "$UPSTREAM"
git -C "$UPSTREAM" config user.email test@example.com
git -C "$UPSTREAM" config user.name "test"
printf 'spec v1\n' > "$UPSTREAM/spec.md"
git -C "$UPSTREAM" add spec.md
git -C "$UPSTREAM" commit --quiet -m "spec v1"
REV="$(git -C "$UPSTREAM" rev-parse HEAD)"
printf 'spec v2\n' > "$UPSTREAM/spec.md"
git -C "$UPSTREAM" commit --quiet -am "spec v2"
REV_TIP="$(git -C "$UPSTREAM" rev-parse HEAD)"

ABSENT_REV="0123456789abcdef0123456789abcdef01234567"
OTHER_REV="f10dcbefac2502d356d93f686e8b4ecd8dc8840a"

# readme_with <rows> — the interop table with its revision rows swapped.
readme_with() {
    printf '%s\n' '# lean-rust

## Interop parameters (pq-devnet-1)

| Parameter | Value | Source |
| --------- | ----- | ------ |'
    printf '%s\n' "$1"
    printf '%s\n' '| Validator registry limit | `2^12` = `4096` | `config::DEVNET_CONFIG.validator_registry_limit` |'
}

# The shipped row shape, with the upstream link the parser reads.
row_spec() {
    printf '| leanSpec revision | `%s` | [leanEthereum/leanSpec](https://github.com/leanEthereum/leanSpec) |\n' "$1"
}

# seed_checkout <dir> <rev> — a pre-existing checkout parked on <rev>.
seed_checkout() {
    git clone --quiet "$UPSTREAM" "$1/.audit/leanSpec"
    git -C "$1/.audit/leanSpec" checkout --quiet --detach --force "$2"
}

# Per-case knobs, reset by run(). SETUP is a function name called with the case
# directory; URL_OVERRIDE is exported as LEANSPEC_URL unless empty.
SETUP=""
URL_OVERRIDE=""
EXPECT_HEAD=""
EXPECT_OUT=""
EXTRA_ENV=""

# run <name> <expect: pass|fail> <readme-contents>
run() {
    local name="$1" expect="$2" readme="$3"
    local dir="$WORK/case-$((pass_count + fail_count))"
    mkdir -p "$dir/scripts"
    cp "$SCRIPT" "$dir/scripts/fetch-leanspec.sh"
    [ "$readme" = "__NO_README__" ] || printf '%s\n' "$readme" > "$dir/README.md"
    [ -z "$SETUP" ] || "$SETUP" "$dir"

    local out rc
    out="$(cd "$dir" && env ${EXTRA_ENV} \
        ${URL_OVERRIDE:+LEANSPEC_URL="$URL_OVERRIDE"} \
        bash scripts/fetch-leanspec.sh 2>&1)"; rc=$?

    local got; [ "$rc" -eq 0 ] && got=pass || got=fail
    local problem=""
    [ "$got" = "$expect" ] || problem="expected $expect, got $got (exit $rc)"

    if [ -z "$problem" ] && [ -n "$EXPECT_HEAD" ]; then
        local head; head="$(git -C "$dir/.audit/leanSpec" rev-parse HEAD 2>/dev/null)"
        [ "$head" = "$EXPECT_HEAD" ] || problem="HEAD is ${head:-<none>}, expected $EXPECT_HEAD"
    fi
    if [ -z "$problem" ] && [ -n "$EXPECT_OUT" ]; then
        printf '%s' "$out" | grep -q -- "$EXPECT_OUT" || problem="output does not mention '$EXPECT_OUT'"
    fi

    if [ -z "$problem" ]; then
        printf '  ok    %-52s (%s, exit %d)\n' "$name" "$got" "$rc"
        pass_count=$((pass_count + 1))
    else
        printf '  FAIL  %-52s %s\n' "$name" "$problem"
        printf '%s\n' "$out" | sed 's/^/          | /'
        fail_count=$((fail_count + 1))
    fi

    SETUP=""; URL_OVERRIDE=""; EXPECT_HEAD=""; EXPECT_OUT=""; EXTRA_ENV=""
}

printf 'leanSpec checkout — recorded-revision assertion\n'

URL_OVERRIDE="$UPSTREAM"; EXPECT_HEAD="$REV"
run "TC-1  clean clone at the recorded revision" pass "$(readme_with "$(row_spec "$REV")")"

# The commit is already in the object store, so no fetch may be attempted: the
# URL points nowhere and the run must still succeed.
setup_seeded() { seed_checkout "$1" "$REV"; }
SETUP=setup_seeded; URL_OVERRIDE="$WORK/does-not-exist"; EXPECT_HEAD="$REV"
run "TC-2  repeat run needs no upstream" pass "$(readme_with "$(row_spec "$REV")")"

URL_OVERRIDE="$UPSTREAM"
run "TC-3  recorded revision absent upstream" fail "$(readme_with "$(row_spec "$ABSENT_REV")")"

# --- the recorded value must be an exact full hash ---

URL_OVERRIDE="$UPSTREAM"
run "TC-4  recorded revision is 39 chars" fail "$(readme_with "$(row_spec "${REV:0:39}")")"

# Fails only if the value cell is validated whole. A 40-of-41 substring match
# would read this as a valid pin and check out a commit nobody recorded.
URL_OVERRIDE="$UPSTREAM"
run "TC-5  recorded revision is 41 chars" fail "$(readme_with "$(row_spec "${REV}a")")"

URL_OVERRIDE="$UPSTREAM"
run "TC-6  recorded revision is uppercase hex" fail \
    "$(readme_with "$(row_spec "$(printf '%s' "$REV" | tr 'a-f' 'A-F')")")"

# --- the row itself must be found, exactly once ---

URL_OVERRIDE="$UPSTREAM"
run "TC-7  no leanSpec revision row" fail \
    "$(readme_with "| leanSig revision | \`$OTHER_REV\` | [leanEthereum/leanSig](https://github.com/leanEthereum/leanSig) |")"

URL_OVERRIDE="$UPSTREAM"
run "TC-8  two leanSpec revision rows" fail \
    "$(readme_with "$(row_spec "$REV")
$(row_spec "$REV_TIP")")"

# The false-pass this pins: an unscoped 40-hex match takes the first hash in the
# table, which belongs to the signature scheme, and the checkout lands nowhere
# near the audited spec. Exit 0 alone does not catch it — HEAD does.
URL_OVERRIDE="$UPSTREAM"; EXPECT_HEAD="$REV"
run "TC-9  leanSig row carries a different hash" pass \
    "$(readme_with "| leanSig revision | \`$OTHER_REV\` | [leanEthereum/leanSig](https://github.com/leanEthereum/leanSig) |
$(row_spec "$REV")")"

# --- pre-existing state at the checkout path ---

setup_not_a_repo() { mkdir -p "$1/.audit/leanSpec"; printf 'stray\n' > "$1/.audit/leanSpec/notes.txt"; }
SETUP=setup_not_a_repo; URL_OVERRIDE="$UPSTREAM"
run "TC-10 checkout path exists, not a repository" fail "$(readme_with "$(row_spec "$REV")")"

# A stale checkout from an earlier run sits on the branch tip with an edit left
# behind by an interrupted run. A plain detach refuses to overwrite that edit, so
# without --force the script leaves the wrong revision on disk.
setup_stale() { seed_checkout "$1" "$REV_TIP"; printf 'interrupted\n' > "$1/.audit/leanSpec/spec.md"; }
SETUP=setup_stale; URL_OVERRIDE="$UPSTREAM"; EXPECT_HEAD="$REV"
run "TC-11 stale checkout parked on another commit" pass "$(readme_with "$(row_spec "$REV")")"

URL_OVERRIDE="$UPSTREAM"
run "TC-12 README.md missing" fail "__NO_README__"

URL_OVERRIDE="$UPSTREAM"
run "TC-13 value cell is not backtick-quoted" fail \
    "$(readme_with "| leanSpec revision | $REV | [leanEthereum/leanSpec](https://github.com/leanEthereum/leanSpec) |")"

# No override, so the URL must come from the row. GIT_ALLOW_PROTOCOL=file makes
# the https attempt fail on protocol instead of on the network, which keeps the
# suite offline while still proving which URL was used.
EXTRA_ENV="GIT_ALLOW_PROTOCOL=file GIT_TERMINAL_PROMPT=0"
EXPECT_OUT="https://spec.example.invalid/leanSpec"
run "TC-14 upstream URL is read from the row" fail \
    "$(readme_with "| leanSpec revision | \`$REV\` | [mirror](https://spec.example.invalid/leanSpec) |")"

printf '\n%d passed, %d failed\n' "$pass_count" "$fail_count"
[ "$fail_count" -eq 0 ]
