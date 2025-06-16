use crate::rebalancing::{
    balance_shift::{BalanceShift, BalanceShiftImpactType},
    range::{Bound, Range},
};
use cosmwasm_std::{Decimal, SignedDecimal256};
use std::ops::Neg;

/// Represents a zone in the balance range
#[derive(Debug, PartialEq, Eq)]
pub struct Zone {
    range: Range,
    adjustment_rate: Decimal,
}

impl Zone {
    pub fn new(start: Bound, end: Bound, adjustment_rate: Decimal) -> Self {
        Self {
            range: Range::new(start, end).unwrap(),
            adjustment_rate,
        }
    }

    /// Compute the adjustment rate for a given balance shift and ideal range accumulated within this zone.
    pub fn compute_adjustment_rate(
        &self,
        balance_shift: &BalanceShift,
        ideal: Range,
    ) -> SignedDecimal256 {
        let overlap = self.range.intersect(balance_shift.range());

        let Some(overlap) = overlap else {
            return SignedDecimal256::zero();
        };

        let impact_type = balance_shift.get_impact_type(ideal);
        let segment_length = overlap.end().value() - overlap.start().value();

        let unsigned_cumulative_adjustment = self.adjustment_rate * segment_length;

        match impact_type {
            BalanceShiftImpactType::Debalance => {
                SignedDecimal256::from(unsigned_cumulative_adjustment).neg()
            }
            BalanceShiftImpactType::Rebalance => {
                SignedDecimal256::from(unsigned_cumulative_adjustment)
            }
            BalanceShiftImpactType::Neutral => SignedDecimal256::zero(),
        }
    }
}

// TODO: test 2
