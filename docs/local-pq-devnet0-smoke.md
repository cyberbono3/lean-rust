# Issue #13 - local-pq Devnet0 Smoke Validation

This report tracks Issue #13: run and record end-to-end smoke validation for
the Docker `ream <-> lean-rust` local-pq devnet, and apply logging/tracing
features that make pq-devnet-0 failures diagnosable.

## Current Branch State

| Field | Value |
| --- | --- |
| Branch | `test/run-record-e2e-devnet-smoke-validation` |
| Merge state | Includes `pq-devnet-0` and logging/tracing features. |
| Reference | Ream `ethpandaops/ream:master-0bceaee` |
| Latest result | Pass for two-node head convergence and cleanup; prior follow-ups addressed. |

## Implemented Fixes

- Added `lean-beacon devnet-config` and generated a separate
  `genesis/lean-rust-devnet0.yaml` for lean-rust.
- Added Ream local-pq genesis SSZ compatibility for the generated 145-byte
  `genesis.ssz` fixture.
- Matched Ream's nine-field local-pq state root shape for the slot-0 anchor.
- Hardened devnet timing by raising the default genesis offset to 180 seconds,
  preflighting images before genesis generation, and adding a stale-genesis
  guard before Compose startup.
- Updated request/response framing to use `uvarint(ssz_len) ||
  snappy_framed(ssz_bytes)`.
- Aligned lean-rust gossipsub topics with Ream:
  `/leanconsensus/devnet0/block/ssz_snappy` and
  `/leanconsensus/devnet0/vote/ssz_snappy`.
- Fixed forkchoice/local production so lean-rust advances head after producing
  or importing valid blocks.
- Wired `GossipIngestService` into devnet startup so p2p-delivered blocks and
  votes reach the chain service.
- Fixed status and head-sampling scripts to parse Ream's scalar head API
  response.
- Normalized Ream's slot-0 zero-source vote shape before forkchoice
  validation.
- Downgraded the optional Ream status RPC timeout from a warning to a debug
  diagnostic.
- Labeled finalized-root comparison as `not-compared` when Ream's scalar head
  API omits finalized fields.

## Generated Artifacts

`make devnet-genesis` writes Ream-compatible artifacts under
`crates/pq-devnet-0/genesis/`: `config.yaml` with `GENESIS_TIME`,
`genesis.ssz` as the 145-byte Ream leanchain genesis state, `genesis.json` for
inspection, `validators.yaml`, and `nodes.yaml`. lean-rust reads the same
genesis state through a native SSZ decode first, then a Ream leanchain
compatibility decode for this fixture shape.

The script also writes lean-rust-specific compatibility files:
`lean-rust-devnet0.yaml` for runtime config and `bootnodes.rust.yaml` with the
Ream node0 QUIC/libp2p multiaddr. Ream keeps using its generated node list;
lean-rust uses the derived bootnode file so the two clients share genesis
inputs while preserving each client's expected peer configuration format.

## Latest Run

| Field | Value |
| --- | --- |
| Updated UTC | `2026-05-19T12:08:26Z` |
| Genesis time | `1779192452` (`2026-05-19T12:07:32Z`) |
| lean-rust image | `lean-rust:local` manifest `sha256:961ac62efed8b41b645c46dea4b554b08b21259add753eef1dd71897c5b683dd` |
| Ream API | `127.0.0.1:5052` |
| lean-rust API | `127.0.0.1:5053` |
| Ream metrics | `127.0.0.1:8080`, 4701 bytes scraped |
| lean-rust metrics | `127.0.0.1:8081`, 267 bytes scraped |
| Pre-cleanup container state | Both containers `running`, `restarts=0`, `exit=0` |
| Cleanup state | `make devnet-clean` removed containers, volumes, and generated state; image remained present |

Pre-cleanup status:

```text
--- ream node0 ---
head.root: 0x6e140e132cd4c7e3f95382bff0480f264945303b3741d0605a0178bc7206d752

--- lean-rust node1 ---
head.root: 0x6e140e132cd4c7e3f95382bff0480f264945303b3741d0605a0178bc7206d752
head.slot: 5
finalized.root: 0x7297a9c85bb751189dd4e2d3c0a46b6c2547c79729f854f104c346ad0a05dcb9
finalized.slot: 0
```

Ten-sample diagnostic sampler:

| Sample UTC | Ream head.root | lean-rust head.root | lean-rust slot | Finalized match | Match |
| --- | --- | --- | --- | --- | --- |
| `2026-05-19T12:07:58Z` | `0xc8aa31e5b983e1ffc807a0094d1b8db2c90bbb00cc4276b70090e0340a2cef65` | `0xc8aa31e5b983e1ffc807a0094d1b8db2c90bbb00cc4276b70090e0340a2cef65` | `6` | `not-compared` | yes |
| `2026-05-19T12:08:01Z` | `0x58cab6841723cd2814695b50c266a839953f281bc7d2851dd6c118d18fad8e35` | `0x58cab6841723cd2814695b50c266a839953f281bc7d2851dd6c118d18fad8e35` | `7` | `not-compared` | yes |
| `2026-05-19T12:08:04Z` | `0xfeb2f4582a0aeaef583b162c33678d3d8e4f2edd662fb9e88664f592683a0286` | `0xfeb2f4582a0aeaef583b162c33678d3d8e4f2edd662fb9e88664f592683a0286` | `8` | `not-compared` | yes |
| `2026-05-19T12:08:08Z` | `0x4ba6aa13d22ed18708a331ab19c91c6a428f06bff957dfceea1ca7f02a625bbe` | `0x4ba6aa13d22ed18708a331ab19c91c6a428f06bff957dfceea1ca7f02a625bbe` | `9` | `not-compared` | yes |
| `2026-05-19T12:08:11Z` | `0x4ba6aa13d22ed18708a331ab19c91c6a428f06bff957dfceea1ca7f02a625bbe` | `0x4ba6aa13d22ed18708a331ab19c91c6a428f06bff957dfceea1ca7f02a625bbe` | `9` | `not-compared` | yes |
| `2026-05-19T12:08:14Z` | `0xe83290eca649fd253cfc81a8be5435490f28f8797caa679f6173fa391df212f1` | `0xe83290eca649fd253cfc81a8be5435490f28f8797caa679f6173fa391df212f1` | `10` | `not-compared` | yes |
| `2026-05-19T12:08:17Z` | `0xc7fb9692ead1b061c9f713342f65c6646734bdd5b6c3a7f83dda2b8371309a87` | `0xc7fb9692ead1b061c9f713342f65c6646734bdd5b6c3a7f83dda2b8371309a87` | `11` | `not-compared` | yes |
| `2026-05-19T12:08:20Z` | `0xd0b04f34e009f7f08a04494e630368a262383dbe98b73043962cd993634f9a0c` | `0xd0b04f34e009f7f08a04494e630368a262383dbe98b73043962cd993634f9a0c` | `12` | `not-compared` | yes |
| `2026-05-19T12:08:23Z` | `0xd0b04f34e009f7f08a04494e630368a262383dbe98b73043962cd993634f9a0c` | `0xd0b04f34e009f7f08a04494e630368a262383dbe98b73043962cd993634f9a0c` | `12` | `not-compared` | yes |
| `2026-05-19T12:08:26Z` | `0x9524cc7d6735f5b442c7e719dca8303e8e375ec1eb2d84806562a2e0161c60d9` | `0x9524cc7d6735f5b442c7e719dca8303e8e375ec1eb2d84806562a2e0161c60d9` | `13` | `not-compared` | yes |

The sampler printed `observed 10 consecutive matching head samples; finalized
roots were not compared because Ream did not report finalized fields`.

## Log Markers

lean-rust importing a Ream block:

```text
node::gossip_ingest: gossip block accepted slot=6 proposer=0 block_root=0xc8aa31e5b983e1ffc807a0094d1b8db2c90bbb00cc4276b70090e0340a2cef65
```

Ream importing a lean-rust block:

```text
ream_chain_lean::service: Processing block built by Validator 1 slot=7 block_root=0x58cab6841723cd2814695b50c266a839953f281bc7d2851dd6c118d18fad8e35
```

Follow-up log markers:

```text
runtime_p2p::service: status rpc outbound timeout; peer did not answer optional status request
lean_beacon::genesis: genesis state native SSZ decode failed; trying Ream leanchain compatibility decode
node::gossip_ingest: gossip vote accepted slot=0 validator=0 head_root=0x7297a9c85bb751189dd4e2d3c0a46b6c2547c79729f854f104c346ad0a05dcb9
ream_p2p::network::lean: Publish block failed slot=7 error=Duplicate
```

The status timeout is now debug-only. The prior Ream slot-0 zero-source vote
rejection is now accepted after genesis-source normalization. The expected
native SSZ fallback for Ream's 145-byte genesis file is debug-only. Ream
duplicate publish warnings are still observed while both clients import each
other's blocks.

## Evidence Checklist

| Check | Status | Evidence |
| --- | --- | --- |
| pq-devnet and logging/tracing feature branches are merged | Done | Branch includes both feature sets. |
| Logging/tracing code compiles and passes local static checks | Done | Focused tests, clippy, and format completed. |
| Ream and lean-rust use separate compatible config files | Done | `lean-rust-devnet0.yaml` generated by `lean-beacon devnet-config`. |
| Ream genesis SSZ loads in lean-rust | Done | 145-byte Ream local-pq genesis fixture decodes through the compatibility adapter. |
| Ream and lean-rust derive the same slot-0 anchor root | Done | Anchor/finalized root `0x7297a9c85bb751189dd4e2d3c0a46b6c2547c79729f854f104c346ad0a05dcb9`. |
| Both containers remain live after startup | Done | Both `running`, `restarts=0`, `exit=0`. |
| Both HTTP APIs are reachable | Done | `make devnet-status` reaches both nodes. |
| Metrics scrape successfully on host ports `8080` and `8081` | Done | Ream metrics scrape returned 4701 bytes; lean-rust metrics scrape returned 267 bytes. |
| lean-rust establishes libp2p connectivity to Ream | Done | Gossip blocks and votes are exchanged. |
| Block/vote gossip publishes to matching topics | Done | `devnet0` block/vote topics are used by lean-rust. |
| Ream and lean-rust import blocks from each other | Done | Log markers above show both directions. |
| `head.root` agrees across both nodes for consecutive samples | Done | Ten consecutive samples matched. |
| Finalized roots agree across both APIs | Not comparable | The sampler labels this `not-compared` because Ream scalar head API does not include finalized fields. |
| Cleanup removes generated state without removing images | Done | `make devnet-clean` removed containers, volumes, and generated files; `lean-rust:local` remained present; `make devnet-clean-check` passed. |

## Local Verification Completed

```sh
cargo fmt --all
bash -n crates/pq-devnet-0/scripts/core/check-cleanup.sh crates/pq-devnet-0/scripts/core/check-genesis-time.sh crates/pq-devnet-0/scripts/core/devnet-paths.sh crates/pq-devnet-0/scripts/core/setup-genesis.sh crates/pq-devnet-0/scripts/core/smoke-head-sample.sh crates/pq-devnet-0/scripts/core/status.sh
cargo test -p beacon -p forkchoice -p runtime-p2p -p pq-devnet-0 -- --nocapture
cargo clippy -p beacon -p forkchoice -p runtime-p2p -p pq-devnet-0 --all-targets -- -D warnings
make devnet-down
FORCE=1 make devnet-build
make devnet-genesis
make devnet-up
make devnet-status
PQ_DEVNET_SMOKE_MATCHES=10 PQ_DEVNET_SMOKE_MAX_ATTEMPTS=14 PQ_DEVNET_SMOKE_INTERVAL_SECONDS=3 make devnet-smoke-head-sample
curl --fail http://127.0.0.1:8080/metrics
curl --fail http://127.0.0.1:8081/metrics
make devnet-clean
make devnet-clean-check
cp crates/pq-devnet-0/.env.example crates/pq-devnet-0/.env
make devnet-start
make devnet-status
make devnet-stop
make devnet-clean
rm -f crates/pq-devnet-0/.env
make verify
git diff --check
```

## Remaining Observation

Ream duplicate publish warnings remain visible in the reference client while
both implementations keep importing blocks. Treat them as reference-client
noise unless they correlate with missed imports.
