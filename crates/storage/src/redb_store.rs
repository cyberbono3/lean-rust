//! Persistent [`Store`] adapter backed by an embedded `redb` key-value store.
//!
//! Mirrors [`crate::MemoryStore`] semantics but survives process restarts:
//! blocks, post-states, and the canonical-chain head live in three tables on
//! disk. Values are SSZ-encoded; keys are 32-byte roots. The
//! [`Store::save_accepted`] override commits block, state, and head in one
//! write transaction — the commit is the atomic barrier, so a crash never
//! strands the head ahead of its payload and readers never observe a torn
//! intermediate state.
//!
//! The module is named `redb_store` (not `redb`) so a crate-root module never
//! shadows the extern `redb` crate the adapter imports.

use std::path::Path;
use std::sync::OnceLock;

use protocol::{Checkpoint, SignedBlockWithAttestation, State};
use redb::{Database, TableDefinition};
use types::Bytes32;

use crate::error::StorageError;
use crate::store::{HeadInfo, Store};

/// `root -> SSZ(SignedBlockWithAttestation)`.
const BLOCKS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("blocks");
/// `root -> SSZ(State)`.
const STATES: TableDefinition<&[u8], &[u8]> = TableDefinition::new("states");
/// Singleton canonical-head record: `HEAD_KEY -> SSZ(head) ++ SSZ(finalized)`.
const HEAD: TableDefinition<&[u8], &[u8]> = TableDefinition::new("head");

/// Fixed key for the single row in the [`HEAD`] table.
const HEAD_KEY: &[u8] = b"head";

/// Persistent `redb`-backed [`Store`] adapter. Construct with [`Self::new`];
/// share across services via `Arc<RedbStore>` or `Arc<dyn Store>`.
pub struct RedbStore {
    db: Database,
}

impl RedbStore {
    /// Opens (creating if absent) a persistent store at `path`.
    ///
    /// The parent directory is created if missing, so a caller-supplied
    /// `--storage-path` under a not-yet-existing data dir succeeds instead of
    /// failing with a bare "no such file or directory" from the backend. All
    /// three tables are created eagerly so a read that precedes the first write
    /// returns `Ok(None)` rather than a missing-table error.
    ///
    /// # Errors
    /// [`StorageError::Backend`] if the parent directory cannot be created, the
    /// database cannot be opened/created, or the initial table-creation
    /// transaction fails.
    pub fn new(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(backend)?;
        }
        let db = Database::create(path).map_err(backend)?;
        let txn = db.begin_write().map_err(backend)?;
        {
            txn.open_table(BLOCKS).map_err(backend)?;
            txn.open_table(STATES).map_err(backend)?;
            txn.open_table(HEAD).map_err(backend)?;
        }
        txn.commit().map_err(backend)?;
        Ok(Self { db })
    }

    fn get(
        &self,
        table: TableDefinition<&[u8], &[u8]>,
        key: &[u8],
    ) -> Result<Option<Vec<u8>>, StorageError> {
        let txn = self.db.begin_read().map_err(backend)?;
        let table = txn.open_table(table).map_err(backend)?;
        let value = table.get(key).map_err(backend)?;
        Ok(value.map(|guard| guard.value().to_vec()))
    }

    /// Existence probe that never materializes the value — avoids the
    /// `guard.value().to_vec()` copy of a multi-KB SSZ payload on the
    /// `has_block`/`has_state` hot path.
    fn contains(
        &self,
        table: TableDefinition<&[u8], &[u8]>,
        key: &[u8],
    ) -> Result<bool, StorageError> {
        let txn = self.db.begin_read().map_err(backend)?;
        let table = txn.open_table(table).map_err(backend)?;
        Ok(table.get(key).map_err(backend)?.is_some())
    }

    fn put(
        &self,
        table: TableDefinition<&[u8], &[u8]>,
        key: &[u8],
        value: &[u8],
    ) -> Result<(), StorageError> {
        let txn = self.db.begin_write().map_err(backend)?;
        {
            let mut table = txn.open_table(table).map_err(backend)?;
            table.insert(key, value).map_err(backend)?;
        }
        txn.commit().map_err(backend)?;
        Ok(())
    }
}

/// Maps any `redb` error into the crate's opaque backend variant.
fn backend<E: std::fmt::Display>(err: E) -> StorageError {
    StorageError::Backend {
        message: err.to_string(),
    }
}

/// Fixed SSZ byte length of one [`Checkpoint`], computed once. `Checkpoint` is
/// a fixed-size SSZ container, so encoding a default value yields the length
/// every checkpoint encodes to.
fn checkpoint_len() -> usize {
    static LEN: OnceLock<usize> = OnceLock::new();
    *LEN.get_or_init(|| ssz::encode(&Checkpoint::default()).len())
}

/// Encodes a [`HeadInfo`] as `SSZ(head) ++ SSZ(finalized)` — two fixed-size
/// checkpoints of equal length.
fn encode_head(info: HeadInfo) -> Vec<u8> {
    let mut bytes = ssz::encode(&info.head);
    bytes.extend_from_slice(&ssz::encode(&info.finalized));
    bytes
}

/// Decodes a [`HeadInfo`] written by [`encode_head`], re-validating the
/// `finalized.slot <= head.slot` invariant at the deserialization seam (mirrors
/// the `try_new` guard used when persisting the genesis anchor).
fn decode_head(bytes: &[u8]) -> Result<HeadInfo, StorageError> {
    // Split at the fixed SSZ length of one Checkpoint rather than len/2, so a
    // future variable-length HeadInfo field cannot silently corrupt the split.
    let checkpoint_len = checkpoint_len();
    if bytes.len() != checkpoint_len * 2 {
        return Err(StorageError::Backend {
            message: format!(
                "head record has {} bytes, expected {}",
                bytes.len(),
                checkpoint_len * 2
            ),
        });
    }
    let head: Checkpoint = ssz::decode(&bytes[..checkpoint_len]).map_err(backend)?;
    let finalized: Checkpoint = ssz::decode(&bytes[checkpoint_len..]).map_err(backend)?;
    HeadInfo::try_new(head, finalized).map_err(backend)
}

impl Store for RedbStore {
    fn save_block(
        &self,
        root: Bytes32,
        block: SignedBlockWithAttestation,
    ) -> Result<(), StorageError> {
        self.put(BLOCKS, root.0.as_slice(), &ssz::encode(&block))
    }

    fn save_state(&self, root: Bytes32, state: State) -> Result<(), StorageError> {
        self.put(STATES, root.0.as_slice(), &ssz::encode(&state))
    }

    fn save_head(&self, info: HeadInfo) -> Result<(), StorageError> {
        self.put(HEAD, HEAD_KEY, &encode_head(info))
    }

    /// Atomic override: block, state, and head are inserted inside one write
    /// transaction, head last; `commit()` makes all three durable together or
    /// none at all.
    fn save_accepted(
        &self,
        block_root: Bytes32,
        block: SignedBlockWithAttestation,
        state: State,
        head: HeadInfo,
    ) -> Result<(), StorageError> {
        let block_bytes = ssz::encode(&block);
        let state_bytes = ssz::encode(&state);
        let head_bytes = encode_head(head);
        let key = block_root.0;

        let txn = self.db.begin_write().map_err(backend)?;
        {
            let mut blocks = txn.open_table(BLOCKS).map_err(backend)?;
            blocks
                .insert(key.as_slice(), block_bytes.as_slice())
                .map_err(backend)?;
            let mut states = txn.open_table(STATES).map_err(backend)?;
            states
                .insert(key.as_slice(), state_bytes.as_slice())
                .map_err(backend)?;
            // Head last within the transaction; commit is the atomic barrier.
            let mut head_table = txn.open_table(HEAD).map_err(backend)?;
            head_table
                .insert(HEAD_KEY, head_bytes.as_slice())
                .map_err(backend)?;
        }
        txn.commit().map_err(backend)?;
        Ok(())
    }

    fn has_block(&self, root: &Bytes32) -> Result<bool, StorageError> {
        self.contains(BLOCKS, root.0.as_slice())
    }

    fn has_state(&self, root: &Bytes32) -> Result<bool, StorageError> {
        self.contains(STATES, root.0.as_slice())
    }

    fn load_block(
        &self,
        root: &Bytes32,
    ) -> Result<Option<SignedBlockWithAttestation>, StorageError> {
        match self.get(BLOCKS, root.0.as_slice())? {
            Some(bytes) => Ok(Some(ssz::decode(&bytes).map_err(backend)?)),
            None => Ok(None),
        }
    }

    fn load_state(&self, root: &Bytes32) -> Result<Option<State>, StorageError> {
        match self.get(STATES, root.0.as_slice())? {
            Some(bytes) => Ok(Some(ssz::decode(&bytes).map_err(backend)?)),
            None => Ok(None),
        }
    }

    fn load_head(&self) -> Result<Option<HeadInfo>, StorageError> {
        match self.get(HEAD, HEAD_KEY)? {
            Some(bytes) => Ok(Some(decode_head(&bytes)?)),
            None => Ok(None),
        }
    }
}
