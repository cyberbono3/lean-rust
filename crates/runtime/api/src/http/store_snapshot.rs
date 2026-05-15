//! JSON wire shapes for the `/eth/v1/...` head endpoints.
//!
//! Domain types (`storage::HeadInfo`, `protocol::Checkpoint`,
//! `types::Bytes32`) stay serde-free per the workspace architecture
//! rules; the DTOs below are the only place those shapes cross into a
//! serialisable form. Both DTOs are `pub(crate)` — the public API
//! surface is the [`crate::HttpService`], not the wire types.

use protocol::Checkpoint;
use serde::Serialize;
use storage::HeadInfo;

/// JSON view of a [`protocol::Checkpoint`] for the runtime-api wire
/// shape: `{"root":"0x<64hex>","slot":N}`. The root is pre-encoded as a
/// `0x`-prefixed lowercase hex string at the [`From`] boundary.
#[derive(Serialize)]
pub(crate) struct CheckpointDto {
    root: String,
    slot: u64,
}

/// JSON view of [`storage::HeadInfo`] returned by `GET /eth/v1/head`.
#[derive(Serialize)]
pub(crate) struct HeadInfoDto {
    head: CheckpointDto,
    finalized: CheckpointDto,
}

impl From<Checkpoint> for CheckpointDto {
    fn from(cp: Checkpoint) -> Self {
        Self {
            root: cp.root.to_hex(),
            slot: cp.slot.get(),
        }
    }
}

impl From<HeadInfo> for HeadInfoDto {
    fn from(info: HeadInfo) -> Self {
        Self {
            head: info.head.into(),
            finalized: info.finalized.into(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use protocol::Slot;
    use types::Bytes32;

    #[test]
    fn head_info_serialises_to_expected_wire_shape() {
        let cases: [(&str, HeadInfo, &str); 2] = [
            (
                "default",
                HeadInfo::default(),
                r#"{"head":{"root":"0x0000000000000000000000000000000000000000000000000000000000000000","slot":0},"finalized":{"root":"0x0000000000000000000000000000000000000000000000000000000000000000","slot":0}}"#,
            ),
            (
                "populated",
                HeadInfo::new(
                    Checkpoint::new(Bytes32::new([0xAB; 32]), Slot::new(7)),
                    Checkpoint::new(Bytes32::new([0xCD; 32]), Slot::new(3)),
                ),
                r#"{"head":{"root":"0xabababababababababababababababababababababababababababababababab","slot":7},"finalized":{"root":"0xcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd","slot":3}}"#,
            ),
        ];
        for (name, info, want) in cases {
            let json = serde_json::to_string(&HeadInfoDto::from(info)).unwrap();
            assert_eq!(json, want, "case {name}");
        }
    }
}
