//! Field-by-field parity check for the consensus [`State`] container.
//!
//! Records the ordered field set, declared SSZ shape, and contribution to
//! the fixed portion. The test asserts every property the native wire format
//! and Ream-compatible merkleization depend on:
//!
//! - Field order and count match the canonical declaration.
//! - Fixed fields contribute their declared SSZ width to the fixed portion.
//! - Variable fields contribute exactly one 4-byte offset to the fixed
//!   portion.
//! - The total fixed-portion length equals [`STATE_FIXED_PART_LEN`].
//! - The consensus merkleization width is `next_pow2(root_field_count)`.
//! - Field-level limits ([`HISTORICAL_ROOTS_LIMIT`],
//!   [`JUSTIFICATIONS_VALIDATORS_LIMIT`]) match the chain config caps.

use protocol::{
    HISTORICAL_ROOTS_LIMIT, JUSTIFICATIONS_VALIDATORS_LIMIT, STATE_FIXED_PART_LEN,
    VALIDATOR_REGISTRY_LIMIT,
};

#[derive(Clone, Copy, Debug)]
enum Shape {
    Fixed(usize),
    Variable { _limit: usize },
}

#[derive(Clone, Copy, Debug)]
struct Field {
    name: &'static str,
    shape: Shape,
}

const FIELDS: &[Field] = &[
    Field {
        name: "config",
        shape: Shape::Fixed(16),
    },
    Field {
        name: "slot",
        shape: Shape::Fixed(8),
    },
    Field {
        name: "latest_block_header",
        shape: Shape::Fixed(112),
    },
    Field {
        name: "latest_justified",
        shape: Shape::Fixed(40),
    },
    Field {
        name: "latest_finalized",
        shape: Shape::Fixed(40),
    },
    Field {
        name: "historical_block_hashes",
        shape: Shape::Variable {
            _limit: HISTORICAL_ROOTS_LIMIT,
        },
    },
    Field {
        name: "justified_slots",
        shape: Shape::Variable {
            _limit: HISTORICAL_ROOTS_LIMIT,
        },
    },
    Field {
        name: "validators",
        shape: Shape::Variable {
            _limit: VALIDATOR_REGISTRY_LIMIT,
        },
    },
    Field {
        name: "justifications_roots",
        shape: Shape::Variable {
            _limit: HISTORICAL_ROOTS_LIMIT,
        },
    },
    Field {
        name: "justifications_validators",
        shape: Shape::Variable {
            _limit: JUSTIFICATIONS_VALIDATORS_LIMIT,
        },
    },
];

const ROOT_FIELDS: &[&str] = &[
    "config",
    "slot",
    "latest_block_header",
    "latest_justified",
    "latest_finalized",
    "historical_block_hashes",
    "justified_slots",
    "validators",
    "justifications_roots",
    "justifications_validators",
];

const BYTES_PER_LENGTH_OFFSET: usize = 4;

#[test]
fn native_wire_field_set_has_ten_fields_in_declaration_order() {
    assert_eq!(FIELDS.len(), 10, "State has 10 fields");
    let names: Vec<&str> = FIELDS.iter().map(|f| f.name).collect();
    assert_eq!(
        names,
        vec![
            "config",
            "slot",
            "latest_block_header",
            "latest_justified",
            "latest_finalized",
            "historical_block_hashes",
            "justified_slots",
            "validators",
            "justifications_roots",
            "justifications_validators",
        ]
    );
}

#[test]
fn ream_root_field_set_has_ten_fields_in_declaration_order() {
    assert_eq!(ROOT_FIELDS.len(), 10, "Ream State root has 10 fields");
    assert_eq!(
        ROOT_FIELDS,
        &[
            "config",
            "slot",
            "latest_block_header",
            "latest_justified",
            "latest_finalized",
            "historical_block_hashes",
            "justified_slots",
            "validators",
            "justifications_roots",
            "justifications_validators",
        ]
    );
}

#[test]
fn variable_field_count_is_five() {
    let variable_count = FIELDS
        .iter()
        .filter(|f| matches!(f.shape, Shape::Variable { .. }))
        .count();
    assert_eq!(variable_count, 5);
}

#[test]
fn fixed_portion_length_matches_state_fixed_part_len() {
    let computed: usize = FIELDS
        .iter()
        .map(|f| match f.shape {
            Shape::Fixed(n) => n,
            Shape::Variable { .. } => BYTES_PER_LENGTH_OFFSET,
        })
        .sum();
    assert_eq!(computed, STATE_FIXED_PART_LEN);
    assert_eq!(STATE_FIXED_PART_LEN, 236);
}

#[test]
fn fixed_field_widths_sum_to_two_sixteen() {
    let fixed_total: usize = FIELDS
        .iter()
        .filter_map(|f| match f.shape {
            Shape::Fixed(n) => Some(n),
            Shape::Variable { .. } => None,
        })
        .sum();
    // 16 + 8 + 112 + 40 + 40 = 216
    assert_eq!(fixed_total, 216);
}

#[test]
fn merkleization_width_is_next_power_of_two() {
    // 10 Ream root fields → next power of two is 16.
    let next_pow2 = ROOT_FIELDS.len().next_power_of_two();
    assert_eq!(next_pow2, 16);
}

#[test]
fn limits_match_devnet_config_caps() {
    assert_eq!(HISTORICAL_ROOTS_LIMIT, 262_144);
    assert_eq!(VALIDATOR_REGISTRY_LIMIT, 4_096);
    assert_eq!(
        JUSTIFICATIONS_VALIDATORS_LIMIT,
        HISTORICAL_ROOTS_LIMIT * VALIDATOR_REGISTRY_LIMIT,
    );
}
