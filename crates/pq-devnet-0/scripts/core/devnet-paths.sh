# This file is sourced by pq-devnet helper scripts after DEVNET_ROOT is set.
: "${DEVNET_ROOT:?DEVNET_ROOT must be set before sourcing devnet-paths.sh}"

PQ_DEVNET_GENERATED_PATHS=(
  "$DEVNET_ROOT/config.yaml"
  "$DEVNET_ROOT/genesis/config.yaml"
  "$DEVNET_ROOT/genesis/genesis.json"
  "$DEVNET_ROOT/genesis/genesis.ssz"
  "$DEVNET_ROOT/genesis/nodes.yaml"
  "$DEVNET_ROOT/genesis/validators.yaml"
  "$DEVNET_ROOT/genesis/validator-config.yaml"
  "$DEVNET_ROOT/genesis/bootnodes.rust.yaml"
  "$DEVNET_ROOT"/config/keys/*.key
  "$DEVNET_ROOT"/logs/*.log
)

PQ_DEVNET_SCAFFOLD_PATHS=(
  "$DEVNET_ROOT/config/keys/.gitkeep"
  "$DEVNET_ROOT/genesis/.gitkeep"
  "$DEVNET_ROOT/logs/.gitkeep"
  "$DEVNET_ROOT/scripts/core/.gitkeep"
)
