use crate::rebalancing::range::{Bound, Range};
use crate::TransmuterMathError as Error;
use cosmwasm_std::Decimal;

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

        let is_below_ideal = self.range.end().value() <= ideal.start().value();
        let is_above_ideal = self.range.start().value() >= ideal.end().value();
        let is_ideal_zone = self.range.start().value() == ideal.start().value()
            && self.range.end().value() == ideal.end().value();

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
