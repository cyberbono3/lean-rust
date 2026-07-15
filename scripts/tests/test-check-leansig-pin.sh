#!/usr/bin/env bash
# test-check-leansig-pin.sh — verify the leanSig pin guard actually rejects a floating ref.
# Usage: bash scripts/tests/test-check-leansig-pin.sh
#
# The pin exists because a moving ref silently breaks existing keys: the same
# code against a different scheme revision produces signatures no other client
# verifies. So most cases here are FALSE-PASS regressions — the pin really is
# floating, and the guard must exit non-zero. Each builds a throwaway repo
# skeleton and runs the real script.
#
#   TC-1  git + exact rev, no floating key    -> pass
#   TC-2  branch instead of rev               -> fail   (the core assertion)
#   TC-3  tag instead of rev                  -> fail
#   TC-4  bare version, no git                -> fail
#   TC-5  git present but no rev              -> fail
#   TC-6  rev alongside branch                -> fail   (false-pass: rev found, branch unread)
#   TC-7  rev is a 7-char prefix              -> fail   (the pin must be the full hash)
#   TC-8  leansig entry absent entirely       -> fail   (false-pass: unasserted)
#   TC-9  commented-out entry is ignored      -> pass   (false-FAIL guard)
#   TC-10 unreadable source                   -> fail   (must not default to pass)
#   TC-11 entry spans multiple lines          -> pass   (false-FAIL: line-at-a-time parse)
#   TC-12 leansig outside [workspace.dependencies] -> fail (false-pass: unscoped grep)
#   TC-13 rev only in a trailing comment      -> fail   (false-pass: comment read as key)
#   TC-14 stale branch in a trailing comment  -> pass   (false-FAIL: comment read as key)
#   TC-15 version + rev but no git            -> fail   (false-pass: git check unasserted)
#   TC-16 rev in a URL query string only      -> fail   (false-pass: unbounded key match)
#   TC-17 key names appear inside the git URL -> pass   (false-FAIL: unbounded key match)
#   TC-18 rev is 39 chars                     -> fail   (off-by-one at the length boundary)
#
# Every case above is mutation-checked: removing the assertion it names from the
# guard turns it red. The one exception is deliberate — deleting `has_key rev`
# leaves the suite green, because the rev extraction below it fails closed on the
# same input. That check is kept for its error message, not its enforcement.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
GUARD="${REPO_ROOT}/scripts/check-leansig-pin.sh"

pass_count=0
fail_count=0

# Build a minimal repo skeleton; the arg overrides the manifest content.
make_repo() {
    local dir="$1" cargo="$2"
    mkdir -p "$dir/scripts"
    printf '%s\n' "$cargo" > "$dir/Cargo.toml"
    cp "$GUARD" "$dir/scripts/check-leansig-pin.sh"
}

# The shipped shape: git + full 40-char rev, in its own group.
CARGO_OK='[workspace]
resolver = "2"

[workspace.dependencies]
sha2      = "0.10"

# pq crypto
leansig = { git = "https://github.com/leanEthereum/leanSig", rev = "f10dcbefac2502d356d93f686e8b4ecd8dc8840a" }'

# cargo_with <leansig-entry> — same skeleton, one entry swapped.
cargo_with() {
    printf '%s\n' '[workspace]
resolver = "2"

[workspace.dependencies]
sha2      = "0.10"

# pq crypto'
    printf '%s\n' "$1"
}

# run <name> <expect: pass|fail> <cargo>
run() {
    local name="$1" expect="$2" cargo="$3"
    local dir; dir="$(mktemp -d)"
    make_repo "$dir" "$cargo"
    local out rc
    out="$(cd "$dir" && bash scripts/check-leansig-pin.sh 2>&1)"; rc=$?
    rm -rf "$dir"

    local got; [ "$rc" -eq 0 ] && got=pass || got=fail
    if [ "$got" = "$expect" ]; then
        printf '  ok    %-52s (%s, exit %d)\n' "$name" "$got" "$rc"
        pass_count=$((pass_count + 1))
    else
        printf '  FAIL  %-52s expected %s, got %s (exit %d)\n' "$name" "$expect" "$got" "$rc"
        printf '%s\n' "$out" | sed 's/^/          | /'
        fail_count=$((fail_count + 1))
    fi
}

printf 'leanSig pin guard — floating-ref rejection\n'

run "TC-1  git + exact rev" pass "$CARGO_OK"

# --- floating refs: the whole point of the guard ---

run "TC-2  branch instead of rev" fail \
    "$(cargo_with 'leansig = { git = "https://github.com/leanEthereum/leanSig", branch = "main" }')"

run "TC-3  tag instead of rev" fail \
    "$(cargo_with 'leansig = { git = "https://github.com/leanEthereum/leanSig", tag = "v0.1.0" }')"

run "TC-4  bare version, no git" fail \
    "$(cargo_with 'leansig = "0.1"')"

# TC-4 alone does not pin the git-source assertion: it fails on the missing rev,
# so deleting the git check leaves it green. This one carries a valid rev and so
# fails only if the git source is actually asserted.
run "TC-15 version + rev but no git" fail \
    "$(cargo_with 'leansig = { version = "0.1", rev = "f10dcbefac2502d356d93f686e8b4ecd8dc8840a" }')"

# A rev-looking fragment in the URL is not a pin. Fails only if keys are matched
# at a value boundary rather than by substring.
run "TC-16 rev in a URL query string only" fail \
    "$(cargo_with 'leansig = { git = "https://example.com/leanSig?rev=f10dcbefac2502d356d93f686e8b4ecd8dc8840a" }')"

# The inverse of TC-16, and the case that pins the boundary itself: a correctly
# pinned entry whose URL merely contains the key names must not be rejected.
# Substring matching would read the path as a branch key and fail this.
run "TC-17 key names appear inside the git URL" pass \
    "$(cargo_with 'leansig = { git = "https://github.com/leanEthereum/leanSig-branch-tag-rev", rev = "f10dcbefac2502d356d93f686e8b4ecd8dc8840a" }')"

run "TC-5  git present but no rev" fail \
    "$(cargo_with 'leansig = { git = "https://github.com/leanEthereum/leanSig" }')"

# false-pass: a guard that stops at the first `rev` never sees the branch that
# actually moves. cargo rejects this combination, but the guard must not be the
# thing that depends on cargo noticing.
run "TC-6  rev alongside branch" fail \
    "$(cargo_with 'leansig = { git = "https://github.com/leanEthereum/leanSig", rev = "f10dcbefac2502d356d93f686e8b4ecd8dc8840a", branch = "main" }')"

# Any prefix is rejected, not just an implausibly short one. `f10dcbe` is the
# abbreviated form of the very commit this repo pins, so it is the prefix most
# likely to be typed by hand — and with no lockfile entry to disambiguate it,
# the manifest line is the only record. It must be the full hash.
run "TC-7  rev is a 7-char prefix of the real commit" fail \
    "$(cargo_with 'leansig = { git = "https://github.com/leanEthereum/leanSig", rev = "f10dcbe" }')"

run "TC-18 rev is 39 chars — one short of a full hash" fail \
    "$(cargo_with 'leansig = { git = "https://github.com/leanEthereum/leanSig", rev = "f10dcbefac2502d356d93f686e8b4ecd8dc884" }')"

# --- false-pass: the assertion silently stops applying ---

run "TC-8  leansig entry absent entirely" fail \
    '[workspace]
resolver = "2"

[workspace.dependencies]
sha2      = "0.10"'

# A dep of a member crate is not the workspace pin: reading it would report a
# pin this workspace does not actually declare.
run "TC-12 leansig outside [workspace.dependencies]" fail \
    '[workspace]
resolver = "2"

[workspace.dependencies]
sha2      = "0.10"

[dependencies]
leansig = { git = "https://github.com/leanEthereum/leanSig", rev = "f10dcbefac2502d356d93f686e8b4ecd8dc8840a" }'

# --- false-FAIL guards: correct manifests must not be rejected ---

run "TC-9  commented-out entry is ignored" pass \
    "$(cargo_with '# leansig = { git = "https://github.com/leanEthereum/leanSig", branch = "main" }
leansig = { git = "https://github.com/leanEthereum/leanSig", rev = "f10dcbefac2502d356d93f686e8b4ecd8dc8840a" }')"

# The likeliest way this pin ever floats: unpin to test against upstream, leave
# the old rev in a trailing comment. There is no rev key here at all — cargo would
# track the default branch — so a guard that reads the comment calls a floating
# pin exact. This is the case that must not regress.
run "TC-13 rev only in a trailing comment" fail \
    "$(cargo_with 'leansig = { git = "https://github.com/leanEthereum/leanSig" } # rev = "f10dcbefac2502d356d93f686e8b4ecd8dc8840a"')"

# The inverse: the entry is correctly pinned and the comment is just history.
# Rejecting it would train people to delete the guard.
run "TC-14 stale branch in a trailing comment" pass \
    "$(cargo_with 'leansig = { git = "https://github.com/leanEthereum/leanSig", rev = "f10dcbefac2502d356d93f686e8b4ecd8dc8840a" } # was branch = "main"')"

# The repo already formats libp2p across several lines; a future reformat of this
# entry must not turn the guard red.
run "TC-11 entry spans multiple lines" pass \
    "$(cargo_with 'leansig = {
    git = "https://github.com/leanEthereum/leanSig",
    rev = "f10dcbefac2502d356d93f686e8b4ecd8dc8840a",
}')"

# --- unreadable source must fail, never default to pass ---

printf '  --- unreadable source ---\n'
tc10_dir="$(mktemp -d)"
make_repo "$tc10_dir" "$CARGO_OK"
rm "$tc10_dir/Cargo.toml"
tc10_out="$(cd "$tc10_dir" && bash scripts/check-leansig-pin.sh 2>&1)"; tc10_rc=$?
rm -rf "$tc10_dir"
if [ "$tc10_rc" -ne 0 ]; then
    printf '  ok    %-52s (fail, exit %d)\n' "TC-10 missing source file" "$tc10_rc"
    pass_count=$((pass_count + 1))
else
    printf '  FAIL  %-52s expected fail, got pass\n' "TC-10 missing source file"
    printf '%s\n' "$tc10_out" | sed 's/^/          | /'
    fail_count=$((fail_count + 1))
fi

printf '\n%d passed, %d failed\n' "$pass_count" "$fail_count"
[ "$fail_count" -eq 0 ]
