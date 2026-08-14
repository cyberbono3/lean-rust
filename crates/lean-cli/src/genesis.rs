//! Genesis config/state loading for the beacon binary.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{ensure, Context, Result};
use config::Config as ChainConfig;
use protocol::{Block, BlockBody, State, Validator};
use runtime::duties::{GenesisRegistry, ValidatorAssignments};
use ssz::HashTreeRoot;
use tracing::{debug, info, warn};

const DEFAULT_GENESIS_DELAY_SLOTS: u64 = 15;

/// Companion pubkey-manifest filename, resolved as a sibling of the assignment
/// `validators.yaml` on the synthesized-genesis path.
const GENESIS_VALIDATORS_MANIFEST: &str = "genesis_validators.yaml";

/// Loads a devnet chain config from `path`, or returns the default config.
///
/// # Errors
///
/// Returns an error when the YAML file cannot be read or the config parser
/// rejects its contents.
pub fn load_chain_config(path: Option<&Path>) -> Result<ChainConfig> {
    let Some(path) = path else {
        let config = ChainConfig::default();
        info!(
            slot_duration_ms = config.slot_duration_ms,
            seconds_per_slot = config.seconds_per_slot,
            validator_registry_limit = config.validator_registry_limit,
            historical_roots_limit = config.historical_roots_limit,
            "using default genesis config",
        );
        return Ok(config);
    };
    let yaml = std::fs::read_to_string(path)
        .with_context(|| format!("read genesis config YAML {}", path.display()))?;
    debug!(
        path = %path.display(),
        bytes = yaml.len(),
        "read genesis config YAML",
    );
    let config = ChainConfig::from_yaml(&yaml)
        .inspect_err(|err| warn!(path = %path.display(), %err, "genesis config parse failed"))
        .with_context(|| format!("parse genesis config YAML {}", path.display()))?;
    info!(
        path = %path.display(),
        slot_duration_ms = config.slot_duration_ms,
        seconds_per_slot = config.seconds_per_slot,
        validator_registry_limit = config.validator_registry_limit,
        historical_roots_limit = config.historical_roots_limit,
        "loaded genesis config",
    );
    Ok(config)
}

/// Loads an SSZ-encoded genesis state from disk.
///
/// # Errors
///
/// Returns an error when the file cannot be read or the SSZ decoder rejects
/// the bytes.
// NOTE: this decodes only — it does NOT enforce chain-config limits or the
// non-empty-registry requirement. `load_or_synthesize_state` is the single
// validation boundary and applies `validate_state_limits` to BOTH the supplied
// and synthesized paths. Kept crate-private so that boundary cannot be
// bypassed; promote it only together with the validation call.
pub(crate) fn load_state(path: &Path) -> Result<State> {
    // Upper bound on the on-disk genesis state. The wire-format State for
    // devnet0's validator-registry-limit (4096) + historical-roots-limit
    // (262_144) bounds out well under this; the cap exists so an
    // operator-supplied (or symlinked) huge / non-SSZ file cannot OOM the
    // process during the initial read.
    const MAX_GENESIS_STATE_BYTES: u64 = 16 * 1024 * 1024;

    let meta = std::fs::metadata(path)
        .with_context(|| format!("stat genesis state SSZ {}", path.display()))?;
    anyhow::ensure!(
        meta.len() <= MAX_GENESIS_STATE_BYTES,
        "genesis state SSZ {} is {} bytes; refusing to read >{} bytes",
        path.display(),
        meta.len(),
        MAX_GENESIS_STATE_BYTES,
    );
    let bytes = std::fs::read(path)
        .with_context(|| format!("read genesis state SSZ {}", path.display()))?;
    debug!(
        path = %path.display(),
        bytes = bytes.len(),
        "read genesis state SSZ",
    );
    let state = match ssz::decode::<State>(&bytes) {
        Ok(state) => state,
        Err(native_err) => {
            debug!(
                path = %path.display(),
                bytes = bytes.len(),
                err = %native_err,
                "genesis state native SSZ decode failed; trying Ream leanchain compatibility decode",
            );
            State::from_ream_legacy_ssz_bytes(&bytes)
                .map_err(|legacy_err| {
                    warn!(
                        path = %path.display(),
                        bytes = bytes.len(),
                        err = ?legacy_err,
                        "genesis state Ream leanchain compatibility decode failed",
                    );
                    anyhow::anyhow!(
                        "decode genesis state SSZ {} as native or Ream leanchain state: native={native_err}; ream_legacy={legacy_err:?}",
                        path.display(),
                    )
                })?
        }
    };
    info!(
        path = %path.display(),
        validators = state.num_validators(),
        genesis_time = state.config.genesis_time,
        slot = state.slot.get(),
        "decoded genesis state SSZ",
    );
    Ok(state)
}

/// Loads a supplied genesis state, or synthesizes a devnet state from the
/// validator assignment file when no state path was provided.
///
/// # Errors
///
/// Returns an error when the supplied state cannot be loaded, the validator
/// assignment file cannot be loaded, or the resulting state would violate
/// chain-config limits.
pub fn load_or_synthesize_state(
    state_path: Option<&Path>,
    chain_config: &ChainConfig,
    validators_path: &Path,
) -> Result<State> {
    let state = if let Some(path) = state_path {
        load_state(path)?
    } else {
        debug!(
            path = %validators_path.display(),
            "loading validator assignments for synthesized genesis state",
        );
        let assignments = ValidatorAssignments::load(validators_path).with_context(|| {
            format!(
                "load validator assignments for synthesized genesis state from {}",
                validators_path.display()
            )
        })?;
        // The registry IS the validator count — `GenesisRegistry::load` has
        // already checked its length against the assignment file's total.
        let validators = load_genesis_registry(validators_path, &assignments)?;
        let genesis_time = default_genesis_time(chain_config)?;
        let state = synthesize_state(genesis_time, validators);
        info!(
            validator_registry_path = %validators_path.display(),
            validators = state.num_validators(),
            registry_len = state.validators.len(),
            genesis_time = state.config.genesis_time,
            "synthesized genesis state",
        );
        state
    };
    validate_state_limits(&state, chain_config)?;
    info!(
        validators = state.num_validators(),
        genesis_time = state.config.genesis_time,
        slot = state.slot.get(),
        "loaded genesis state",
    );
    Ok(state)
}

/// Derives the anchor block required by `node::devnet::Config`.
///
/// Only genesis-shaped states can be derived losslessly because the state does
/// not carry a full block body. The latest block header must therefore commit
/// to the empty body.
///
/// # Errors
///
/// Returns an error when the state is not genesis-shaped enough to reconstruct
/// its anchor block.
pub fn anchor_block_for_state(state: &State) -> Result<Block> {
    let header = state.latest_block_header;
    let body = BlockBody::default();
    let body_root = body.hash_tree_root().into();
    ensure!(
        header.body_root == body_root,
        "genesis state latest block header does not commit to an empty block body"
    );
    ensure!(
        state.slot == header.slot,
        "genesis state slot {} does not match latest block header slot {}",
        state.slot,
        header.slot,
    );

    let block = Block {
        slot: header.slot,
        proposer_index: header.proposer_index,
        parent_root: header.parent_root,
        state_root: state.hash_tree_root().into(),
        body,
    };
    info!(
        slot = block.slot.get(),
        proposer = block.proposer_index.get(),
        state_root = %hex32(block.state_root.0),
        block_root = %hex32(block.hash_tree_root()),
        "derived genesis anchor block",
    );
    Ok(block)
}

/// Loads the companion `genesis_validators.yaml` next to `validators_path` and
/// builds the ordered registry.
///
/// The manifest is REQUIRED: `State.validators` is the sole source of the
/// validator-set size, so an absent manifest would synthesize a chain with zero
/// validators — one that cannot propose, attest, or justify. A count or format
/// mismatch is likewise a hard error, never a silent partial registry.
fn load_genesis_registry(
    validators_path: &Path,
    assignments: &ValidatorAssignments,
) -> Result<Vec<Validator>> {
    // Sibling of the assignment file: same directory, manifest filename.
    let manifest_path = validators_path.with_file_name(GENESIS_VALIDATORS_MANIFEST);
    // `load_optional` owns the absent-vs-present decision under ITS path
    // resolution — do NOT pre-probe with `Path::exists`, which resolves against
    // a different root and can silently disagree with the actual read.
    let registry = GenesisRegistry::load(assignments, &manifest_path).with_context(|| {
        format!(
            "load genesis pubkey manifest {}; the validator registry is the sole source of \
             the validator count, so the manifest is required",
            manifest_path.display(),
        )
    })?;
    Ok(registry.into_validators())
}

/// Builds the genesis [`State`] from the ordered validator registry.
///
/// The registry length IS the validator count; there is no separate scalar to
/// keep in step with it.
fn synthesize_state(genesis_time: u64, validators: Vec<Validator>) -> State {
    protocol::stf::genesis_state(genesis_time, validators)
}

fn validate_state_limits(state: &State, chain_config: &ChainConfig) -> Result<()> {
    ensure!(
        !state.validators.is_empty(),
        "genesis anchor carries an empty validator registry: the registry is the sole source \
         of the validator count, so a node anchored here rejects every attestation and can \
         never propose or justify. Supply a genesis state whose validator registry is \
         populated (note that the compact interop format carries no registry at all)",
    );
    let registry_len = u64::try_from(state.validators.len())
        .context("genesis state validator-registry length does not fit in u64")?;
    ensure!(
        registry_len <= chain_config.validator_registry_limit,
        "genesis state contains {registry_len} validators, exceeding genesis config validator_registry_limit {}",
        chain_config.validator_registry_limit,
    );
    // Defense-in-depth: the runtime `validator_registry_limit` is an operator
    // knob and may be raised above the compile-time SSZ cap. A registry past the
    // SSZ cap makes `State::hash_tree_root` collapse the validators subtree to a
    // zero hash — a silently-wrong, un-re-decodable genesis. Bound against the
    // SSZ cap regardless of the runtime knob.
    ensure!(
        registry_len <= config::VALIDATOR_REGISTRY_LIMIT as u64,
        "genesis state contains {registry_len} validators, exceeding the SSZ validator-registry cap {}",
        config::VALIDATOR_REGISTRY_LIMIT,
    );
    let historical_roots = u64::try_from(state.historical_block_hashes.len())
        .context("genesis state historical root count does not fit in u64")?;
    ensure!(
        historical_roots <= chain_config.historical_roots_limit,
        "genesis state contains {historical_roots} historical roots, exceeding genesis config historical_roots_limit {}",
        chain_config.historical_roots_limit,
    );
    debug!(
        validators = state.num_validators(),
        validator_registry_limit = chain_config.validator_registry_limit,
        historical_roots,
        historical_roots_limit = chain_config.historical_roots_limit,
        "genesis state limits accepted",
    );
    Ok(())
}

fn default_genesis_time(chain_config: &ChainConfig) -> Result<u64> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before UNIX epoch")?
        .as_secs();
    let delay = chain_config
        .seconds_per_slot
        .checked_mul(DEFAULT_GENESIS_DELAY_SLOTS)
        .context("default genesis delay overflowed")?;
    now.checked_add(delay)
        .context("default genesis timestamp overflowed")
}

fn hex32(bytes: [u8; 32]) -> String {
    let mut out = String::with_capacity(66);
    out.push_str("0x");
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    // PublicKey is re-exported from crypto (crypto/lib.rs → types::PublicKey), not
    // protocol; ValidatorIndex from protocol. Both are test-only here.
    use crypto::PublicKey;
    use protocol::ValidatorIndex;
    use ssz::encode;

    /// A deterministic `n`-entry registry for tests that only need a populated
    /// `State.validators` of a given length (pubkey `i` filled with byte `i`).
    fn dummy_registry(n: u64) -> Vec<Validator> {
        (0..n)
            .map(|i| {
                // `& 0xff` makes this conversion infallible; the fallback is
                // unreachable and exists only to keep the expression total.
                let seed = u8::try_from(i & 0xff).unwrap_or(0);
                Validator::new(
                    PublicKey::new([seed; PublicKey::LEN]),
                    ValidatorIndex::new(i),
                )
            })
            .collect()
    }

    fn decode_hex_fixture(name: &str) -> Vec<u8> {
        let hex = std::fs::read_to_string(fixtures::fixture_path(name)).expect("read hex fixture");
        hex::decode(hex.split_whitespace().collect::<String>()).expect("fixture must be valid hex")
    }

    #[test]
    fn loads_chain_config_from_yaml() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("devnet.yaml");
        std::fs::write(
            &path,
            ChainConfig::default().to_yaml().expect("serialize config"),
        )
        .expect("write config");

        let loaded = load_chain_config(Some(&path)).expect("load config");

        assert_eq!(loaded, ChainConfig::default());
    }

    #[test]
    fn loads_state_from_ssz() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("genesis.ssz");
        let state = synthesize_state(1_700_000_000, dummy_registry(4));
        std::fs::write(&path, encode(&state)).expect("write state");

        let loaded = load_state(&path).expect("load state");

        assert_eq!(loaded, state);
    }

    #[test]
    fn validate_state_limits_rejects_registry_over_limit() {
        // The registry length is the bound: a registry one past the configured
        // limit is refused.
        let mut state = synthesize_state(1_700_000_000, dummy_registry(3));
        state.validators.push(Validator::new(
            PublicKey::new([3u8; PublicKey::LEN]),
            ValidatorIndex::new(3),
        ));
        let chain_config = ChainConfig {
            validator_registry_limit: 3,
            ..ChainConfig::default()
        };
        let err = validate_state_limits(&state, &chain_config)
            .expect_err("over-limit registry must be refused");
        assert!(
            err.to_string()
                .contains("exceeding genesis config validator_registry_limit"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn loads_ream_legacy_local_pq_state_from_ssz() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("genesis.ssz");
        std::fs::write(&path, decode_hex_fixture("genesis-2node.ssz.hex"))
            .expect("write legacy state");

        let loaded = load_state(&path).expect("load legacy state");
        let block = anchor_block_for_state(&loaded).expect("derive anchor block");

        // The compact interop format carries no validators tail, so the legacy
        // anchor has an EMPTY registry — the declared count is validated during
        // decode and discarded. `validate_state_limits` refuses such a state as
        // a chain anchor; this test exercises the decoder alone.
        assert_eq!(loaded.num_validators(), 0);
        assert_eq!(loaded.config.genesis_time, 1_778_169_008);
        assert!(loaded.historical_block_hashes.is_empty());
        assert!(loaded.justified_slots.is_empty());
        assert_eq!(
            loaded.latest_block_header.body_root,
            block.body.hash_tree_root().into()
        );
        assert_eq!(block.state_root, loaded.hash_tree_root().into());
        assert_eq!(
            hex::encode(loaded.hash_tree_root()),
            // Moved when the in-state config dropped `num_validators`: the
            // config container went from two fields to one, so the state root
            // changes. The 145-byte devnet0 payload itself is unchanged.
            "9bcc325e28fd8fb4882da3406303fb2048600463806fc9819b7ed527115b6f58"
        );
        assert_eq!(
            hex::encode(block.hash_tree_root()),
            // Anchor-block root follows the state root above (the block
            // commits to `state_root`).
            "cbe48721138a0e2b9dabcf556a039c10bd288a87fe8d02f12421631756e7bc4f"
        );
    }

    #[test]
    fn synthesis_requires_genesis_manifest() {
        // Rewrite of the former `synthesizes_state_from_validator_assignments`,
        // whose premise this change inverts: the registry IS the validator
        // count, so an absent manifest would synthesize a zero-validator chain
        // that can never propose or justify. It is now a hard error.
        let dir = tempfile::tempdir().expect("create temp dir");
        let validators = dir.path().join("validators.yaml");
        std::fs::write(&validators, "ream: [0, 1, 2, 3]\n").expect("write validators");

        let err = load_or_synthesize_state(None, &ChainConfig::default(), &validators)
            .expect_err("absent manifest must be refused");
        let rendered = format!("{err:#}");
        assert!(
            rendered.contains("genesis_validators.yaml"),
            "error must name the missing manifest, got: {rendered}"
        );
    }

    #[test]
    fn synthesizes_state_with_pubkey_manifest_populates_registry() {
        // Write BOTH the assignment file and its sibling genesis_validators.yaml,
        // then synthesize: State.validators is populated and index-ordered, and
        // its length is the validator count.
        let dir = tempfile::tempdir().expect("temp dir");
        let validators = dir.path().join("validators.yaml");
        std::fs::write(&validators, "ream_0:\n  - 0\nleanrust_1:\n  - 1\n")
            .expect("write validators");
        let manifest = dir.path().join("genesis_validators.yaml");
        // Unprefixed lower-case hex, mirroring the manifest's `hex::encode`.
        let pk0 = hex::encode([0_u8; PublicKey::LEN]);
        let pk1 = hex::encode([1_u8; PublicKey::LEN]);
        std::fs::write(
            &manifest,
            format!("genesis_validators:\n  - {pk0}\n  - {pk1}\n"),
        )
        .expect("write manifest");

        let state =
            load_or_synthesize_state(None, &ChainConfig::default(), &validators).expect("state");

        assert_eq!(state.num_validators(), 2);
        assert_eq!(state.validators.len(), 2);
        assert_eq!(state.validators[0].index, ValidatorIndex::new(0));
        assert_eq!(state.validators[1].index, ValidatorIndex::new(1));
        assert_eq!(
            state.validators[1].pubkey.as_slice(),
            &[1_u8; PublicKey::LEN]
        );
    }

    #[test]
    fn synthesize_rejects_manifest_count_mismatch() {
        // A present-but-invalid manifest (1 pubkey for 2 validators) must be a
        // HARD error through the lean-cli seam — never a silent empty registry.
        let dir = tempfile::tempdir().expect("temp dir");
        let validators = dir.path().join("validators.yaml");
        std::fs::write(&validators, "ream_0:\n  - 0\nleanrust_1:\n  - 1\n")
            .expect("write validators");
        let manifest = dir.path().join("genesis_validators.yaml");
        let pk0 = hex::encode([0_u8; PublicKey::LEN]);
        std::fs::write(&manifest, format!("genesis_validators:\n  - {pk0}\n"))
            .expect("write manifest");

        let err = load_or_synthesize_state(None, &ChainConfig::default(), &validators)
            .expect_err("count mismatch must fail");

        assert!(
            format!("{err:#}").contains("genesis pubkey manifest")
                || format!("{err:#}").contains("expected 2"),
            "unexpected error: {err:#}",
        );
    }

    #[test]
    fn anchor_block_matches_state_root() {
        let state = synthesize_state(1_700_000_000, dummy_registry(4));

        let block = anchor_block_for_state(&state).expect("derive block");

        assert_eq!(block.state_root, state.hash_tree_root().into());
        assert_eq!(block.body, BlockBody::default());
    }

    #[test]
    fn supplied_state_is_validated_against_chain_config() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("genesis.ssz");
        let state = synthesize_state(1_700_000_000, dummy_registry(4));
        std::fs::write(&path, encode(&state)).expect("write state");
        let chain_config = ChainConfig {
            validator_registry_limit: 3,
            ..ChainConfig::default()
        };

        let err = load_or_synthesize_state(Some(&path), &chain_config, dir.path())
            .expect_err("state exceeds validator limit");

        assert!(
            err.to_string().contains("validator_registry_limit"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn validate_state_limits_rejects_registry_over_ssz_cap() {
        // A raised runtime knob must not admit a registry past the compile-time
        // SSZ cap. The subject carries a REAL over-cap registry: the bound is
        // registry-derived now, and an empty registry would trip the
        // empty-registry check first. Calls `validate_state_limits` directly —
        // routing through the load path would hit the SSZ decoder's own
        // registry-limit check and surface a DecodeError instead.
        let over_cap = config::VALIDATOR_REGISTRY_LIMIT + 1;
        let state = synthesize_state(1_700_000_000, dummy_registry(over_cap as u64));
        let chain_config = ChainConfig {
            validator_registry_limit: over_cap as u64 + 1_000,
            ..ChainConfig::default()
        };

        let err =
            validate_state_limits(&state, &chain_config).expect_err("registry exceeds SSZ cap");

        assert!(
            err.to_string().contains("SSZ validator-registry cap"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn validate_state_limits_rejects_empty_registry() {
        // An empty registry means a zero validator count: such a node rejects
        // every attestation and can never propose. Refuse it as a chain anchor
        // rather than starting a silent non-participant.
        let state = synthesize_state(1_700_000_000, Vec::new());
        let err = validate_state_limits(&state, &ChainConfig::default())
            .expect_err("empty registry must be refused");
        assert!(
            err.to_string().contains("empty validator registry"),
            "unexpected error: {err:#}"
        );
    }
}
