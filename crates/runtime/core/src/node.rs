//! The [`Node`] composition root.
//!
//! Holds up to six [`Arc<dyn Service>`] slots (`chain`, `p2p`, `sync`,
//! `duties`, `http`, `metrics`) plus a lifecycle state cell driven by
//! [`crate::lifecycle`].

use std::sync::Arc;

use parking_lot::Mutex;

use crate::config::NodeConfig;
use crate::service::Service;

/// One started service, paired with its slot label for error reporting.
pub(crate) type NamedService = (&'static str, Arc<dyn Service>);

/// Lifecycle state: `None` while idle, `Some(vec)` after `start` succeeds.
/// Replacing this single cell atomically swaps "not started" for "started
/// with these N services" and back, with no derivable inconsistency
/// between separate flags.
pub(crate) type LifecycleState = Mutex<Option<Vec<NamedService>>>;

/// Composition root for the runtime shell.
///
/// Construct via [`Node::new`] and wire services through the `with_*`
/// builder methods. Each setter consumes `self` and returns the updated
/// `Node`, so a partially-wired node is built up in a single chained
/// expression. Unwired slots are skipped at lifecycle time.
///
/// Lifecycle methods (`start`, `stop`, `status`, `run`) live in
/// [`crate::lifecycle`].
pub struct Node {
    pub(crate) config: NodeConfig,
    pub(crate) chain: Option<Arc<dyn Service>>,
    pub(crate) p2p: Option<Arc<dyn Service>>,
    pub(crate) sync: Option<Arc<dyn Service>>,
    pub(crate) duties: Option<Arc<dyn Service>>,
    pub(crate) http: Option<Arc<dyn Service>>,
    pub(crate) metrics: Option<Arc<dyn Service>>,
    pub(crate) state: LifecycleState,
}

impl Node {
    /// Builds an empty node carrying `config`. Slots are populated via the
    /// `with_*` builder methods.
    #[must_use]
    pub fn new(config: NodeConfig) -> Self {
        Self {
            config,
            chain: None,
            p2p: None,
            sync: None,
            duties: None,
            http: None,
            metrics: None,
            state: Mutex::new(None),
        }
    }

    /// Wires the `chain` slot. Replaces any prior value.
    #[must_use]
    pub fn with_chain(mut self, svc: Arc<dyn Service>) -> Self {
        self.chain = Some(svc);
        self
    }

    /// Wires the `p2p` slot. Replaces any prior value.
    #[must_use]
    pub fn with_p2p(mut self, svc: Arc<dyn Service>) -> Self {
        self.p2p = Some(svc);
        self
    }

    /// Wires the `sync` slot. Replaces any prior value.
    #[must_use]
    pub fn with_sync(mut self, svc: Arc<dyn Service>) -> Self {
        self.sync = Some(svc);
        self
    }

    /// Wires the `duties` slot. Replaces any prior value.
    #[must_use]
    pub fn with_duties(mut self, svc: Arc<dyn Service>) -> Self {
        self.duties = Some(svc);
        self
    }

    /// Wires the `http` slot. Replaces any prior value.
    #[must_use]
    pub fn with_http(mut self, svc: Arc<dyn Service>) -> Self {
        self.http = Some(svc);
        self
    }

    /// Wires the `metrics` slot. Replaces any prior value.
    #[must_use]
    pub fn with_metrics(mut self, svc: Arc<dyn Service>) -> Self {
        self.metrics = Some(svc);
        self
    }
}

/// Function pointer that fetches the optional service for a given slot
/// from a `Node`. Used to drive [`SLOT_ORDER`] without naming each slot
/// inside every lifecycle method.
type SlotAccessor = fn(&Node) -> Option<&Arc<dyn Service>>;

/// Canonical start-order traversal. `Node::stop` walks this in reverse;
/// `Node::status` walks it forward.
///
/// Adding or reordering a slot is one edit here.
pub(crate) const SLOT_ORDER: [(&str, SlotAccessor); 6] = [
    ("chain", |n| n.chain.as_ref()),
    ("p2p", |n| n.p2p.as_ref()),
    ("sync", |n| n.sync.as_ref()),
    ("duties", |n| n.duties.as_ref()),
    ("http", |n| n.http.as_ref()),
    ("metrics", |n| n.metrics.as_ref()),
];

impl Node {
    /// Iterates the wired slots in start order, yielding
    /// `(slot_label, cloned_arc)` for each populated slot.
    pub(crate) fn ordered_slots(&self) -> Vec<NamedService> {
        SLOT_ORDER
            .iter()
            .filter_map(|(name, accessor)| accessor(self).map(|svc| (*name, Arc::clone(svc))))
            .collect()
    }
}
