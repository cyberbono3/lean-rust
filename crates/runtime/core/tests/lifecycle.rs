//! End-to-end lifecycle tests for `Node`: start order, reverse stop,
//! start-time unwinding, status aggregation, and the `run` driver.

#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::unwrap_used
)]

use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use async_trait::async_trait;
use parking_lot::Mutex;
use runtime_core::{Node, NodeConfig, NodeError, Service, ServiceFailure};
use static_assertions::{assert_impl_all, assert_obj_safe};
use tokio_util::sync::CancellationToken;

// -----------------------------------------------------------------------------
// compile-time witnesses
// -----------------------------------------------------------------------------

assert_obj_safe!(Service);
assert_impl_all!(dyn Service: Send, Sync);

// -----------------------------------------------------------------------------
// fake service
// -----------------------------------------------------------------------------

type Events = Arc<Mutex<Vec<String>>>;

fn events() -> Events {
    Arc::new(Mutex::new(Vec::new()))
}

#[derive(Default)]
struct FakeScript {
    start_err: Option<&'static str>,
    stop_err: Option<&'static str>,
    status_err: Option<&'static str>,
    stop_delay: Option<Duration>,
}

struct FakeService {
    name: &'static str,
    events: Events,
    script: FakeScript,
}

impl FakeService {
    fn new(name: &'static str, events: &Events) -> Self {
        Self {
            name,
            events: events.clone(),
            script: FakeScript::default(),
        }
    }

    fn with_start_err(mut self, msg: &'static str) -> Self {
        self.script.start_err = Some(msg);
        self
    }

    fn with_stop_err(mut self, msg: &'static str) -> Self {
        self.script.stop_err = Some(msg);
        self
    }

    fn with_status_err(mut self, msg: &'static str) -> Self {
        self.script.status_err = Some(msg);
        self
    }

    fn with_stop_delay(mut self, delay: Duration) -> Self {
        self.script.stop_delay = Some(delay);
        self
    }

    fn record(&self, phase: &str) {
        self.events.lock().push(format!("{phase}:{}", self.name));
    }
}

#[async_trait]
impl Service for FakeService {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn start(&self) -> anyhow::Result<()> {
        self.record("start");
        match self.script.start_err {
            Some(msg) => Err(anyhow!("{msg}")),
            None => Ok(()),
        }
    }

    async fn stop(&self, _cancel: CancellationToken) -> anyhow::Result<()> {
        self.record("stop");
        if let Some(delay) = self.script.stop_delay {
            tokio::time::sleep(delay).await;
        }
        match self.script.stop_err {
            Some(msg) => Err(anyhow!("{msg}")),
            None => Ok(()),
        }
    }

    async fn status(&self) -> anyhow::Result<()> {
        self.record("status");
        match self.script.status_err {
            Some(msg) => Err(anyhow!("{msg}")),
            None => Ok(()),
        }
    }
}

fn six_slot_node(events: &Events) -> Node {
    Node::new(NodeConfig::default())
        .with_chain(Arc::new(FakeService::new("chain", events)))
        .with_p2p(Arc::new(FakeService::new("p2p", events)))
        .with_sync(Arc::new(FakeService::new("sync", events)))
        .with_duties(Arc::new(FakeService::new("duties", events)))
        .with_http(Arc::new(FakeService::new("http", events)))
        .with_metrics(Arc::new(FakeService::new("metrics", events)))
}

fn snapshot(events: &Events) -> Vec<String> {
    events.lock().clone()
}

// -----------------------------------------------------------------------------
// tests
// -----------------------------------------------------------------------------

#[tokio::test]
async fn start_stop_ordering() {
    let events = events();
    let node = six_slot_node(&events);

    node.start().await.unwrap();
    node.stop().await.unwrap();

    assert_eq!(
        snapshot(&events),
        vec![
            "start:chain",
            "start:p2p",
            "start:sync",
            "start:duties",
            "start:http",
            "start:metrics",
            "stop:metrics",
            "stop:http",
            "stop:duties",
            "stop:sync",
            "stop:p2p",
            "stop:chain",
        ]
    );
}

#[tokio::test]
async fn start_failure_unwinds() {
    let events = events();
    let node = Node::new(NodeConfig::default())
        .with_chain(Arc::new(FakeService::new("chain", &events)))
        .with_p2p(Arc::new(
            FakeService::new("p2p", &events).with_start_err("boom"),
        ))
        .with_sync(Arc::new(FakeService::new("sync", &events)));

    let err = node.start().await.unwrap_err();

    assert!(
        matches!(&err, NodeError::StartFailed(ServiceFailure { service, source })
            if *service == "p2p" && source.to_string() == "boom"),
        "got {err:?}"
    );
    assert_eq!(
        snapshot(&events),
        vec!["start:chain", "start:p2p", "stop:chain"],
        "sync must not start; chain must be stopped",
    );
}

#[tokio::test]
async fn double_start_returns_already_started() {
    let events = events();
    let node =
        Node::new(NodeConfig::default()).with_chain(Arc::new(FakeService::new("chain", &events)));

    node.start().await.unwrap();
    let err = node.start().await.unwrap_err();
    assert!(matches!(err, NodeError::AlreadyStarted), "got {err:?}");
    // The second call must NOT have invoked the service.
    assert_eq!(snapshot(&events), vec!["start:chain"]);
}

#[tokio::test]
async fn stop_before_start_is_noop() {
    let events = events();
    let node = six_slot_node(&events);
    node.stop().await.unwrap();
    assert!(snapshot(&events).is_empty());
}

#[tokio::test]
async fn partial_node_skips_unwired_slots() {
    let events = events();
    let node = Node::new(NodeConfig::default())
        .with_chain(Arc::new(FakeService::new("chain", &events)))
        .with_p2p(Arc::new(FakeService::new("p2p", &events)));

    node.start().await.unwrap();
    node.stop().await.unwrap();

    assert_eq!(
        snapshot(&events),
        vec!["start:chain", "start:p2p", "stop:p2p", "stop:chain"]
    );
}

#[tokio::test]
async fn stop_continues_past_individual_failures() {
    let events = events();
    let node = Node::new(NodeConfig::default())
        .with_chain(Arc::new(FakeService::new("chain", &events)))
        .with_p2p(Arc::new(
            FakeService::new("p2p", &events).with_stop_err("p2p broke"),
        ))
        .with_sync(Arc::new(FakeService::new("sync", &events)));

    node.start().await.unwrap();
    let err = node.stop().await.unwrap_err();

    assert!(
        matches!(&err, NodeError::StopFailed(ServiceFailure { service, source })
            if *service == "p2p" && source.to_string() == "p2p broke"),
        "got {err:?}",
    );
    // All three stops attempted in reverse order even though p2p errored.
    assert_eq!(
        snapshot(&events),
        vec![
            "start:chain",
            "start:p2p",
            "start:sync",
            "stop:sync",
            "stop:p2p",
            "stop:chain",
        ]
    );
}

#[tokio::test]
async fn status_aggregates_named_errors() {
    let events = events();
    let node = Node::new(NodeConfig::default())
        .with_chain(Arc::new(FakeService::new("chain", &events)))
        .with_p2p(Arc::new(
            FakeService::new("p2p", &events).with_status_err("not ready"),
        ));

    let err = node.status().await.unwrap_err();

    let NodeError::ServicesUnhealthy { errors } = err else {
        panic!("expected NodeError::ServicesUnhealthy, got {err:?}");
    };
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].service, "p2p");
    assert_eq!(errors[0].source.to_string(), "not ready");
    // Display derives count + summary from `errors`.
    assert_eq!(
        NodeError::ServicesUnhealthy { errors }.to_string(),
        "node status reports 1 unhealthy service(s): p2p",
    );
}

#[tokio::test]
async fn status_returns_ok_when_all_healthy() {
    // Pins the empty-Vec early return: a regression that inverted the
    // `if errors.is_empty()` check would slip past every other test.
    let events = events();
    let node = six_slot_node(&events);
    node.start().await.unwrap();
    node.status().await.unwrap();
}

#[tokio::test]
async fn status_aggregates_in_slot_order() {
    // Two unhealthy services on non-adjacent slots: verify count + ordering
    // match `SLOT_ORDER` regardless of which positions failed.
    let events = events();
    let node = Node::new(NodeConfig::default())
        .with_chain(Arc::new(
            FakeService::new("chain", &events).with_status_err("a"),
        ))
        .with_p2p(Arc::new(FakeService::new("p2p", &events)))
        .with_sync(Arc::new(
            FakeService::new("sync", &events).with_status_err("c"),
        ));

    let NodeError::ServicesUnhealthy { errors } = node.status().await.unwrap_err() else {
        panic!("expected NodeError::ServicesUnhealthy");
    };
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0].service, "chain");
    assert_eq!(errors[0].source.to_string(), "a");
    assert_eq!(errors[1].service, "sync");
    assert_eq!(errors[1].source.to_string(), "c");
}

#[tokio::test]
async fn run_propagates_start_failure_without_calling_stop() {
    let events = events();
    let node = Node::new(NodeConfig::default()).with_chain(Arc::new(
        FakeService::new("chain", &events).with_start_err("nope"),
    ));
    let shutdown = CancellationToken::new();

    let err = node.run(shutdown).await.unwrap_err();
    assert!(
        matches!(&err, NodeError::StartFailed(ServiceFailure { service, source })
            if *service == "chain" && source.to_string() == "nope"),
        "got {err:?}",
    );
    // No stop events: chain never started successfully, so `run` never
    // reaches its `.cancelled().await` and never invokes `stop`.
    assert_eq!(snapshot(&events), vec!["start:chain"]);
}

#[tokio::test]
async fn node_can_restart_after_stop() {
    let events = events();
    let node =
        Node::new(NodeConfig::default()).with_chain(Arc::new(FakeService::new("chain", &events)));

    node.start().await.unwrap();
    node.stop().await.unwrap();
    node.start().await.unwrap();
    node.stop().await.unwrap();

    assert_eq!(
        snapshot(&events),
        vec!["start:chain", "stop:chain", "start:chain", "stop:chain"]
    );
}

#[tokio::test]
async fn run_starts_waits_for_cancel_then_stops() {
    let events = events();
    let node = Arc::new(six_slot_node(&events));
    let shutdown = CancellationToken::new();

    let driver = {
        let node = Arc::clone(&node);
        let shutdown = shutdown.clone();
        tokio::spawn(async move { node.run(shutdown).await })
    };

    // Give the driver a chance to drive every service through start.
    while events.lock().len() < 6 {
        tokio::task::yield_now().await;
    }

    shutdown.cancel();
    driver.await.unwrap().unwrap();

    assert_eq!(
        snapshot(&events),
        vec![
            "start:chain",
            "start:p2p",
            "start:sync",
            "start:duties",
            "start:http",
            "start:metrics",
            "stop:metrics",
            "stop:http",
            "stop:duties",
            "stop:sync",
            "stop:p2p",
            "stop:chain",
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn run_respects_shutdown_timeout() {
    let events = events();
    let node = Arc::new(
        Node::new(NodeConfig {
            shutdown_timeout: Duration::from_millis(100),
        })
        .with_chain(Arc::new(
            FakeService::new("chain", &events).with_stop_delay(Duration::from_secs(10)),
        )),
    );
    let shutdown = CancellationToken::new();

    let driver = {
        let node = Arc::clone(&node);
        let shutdown = shutdown.clone();
        tokio::spawn(async move { node.run(shutdown).await })
    };

    // Wait until chain has started.
    while events.lock().is_empty() {
        tokio::task::yield_now().await;
    }
    shutdown.cancel();

    let err = driver.await.unwrap().unwrap_err();
    assert!(
        matches!(&err, NodeError::StopFailed(ServiceFailure { service, source })
            if *service == "chain" && source.to_string().contains("deadline")),
        "got {err:?}",
    );
}

#[tokio::test(start_paused = true)]
async fn stop_with_zero_timeout_waits_indefinitely() {
    let events = events();
    let node = Node::new(NodeConfig {
        shutdown_timeout: Duration::ZERO,
    })
    .with_chain(Arc::new(
        FakeService::new("chain", &events).with_stop_delay(Duration::from_secs(60)),
    ));

    node.start().await.unwrap();

    let stop = tokio::spawn(async move { node.stop().await });
    tokio::time::advance(Duration::from_secs(30)).await;
    assert!(
        !stop.is_finished(),
        "stop must wait — zero timeout means no deadline"
    );
    tokio::time::advance(Duration::from_secs(31)).await;
    stop.await.unwrap().unwrap();
}
