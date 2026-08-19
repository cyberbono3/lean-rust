#!/usr/bin/env bash
# test-check-changelog.sh — verify the changelog guard actually blocks a missing entry.
# Usage: bash scripts/tests/test-check-changelog.sh
#
# The guard exists because an entry that is not written by the PR making the change
# is an entry nobody reconstructs later. So most cases here are FALSE-PASS
# regressions — the branch really did skip its entry, and the guard must exit
# non-zero. Each case builds a throwaway base repo, clones it (which creates
# refs/remotes/origin/main, the same ref shape CI produces), branches, and runs the
# real script.
#
#   TC-1  branch edits CHANGELOG.md                   -> pass
#   TC-2  branch edits only crates/**                 -> fail, message names the label
#   TC-3  TC-2 under NO_CHANGELOG_LABEL=true          -> pass   (the escape hatch works)
#   TC-4  BASE_REF unset                              -> fail   (must not degrade to a skip)
#   TC-5  base moved CHANGELOG.md after branch point  -> fail   (a base-tip diff PASSES this)
#   TC-6  CHANGELOG.md deleted on the branch          -> pass   (a deletion IS a diff; the
#                                                               guard asserts touch, not shape)
#   TC-7  origin/<base> unresolvable, changelog TOUCHED-> fail, message names the ref
#   TC-8  uncommitted CHANGELOG.md edit only          -> pass   (merge-base form sees the
#                                                               work tree; local ergonomics)
#   TC-9  CI merge-ref topology, head skipped entry   -> fail   (models refs/pull/N/merge)
#   TC-10 --help                                      -> pass, prints usage
#   TC-11 two changelog paths, only one touched       -> fail   (positional-arg form)
#   TC-12 CHANGELOG.md absent on BOTH sides           -> fail, and says the FILE is gone
#                                                               rather than telling the author
#                                                               to write an entry into it
#   TC-13 typo'd flag WITH the label set              -> fail   (argument validation sits
#                                                               above the label short-circuit)
#   TC-14 git diff itself fails (bad pathspec magic)  -> fail   (exit >1 is an error, not
#                                                               "there is a diff")
#   TC-15 BASE_REF already carries origin/            -> pass   (no origin/origin/main)
#   TC-16 non-default positional path, touched        -> pass   (pins the positional exit 0)
#
# TC-5 is the case that distinguishes merge-base semantics from a diff against the
# base tip: with a base-tip diff the branch differs in CHANGELOG.md purely because
# main moved, and the guard would wrongly pass.
#
# TC-7 touches the changelog on purpose, so the ONLY remaining failure cause is the
# unresolvable base ref. Without that, the case would pass even with the ref check
# deleted, and the fetch-depth message — the single most likely real CI
# misconfiguration — would be untested.
#
# TC-9 models what CI actually checks out: a merge commit of base and head, detached.
# There the base tip is a parent of HEAD, so merge-base returns the base tip and all
# diff forms coincide; the guard must still fail a head that skipped its entry.
#
# bash 3.2 compatible (macOS system bash): no associative arrays, no ${var^^}, and
# possibly-empty arrays are expanded with the ${arr[@]+"${arr[@]}"} idiom.

set -uo pipefail

GUARD="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)/scripts/check-changelog.sh"
[ -f "$GUARD" ] || { printf 'guard not found: %s\n' "$GUARD" >&2; exit 1; }

pass_count=0
fail_count=0
GUARD_ARGS=()

git_c() { git -C "$1" -c user.email=t@t -c user.name=t "${@:2}"; }

# make_repo <dir> — base repo on `main` carrying BOTH changelogs and a crate file,
# cloned into <dir>/work with a `feature` branch checked out. Both changelogs are
# committed in the BASE so that "untouched on the branch" is a real state: an
# untracked file would produce no diff for a reason unrelated to the assertion.
make_repo() {
    local dir="$1"
    git init -q "$dir/base"
    git -C "$dir/base" symbolic-ref HEAD refs/heads/main
    mkdir -p "$dir/base/crates/protocol/src" "$dir/base/scripts"
    printf '# Changelog\n\n## v0.1.0 (Unreleased)\n\n#### Fixes\n\n- Seeded.\n' > "$dir/base/CHANGELOG.md"
    printf '# Other changelog\n\n- Seeded.\n' > "$dir/base/OTHER-CHANGELOG.md"
    printf 'pub fn seed() {}\n' > "$dir/base/crates/protocol/src/lib.rs"
    git -C "$dir/base" add -A
    git_c "$dir/base" commit -qm seed

    git clone -q "$dir/base" "$dir/work"
    git -C "$dir/work" checkout -qb feature
    mkdir -p "$dir/work/scripts"
    cp "$GUARD" "$dir/work/scripts/check-changelog.sh"
}

commit_in() { git -C "$1" add -A; git_c "$1" commit -qm "$2"; }

# run <name> <expect: pass|fail> <setup-fn> <match|-> [env assignments...]
# The ambient environment is scrubbed of BASE_REF and NO_CHANGELOG_LABEL, so a
# developer's exported value cannot flip a case (TC-4 in particular).
run() {
    local name="$1" expect="$2" setup="$3" match="$4"; shift 4
    local dir; dir="$(mktemp -d)" || { printf '  FAIL  %-48s mktemp failed\n' "$name"; fail_count=$((fail_count + 1)); return; }
    GUARD_ARGS=()
    # A setup that half-worked is the failure mode that turns a case into a silent
    # duplicate of another one: TC-5 without its fetch is TC-2, and still green.
    # Both builders therefore report their status and a non-zero one fails the case.
    if ! make_repo "$dir" || ! "$setup" "$dir"; then
        printf '  FAIL  %-48s fixture setup failed\n' "$name"
        rm -rf "$dir"
        fail_count=$((fail_count + 1))
        return
    fi

    local out rc
    out="$(cd "$dir/work" && env -u BASE_REF -u NO_CHANGELOG_LABEL "$@" \
        bash scripts/check-changelog.sh ${GUARD_ARGS[@]+"${GUARD_ARGS[@]}"} 2>&1)"; rc=$?
    rm -rf "$dir"

    local got; [ "$rc" -eq 0 ] && got=pass || got=fail
    if [ "$got" != "$expect" ]; then
        printf '  FAIL  %-48s expected %s, got %s (exit %d)\n' "$name" "$expect" "$got" "$rc"
        printf '%s\n' "$out" | sed 's/^/          | /'
        fail_count=$((fail_count + 1))
        return
    fi
    if [ "$match" != "-" ] && ! printf '%s' "$out" | grep -q "$match"; then
        printf '  FAIL  %-48s %s but message lacks %s\n' "$name" "$got" "$match"
        printf '%s\n' "$out" | sed 's/^/          | /'
        fail_count=$((fail_count + 1))
        return
    fi
    printf '  ok    %-48s (%s, exit %d)\n' "$name" "$got" "$rc"
    pass_count=$((pass_count + 1))
}

# --- setups ---

touch_changelog() {
    printf -- '- Added a thing ([#1](https://github.com/cyberbono3/lean-rust/pull/1)).\n' \
        >> "$1/work/CHANGELOG.md"
    commit_in "$1/work" entry
}

touch_crate_only() {
    printf 'pub fn added() {}\n' >> "$1/work/crates/protocol/src/lib.rs"
    commit_in "$1/work" code
}

delete_changelog() { rm "$1/work/CHANGELOG.md"; commit_in "$1/work" drop-changelog; }

# main gains a changelog commit AFTER the branch point; the branch touches only code.
# The precondition is asserted, not assumed: if the fetch silently no-ops, origin/main
# never advances, the fork point IS the base tip, and TC-5 degenerates into a copy of
# TC-2 that keeps reporting ok while the merge-base behaviour it guards is gone.
advance_base_changelog() {
    printf -- '- Unrelated ([#2](https://github.com/cyberbono3/lean-rust/pull/2)).\n' \
        >> "$1/base/CHANGELOG.md"
    git_c "$1/base" commit -qam "unrelated entry" || return 1
    git -C "$1/work" fetch -q origin || return 1
    [ "$(git -C "$1/work" rev-parse origin/main)" \
      != "$(git -C "$1/work" merge-base origin/main HEAD)" ] || return 1
}

base_moves_changelog() { advance_base_changelog "$1" || return 1; touch_crate_only "$1"; }

# The changelog IS touched here, so an unresolvable base ref is the only failure cause.
drop_origin_ref() { touch_changelog "$1"; git -C "$1/work" remote remove origin; }

uncommitted_entry() {
    printf -- '- Added a thing ([#1](https://github.com/cyberbono3/lean-rust/pull/1)).\n' \
        >> "$1/work/CHANGELOG.md"   # deliberately NOT committed
}

# What actions/checkout gives a pull_request job: a merge of base and head, detached.
ci_merge_ref() {
    touch_crate_only "$1" || return 1
    advance_base_changelog "$1" || return 1
    git -C "$1/work" checkout -q --detach || return 1
    git_c "$1/work" merge -q --no-ff -m "merge base into head" origin/main || return 1
    # Assert the shape actually built. Without the merge this case is byte-for-byte
    # TC-2, so a broken merge must fail loudly rather than pass as something else.
    [ "$(git -C "$1/work" rev-parse HEAD^2)" = "$(git -C "$1/work" rev-parse origin/main)" ] || return 1
}

help_only() { GUARD_ARGS=(--help); }

# git itself fails (exit >1), as opposed to reporting a difference (exit 1). Invalid
# pathspec magic is the cheapest way to force it. The guard must fail closed here,
# not read the non-zero status as "there is a diff".
git_diff_errors() { touch_crate_only "$1"; GUARD_ARGS=(':(nosuchmagic)CHANGELOG.md'); }

# BASE_REF already carrying the remote prefix must not become origin/origin/main.
prefixed_base_ref() { touch_changelog "$1"; }

# A positional path other than the default, touched. This pins the `exit 0` that ends
# the positional branch: without it the guard falls through and also demands the
# untouched default CHANGELOG.md, turning a correct invocation into a failure.
other_path_only() {
    printf -- '- Other ([#3](https://github.com/cyberbono3/lean-rust/pull/3)).\n' \
        >> "$1/work/OTHER-CHANGELOG.md"
    commit_in "$1/work" other-entry
    GUARD_ARGS=(OTHER-CHANGELOG.md)
}

two_paths() { touch_changelog "$1"; GUARD_ARGS=(CHANGELOG.md OTHER-CHANGELOG.md); }

# The required path exists nowhere: main drops it, the branch takes that deletion and
# then changes only code. An absent file produces an empty diff, which must fail —
# the same path a typo'd positional argument takes.
missing_changelog() {
    git -C "$1/base" rm -q CHANGELOG.md
    git_c "$1/base" commit -qm "drop changelog"
    git -C "$1/work" fetch -q origin
    git_c "$1/work" merge -q origin/main
    touch_crate_only "$1"
}

# The label must not swallow a malformed argument list. Runs WITH the label set, so
# it fails only while argument validation sits above the short-circuit.
typo_flag() { touch_crate_only "$1"; GUARD_ARGS=(--dry-run); }

printf 'changelog guard — missing-entry detection\n'

run "TC-1  branch edits CHANGELOG.md"           pass touch_changelog     -                BASE_REF=main
run "TC-2  branch edits only crates/**"         fail touch_crate_only    'no changelog'   BASE_REF=main
run "TC-3  TC-2 under the label"                pass touch_crate_only    -                BASE_REF=main NO_CHANGELOG_LABEL=true
run "TC-4  BASE_REF unset"                      fail touch_crate_only    'BASE_REF'
run "TC-5  base moved CHANGELOG.md after branch" fail base_moves_changelog -              BASE_REF=main
run "TC-6  CHANGELOG.md deleted on the branch"  pass delete_changelog    -                BASE_REF=main
run "TC-7  origin/<base> unresolvable"          fail drop_origin_ref     'cannot resolve' BASE_REF=main
run "TC-8  uncommitted CHANGELOG.md edit"       pass uncommitted_entry   -                BASE_REF=main
run "TC-9  CI merge-ref, head skipped entry"    fail ci_merge_ref        'no changelog'   BASE_REF=main
run "TC-10 --help prints usage (clean env)"     pass help_only           'Usage:'
run "TC-11 two paths, only one touched"         fail two_paths           'OTHER-CHANGELOG.md' BASE_REF=main
run "TC-12 CHANGELOG.md absent on both sides"   fail missing_changelog   'does not exist' BASE_REF=main
run "TC-13 typo'd flag with the label set"      fail typo_flag           'unknown argument' BASE_REF=main NO_CHANGELOG_LABEL=true
run "TC-14 git diff itself fails"               fail git_diff_errors     'git diff failed' BASE_REF=main
run "TC-15 BASE_REF already prefixed"           pass prefixed_base_ref   -                BASE_REF=origin/main
run "TC-16 non-default path, touched"           pass other_path_only     'OTHER-CHANGELOG.md' BASE_REF=main

printf '\n%d passed, %d failed\n' "$pass_count" "$fail_count"
[ "$fail_count" -eq 0 ]
