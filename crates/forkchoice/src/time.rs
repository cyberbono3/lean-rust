//! Forkchoice clock value and the 4-phase classification.
//!
//! [`Time`] is the count of intervals elapsed since chain genesis;
//! [`INTERVALS_PER_SLOT`](config::INTERVALS_PER_SLOT) intervals form a slot.
//! [`Phase`] classifies `time % INTERVALS_PER_SLOT` into the four spec
//! phases that drive [`crate::Store::tick_interval`]'s dispatch.

use core::fmt;

use config::INTERVALS_PER_SLOT;

/// Phase of the 4-interval slot cycle.
///
/// The variant-to-interval mapping is mandated by leanSpec; the
/// [`crate::Store::tick_interval`] dispatch matches on this enum
/// exhaustively, so adding a variant or relabelling a hook is a
/// compile-time error rather than a silent parity break.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[must_use]
pub enum Phase {
    /// Interval 0 — start-of-slot proposal hook.
    Proposal,
    /// Interval 1 — validator voting period; no store-side action.
    Idle,
    /// Interval 2 — refresh safe attestation target.
    UpdateSafeTarget,
    /// Interval 3 — end-of-slot vote acceptance.
    AcceptNewVotes,
}

/// Forkchoice time: intervals since chain genesis.
///
/// Wraps `u64` to carry the slot/interval/phase derivations and the
/// checked-advance arithmetic at the type level rather than at every call
/// site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[must_use]
pub struct Time(u64);

impl Time {
    /// Genesis time (`Time(0)`).
    pub const ZERO: Time = Time(0);

    /// Constructs a [`Time`] from a raw `u64` interval count.
    pub const fn new(t: u64) -> Self {
        Self(t)
    }

    /// Returns the raw `u64`.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Slot index: `time / INTERVALS_PER_SLOT`.
    #[must_use]
    pub const fn slot(self) -> u64 {
        self.0 / INTERVALS_PER_SLOT
    }

    /// Intra-slot interval index: `time % INTERVALS_PER_SLOT`.
    #[must_use]
    pub const fn interval(self) -> u64 {
        self.0 % INTERVALS_PER_SLOT
    }

    /// 4-phase classification derived from [`Self::interval`].
    pub const fn phase(self) -> Phase {
        match self.interval() {
            0 => Phase::Proposal,
            1 => Phase::Idle,
            2 => Phase::UpdateSafeTarget,
            // `INTERVALS_PER_SLOT == 4` ⇒ `interval ∈ {0, 1, 2, 3}`; the
            // residue after matching {0,1,2} is exactly 3. `const fn`
            // cannot panic in stable Rust, so this arm is reached without
            // an `unreachable!()`.
            _ => Phase::AcceptNewVotes,
        }
    }

    /// Advances by one interval. Returns [`None`] when the raw `u64` would
    /// overflow.
    #[must_use = "check the Option to handle the overflow case"]
    pub const fn checked_advance(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(t) => Some(Self(t)),
            None => None,
        }
    }
}

impl From<u64> for Time {
    fn from(t: u64) -> Self {
        Self(t)
    }
}

impl From<Time> for u64 {
    fn from(t: Time) -> u64 {
        t.0
    }
}

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn time_helpers_table() {
        let cases: &[(u64, u64, u64, Phase)] = &[
            (0, 0, 0, Phase::Proposal),
            (1, 0, 1, Phase::Idle),
            (2, 0, 2, Phase::UpdateSafeTarget),
            (3, 0, 3, Phase::AcceptNewVotes),
            (4, 1, 0, Phase::Proposal),
            (15, 3, 3, Phase::AcceptNewVotes),
            (16, 4, 0, Phase::Proposal),
        ];
        for &(t, slot, interval, phase) in cases {
            let time = Time::new(t);
            assert_eq!(time.slot(), slot, "slot at t={t}");
            assert_eq!(time.interval(), interval, "interval at t={t}");
            assert_eq!(time.phase(), phase, "phase at t={t}");
        }
    }

    #[test]
    fn checked_advance_handles_overflow() {
        assert_eq!(Time::new(0).checked_advance(), Some(Time::new(1)));
        assert_eq!(
            Time::new(u64::MAX - 1).checked_advance(),
            Some(Time::new(u64::MAX))
        );
        assert_eq!(Time::new(u64::MAX).checked_advance(), None);
    }

    #[test]
    fn from_into_u64_round_trip() {
        let t: Time = 42_u64.into();
        let raw: u64 = t.into();
        assert_eq!(raw, 42);
    }

    #[test]
    fn display_is_decimal_u64() {
        assert_eq!(format!("{}", Time::new(7)), "7");
    }

    #[test]
    fn default_is_zero() {
        assert_eq!(Time::default(), Time::ZERO);
    }
}
