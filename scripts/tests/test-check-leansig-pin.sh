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
#   TC-7  rev too short to identify a commit  -> fail
#   TC-8  leansig entry absent entirely       -> fail   (false-pass: unasserted)
#   TC-9  commented-out entry is ignored      -> pass   (false-FAIL guard)
#   TC-10 unreadable source                   -> fail   (must not default to pass)
#   TC-11 entry spans multiple lines          -> pass   (false-FAIL: line-at-a-time parse)
#   TC-12 leansig outside [workspace.dependencies] -> fail (false-pass: unscoped grep)
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

run "TC-5  git present but no rev" fail \
    "$(cargo_with 'leansig = { git = "https://github.com/leanEthereum/leanSig" }')"

# false-pass: a guard that stops at the first `rev` never sees the branch that
# actually moves. cargo rejects this combination, but the guard must not be the
# thing that depends on cargo noticing.
run "TC-6  rev alongside branch" fail \
    "$(cargo_with 'leansig = { git = "https://github.com/leanEthereum/leanSig", rev = "f10dcbefac2502d356d93f686e8b4ecd8dc8840a", branch = "main" }')"

# 6 hex is not a commit identifier — it is a prefix that will collide.
run "TC-7  rev too short" fail \
    "$(cargo_with 'leansig = { git = "https://github.com/leanEthereum/leanSig", rev = "f10dcb" }')"

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
