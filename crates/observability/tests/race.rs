//! Concurrency test for `init_tracing`'s one-shot init claim.
//!
//! Lives in its own integration binary (separate process) so the
//! process-wide `INIT_CLAIMED` slot starts fresh — exactly one of the
//! racing threads may win.

#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::unwrap_used
)]

use std::sync::{Arc, Barrier};
use std::thread;

use lean_observability::{init_tracing, TracingGuard, TracingInitError, Verbosity};

#[test]
fn concurrent_init_yields_exactly_one_ok() {
    const THREADS: usize = 16;

    // A barrier so all threads hit `init_tracing` as simultaneously as
    // the scheduler allows, maximizing the race.
    let barrier = Arc::new(Barrier::new(THREADS));
    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                init_tracing(Verbosity::Info, None)
            })
        })
        .collect();

    let results: Vec<Result<TracingGuard, TracingInitError>> =
        handles.into_iter().map(|h| h.join().unwrap()).collect();

    let ok = results.iter().filter(|r| r.is_ok()).count();
    let already = results
        .iter()
        .filter(|r| matches!(r, Err(TracingInitError::AlreadyInitialized)))
        .count();

    assert_eq!(ok, 1, "exactly one thread must win init");
    assert_eq!(
        already,
        THREADS - 1,
        "every loser must observe AlreadyInitialized",
    );
}
