//! Pins the libp2p agent string to this binary crate's own version.
//!
//! `lean_cli::node_config::AGENT_VERSION` is built from lean-cli's
//! `CARGO_PKG_VERSION`. Both crates use `version.workspace = true` today,
//! so the strings match. This test fails the moment they diverge, which is
//! the point: the agent string is wire-visible to peers and must track the
//! binary, not the library it happens to be defined in.

#[test]
fn agent_version_tracks_the_binary_version() {
    assert_eq!(
        lean_cli::node_config::AGENT_VERSION,
        concat!("lean-rust/", env!("CARGO_PKG_VERSION"))
    );
}
