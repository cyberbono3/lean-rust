//! Integration tests for the devnet0 YAML fixture.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use config::{Config, ConfigError, DEVNET_CONFIG};

const FIXTURE: &str = include_str!("data/devnet0.yaml");

#[test]
fn fixture_loads_into_devnet_config() {
    let cfg = Config::from_yaml(FIXTURE).unwrap();
    assert_eq!(cfg, DEVNET_CONFIG);
}

#[test]
fn fixture_round_trips_through_serde_yaml() {
    let parsed = Config::from_yaml(FIXTURE).unwrap();
    let re_emitted = parsed.to_yaml().unwrap();
    let re_parsed = Config::from_yaml(&re_emitted).unwrap();
    assert_eq!(parsed, re_parsed);
}

#[test]
fn fixture_passes_validation() {
    let cfg = Config::from_yaml(FIXTURE).unwrap();
    cfg.validate().unwrap();
}

#[test]
fn fixture_missing_field_is_rejected() {
    // Strip the last field; missing required field must be a YAML error.
    let truncated: String = FIXTURE
        .lines()
        .filter(|l| !l.starts_with("validator_registry_limit:"))
        .collect::<Vec<_>>()
        .join("\n");
    let err = Config::from_yaml(&truncated).unwrap_err();
    assert!(matches!(err, ConfigError::Yaml { .. }));
}
