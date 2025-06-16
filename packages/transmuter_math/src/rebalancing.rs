use std::ops::Neg;

use crate::TransmuterMathError as Error;
use cosmwasm_std::{Decimal, SignedDecimal256};

/// Represents a bound in a range, which can be either inclusive or exclusive
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Bound {
    /// Inclusive bound - the value is included in the range
    Inclusive(Decimal),
    /// Exclusive bound - the value is not included in the range
    Exclusive(Decimal),
}

impl Bound {
    /// Returns the underlying decimal value
    pub fn value(&self) -> Decimal {
        match self {
            Self::Inclusive(v) | Self::Exclusive(v) => *v,
        }
    }

    /// Returns true if the given value is within this bound
    pub fn contains(&self, value: Decimal) -> bool {
        match self {
            Self::Inclusive(bound) => value <= *bound,
            Self::Exclusive(bound) => value < *bound,
        }
    }
}

/// Compute fee or incentive adjustment for a single asset's balance movement.
///
/// This function calculates the incentive/rebate (if positive) or fee (if negative)
/// for a swap that moves an asset's balance from balance to balance_new. The goal is to
/// encourage movements toward the ideal balance range [ideal.start, ideal.end] and
/// discourage movements away from it.
pub fn compute_adjustment_value(
    balance: Decimal,
    balance_new: Decimal,
    balance_total: Decimal,
    params: AdjustmentParams,
) -> Result<SignedDecimal256, Error> {
    let balance_shift = BalanceShift::new(balance, balance_new)?;
    let adjustment = params
        .zones()
        .iter()
        .map(|zone| zone.compute_adjustment_rate(&balance_shift, params.ideal))
        .sum::<SignedDecimal256>();

    Ok(adjustment * SignedDecimal256::from(balance_total))
}

#[derive(Copy, Clone, Debug)]
pub struct Range {
    start: Bound,
    end: Bound,
}

impl Range {
    pub fn new(start: Bound, end: Bound) -> Result<Self, Error> {
        if start.value() > end.value() {
            return Err(Error::InvalidRange(start.value(), end.value()));
        }

        Ok(Self { start, end })
    }

    /// Returns the overlapping segment between two ranges.
    ///
    /// This function finds the intersection of two ranges, returning a new range
    /// that represents the common segment between them.
    pub fn get_overlap(&self, other: Range) -> Range {
        // Find the intersection of the two ranges
        // segment_start: The later of the two range starts
        // segment_end: The earlier of the two range ends
        let segment_start = if self.start.value() > other.start.value() {
            self.start
        } else if other.start.value() > self.start.value() {
            other.start
        } else {
            // If values are equal, prefer inclusive bound
            match (self.start, other.start) {
                (Bound::Inclusive(v), _) | (_, Bound::Inclusive(v)) => Bound::Inclusive(v),
                _ => Bound::Exclusive(self.start.value()),
            }
        };

        let segment_end = if self.end.value() < other.end.value() {
            self.end
        } else if other.end.value() < self.end.value() {
            other.end
        } else {
            // If values are equal, prefer inclusive bound
            match (self.end, other.end) {
                (Bound::Inclusive(v), _) | (_, Bound::Inclusive(v)) => Bound::Inclusive(v),
                _ => Bound::Exclusive(self.end.value()),
            }
        };

        Range {
            start: segment_start,
            end: segment_end,
        }
    }

    /// Returns true if the range contains the given value
    pub fn contains(&self, value: Decimal) -> bool {
        self.start.contains(value) && self.end.contains(value)
    }
}

/// Represents the direction of a balance change
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BalanceDirection {
    /// Balance is increasing
    Increasing,
    /// Balance is decreasing
    Decreasing,
    /// Balance remains unchanged
    Neutral,
}

/// Represents the impact type of a balance shift on the balance
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BalanceShiftImpactType {
    /// Shift away from ideal
    Debalance,
    /// Shift towards ideal
    Rebalance,
    /// No impact
    Neutral,
}

/// Represents a balance change with its range and direction
#[derive(Debug, Clone)]
pub struct BalanceShift {
    range: Range,
    direction: BalanceDirection,
}

impl BalanceShift {
    /// Creates a new balance shift from an old balance to a new balance
    pub fn new(balance: Decimal, balance_new: Decimal) -> Result<Self, Error> {
        let range = Range::new(
            Bound::Inclusive(balance.min(balance_new)),
            Bound::Inclusive(balance.max(balance_new)),
        )?;

        let direction = if balance < balance_new {
            BalanceDirection::Increasing
        } else if balance > balance_new {
            BalanceDirection::Decreasing
        } else {
            BalanceDirection::Neutral
        };
        Ok(Self { range, direction })
    }

    /// Returns the direction of the balance change
    pub fn direction(&self) -> BalanceDirection {
        self.direction
    }

    /// Returns the range of the balance change
    pub fn range(&self) -> &Range {
        &self.range
    }

    /// Returns the impact type of this balance shift relative to the ideal range
    pub fn get_impact_type(&self, ideal: Range) -> BalanceShiftImpactType {
        if self.direction == BalanceDirection::Neutral {
            return BalanceShiftImpactType::Neutral;
        }

        let is_below_ideal = self.range.end.value() <= ideal.start.value();
        let is_above_ideal = self.range.start.value() >= ideal.end.value();
        let is_ideal_zone = self.range.start.value() == ideal.start.value()
            && self.range.end.value() == ideal.end.value();

        if is_ideal_zone {
            return BalanceShiftImpactType::Neutral;
        }

        match self.direction {
            BalanceDirection::Increasing => {
                if is_below_ideal {
                    BalanceShiftImpactType::Rebalance
                } else if is_above_ideal {
                    BalanceShiftImpactType::Debalance
                } else {
                    BalanceShiftImpactType::Neutral
                }
            }
            BalanceDirection::Decreasing => {
                if is_above_ideal {
                    BalanceShiftImpactType::Rebalance
                } else if is_below_ideal {
                    BalanceShiftImpactType::Debalance
                } else {
                    BalanceShiftImpactType::Neutral
                }
            }
            BalanceDirection::Neutral => BalanceShiftImpactType::Neutral,
        }
    }
}

pub struct AdjustmentParams {
    pub ideal: Range,
    pub critical: Range,
    pub limit: Decimal,
    pub adjustment_rate_strained: Decimal,
    pub adjustment_rate_critical: Decimal,
}

impl AdjustmentParams {
    pub fn zones(&self) -> [Zone; 5] {
        // critical low: [0, critical.start) - highest incentive to move out
        let critical_low = Zone::new(
            Bound::Inclusive(Decimal::zero()),
            Bound::Exclusive(self.critical.start.value()),
            self.adjustment_rate_critical,
        );

        // strained low: [critical.start, ideal.start) - moderate incentive to move up
        let strained_low = Zone::new(
            Bound::Inclusive(self.critical.start.value()),
            Bound::Exclusive(self.ideal.start.value()),
            self.adjustment_rate_strained,
        );

        // ideal zone: [ideal.start, ideal.end] - neutral, no fees or incentives
        let ideal = Zone::new(
            Bound::Inclusive(self.ideal.start.value()),
            Bound::Inclusive(self.ideal.end.value()),
            Decimal::zero(),
        );

        // strained high: (ideal.end, critical.end] - moderate incentive to move down
        let strained_high = Zone::new(
            Bound::Exclusive(self.ideal.end.value()),
            Bound::Inclusive(self.critical.end.value()),
            self.adjustment_rate_strained,
        );

        // critical high: (critical.end, limit] - highest incentive to move out
        let critical_high = Zone::new(
            Bound::Exclusive(self.critical.end.value()),
            Bound::Inclusive(self.limit),
            self.adjustment_rate_critical,
        );

        [
            critical_low,
            strained_low,
            ideal,
            strained_high,
            critical_high,
        ]
    }
}

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
        let overlap = self.range.get_overlap(balance_shift.range().clone());

        if overlap.end.value() <= overlap.start.value() {
            return SignedDecimal256::zero();
        }

        let impact_type = balance_shift.get_impact_type(ideal);
        let segment_length = overlap.end.value() - overlap.start.value();

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
