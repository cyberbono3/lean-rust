#!/usr/bin/env bash
#
# check-leansig-pin.sh — assert the leanSig dependency is pinned to an exact commit.
#
# leanSig is the post-quantum signature scheme. Its revision is an interop
# parameter, not a version preference: every client on the network must build
# against the same one, because a different revision produces signatures the
# others will not verify — and existing keys stop working. A `branch` or `tag`
# moves under us and breaks that silently, which is the failure this guard
# exists to prevent.
#
# `Cargo.toml` `[workspace.dependencies].leansig` is the single source of the
# pin; member crates inherit it via `leansig.workspace = true` and never restate
# the revision.
#
# Scope is deliberately the pin's SHAPE — that a `git` source and an exact `rev`
# are present and no moving ref is. It does not read `Cargo.lock`: until a member
# crate consumes leanSig, cargo never resolves it and writes no lock entry, so a
# lock assertion here would fail on every run. That assertion belongs with the
# first consuming crate.
#
# A guard that silently stops asserting is worse than no guard, so an unreadable
# manifest, an unparsable entry, or a missing declaration is a FAILURE, never a
# skip.
#
# Usage:   scripts/check-leansig-pin.sh
# Exit:    0 = pinned to an exact rev, 1 = floating ref or the pin could not be verified
# Tests:   scripts/tests/test-check-leansig-pin.sh

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

CARGO_TOML="Cargo.toml"
DEP="leansig"

# A commit is identified by its full 40-char hash. Cargo would accept a prefix,
# but a prefix can collide as the upstream repository grows, and until a member
# crate consumes leanSig there is no Cargo.lock entry recording the resolved
# commit — so this manifest line is the only record of the revision, and it is
# required to be exact.
REV_HEX_LEN=40

fail() { printf 'check-leansig-pin: %s\n' "$1" >&2; exit 1; }

[ -r "$CARGO_TOML" ] || fail "missing or unreadable required file: $CARGO_TOML"

# Scoped to [workspace.dependencies] so a leansig key in any other table (a
# member's [dependencies], a [patch] section) cannot be read as the workspace
# pin. Brace-balanced so a multi-line entry reads as one value — the repo already
# formats libp2p that way, and a reformat must not blind the guard.
#
# Comments are stripped before anything is matched, and that is load-bearing in
# both directions: a commented-out entry must not be read as live, and a live
# entry's trailing comment must not be read as a key. The likeliest way this pin
# ever floats is someone unpinning to test against upstream and leaving the old
# rev in a trailing comment — a guard that read that comment would call the
# result pinned.
entry="$(
    awk -v dep="$DEP" '
        # Truncate at the first # that is not inside a double-quoted string, so a
        # fragment in a URL survives while a trailing comment does not. TOML
        # escapes (\") are not handled: they cannot occur in the keys parsed here,
        # and mis-stripping one would fail closed rather than pass a floating pin.
        function strip_comment(line,   i, c, inq, out) {
            inq = 0; out = ""
            for (i = 1; i <= length(line); i++) {
                c = substr(line, i, 1)
                if (c == "#" && !inq) break
                if (c == "\"") inq = !inq
                out = out c
            }
            return out
        }
        { $0 = strip_comment($0) }
        /^[[:space:]]*\[/ {
            if (capturing) exit
            in_wd = ($0 ~ /^[[:space:]]*\[workspace\.dependencies\][[:space:]]*$/)
        }
        capturing {
            entry = entry " " $0
            depth += gsub(/\{/, "{") - gsub(/\}/, "}")
            if (depth <= 0) { print entry; exit }
            next
        }
        in_wd && $0 ~ ("^[[:space:]]*" dep "[[:space:]]*=") {
            entry = $0
            depth = gsub(/\{/, "{") - gsub(/\}/, "}")
            if (depth <= 0) { print entry; exit }
            capturing = 1
        }
    ' "$CARGO_TOML"
)"

[ -n "$entry" ] || fail "no live '$DEP = { ... }' inline table found under [workspace.dependencies] in $CARGO_TOML — the entry must be declared in that form (a [workspace.dependencies.$DEP] sub-table is not read); if it was removed, restore it: the pin is the reason this guard exists"

# Match keys only at a value boundary, so a URL or a longer key name that merely
# contains "git"/"rev"/"tag" cannot be read as the key itself.
has_key() { printf '%s' "$entry" | grep -Eq "(^|[[:space:],{])$1[[:space:]]*="; }

printf 'leanSig pin — source of truth: %s [workspace.dependencies].%s\n' "$CARGO_TOML" "$DEP"

has_key git || fail "$DEP is not a git dependency — the devnet pin is a commit in the leanSig repository, not a registry version"

for moving in branch tag; do
    if has_key "$moving"; then
        fail "$DEP declares '$moving' — a moving ref silently rebuilds against a different scheme revision and breaks existing keys; pin an exact 'rev' instead"
    fi
done

has_key rev || fail "$DEP has a git source but no 'rev' — without one cargo tracks the default branch, which moves"

rev="$(printf '%s' "$entry" | sed -n 's/.*[[:space:],{]rev[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p')"
[ -n "$rev" ] || fail "could not read the 'rev' value of $DEP from $CARGO_TOML"

printf '%s' "$rev" | grep -Eq "^[0-9a-fA-F]{${REV_HEX_LEN}}$" ||
    fail "$DEP rev '$rev' is not a full ${REV_HEX_LEN}-character commit hash — a prefix can collide, and this line is the only record of the revision"

printf '  ok    %-34s %s\n' "git source" "pinned by rev"
printf '  ok    %-34s %s\n' "rev" "$rev"

printf '\n%s is pinned to an exact commit.\n' "$DEP"
