use crate::rebalancing::range::{Bound, Range};
use crate::TransmuterMathError as Error;
use cosmwasm_std::Decimal;

/// Represents the direction of a balance change
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BalanceShiftDirection {
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
    direction: BalanceShiftDirection,
}

impl BalanceShift {
    /// Creates a new balance shift from an old balance to a new balance
    pub fn new(balance: Decimal, balance_new: Decimal) -> Result<Self, Error> {
        let range = Range::new(
            Bound::Inclusive(balance.min(balance_new)),
            Bound::Inclusive(balance.max(balance_new)),
        )?;

        let direction = if balance < balance_new {
            BalanceShiftDirection::Increasing
        } else if balance > balance_new {
            BalanceShiftDirection::Decreasing
        } else {
            BalanceShiftDirection::Neutral
        };
        Ok(Self { range, direction })
    }

    /// Returns the direction of the balance change
    pub fn direction(&self) -> BalanceShiftDirection {
        self.direction
    }

    /// Returns the range of the balance change
    pub fn range(&self) -> &Range {
        &self.range
    }

    /// Returns the impact type of this balance shift relative to the ideal range
    pub fn get_impact_type(&self, ideal: Range) -> BalanceShiftImpactType {
        if self.direction == BalanceShiftDirection::Neutral {
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
            BalanceShiftDirection::Increasing => {
                if is_below_ideal {
                    BalanceShiftImpactType::Rebalance
                } else if is_above_ideal {
                    BalanceShiftImpactType::Debalance
                } else {
                    BalanceShiftImpactType::Neutral
                }
            }
            BalanceShiftDirection::Decreasing => {
                if is_above_ideal {
                    BalanceShiftImpactType::Rebalance
                } else if is_below_ideal {
                    BalanceShiftImpactType::Debalance
                } else {
                    BalanceShiftImpactType::Neutral
                }
            }
            BalanceShiftDirection::Neutral => BalanceShiftImpactType::Neutral,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use rstest::rstest;

    const DECIMAL_FRACTIONAL: u128 = 1_000_000_000_000_000_000u128;

    #[rstest]
    #[case::increasing_balance(
        Decimal::from_ratio(1u128, 1u128),
        Decimal::from_ratio(2u128, 1u128),
        BalanceShiftDirection::Increasing
    )]
    #[case::decreasing_balance(
        Decimal::from_ratio(2u128, 1u128),
        Decimal::from_ratio(1u128, 1u128),
        BalanceShiftDirection::Decreasing
    )]
    #[case::neutral_balance(
        Decimal::from_ratio(1u128, 1u128),
        Decimal::from_ratio(1u128, 1u128),
        BalanceShiftDirection::Neutral
    )]
    #[case::zero_to_one(
        Decimal::zero(),
        Decimal::from_ratio(1u128, 1u128),
        BalanceShiftDirection::Increasing
    )]
    #[case::small_numbers(
        Decimal::from_ratio(1u128, DECIMAL_FRACTIONAL),
        Decimal::from_ratio(2u128, DECIMAL_FRACTIONAL),
        BalanceShiftDirection::Increasing
    )]
    #[case::large_numbers(
        Decimal::from_ratio((u128::MAX / DECIMAL_FRACTIONAL) - 1, 1u128),
        Decimal::from_ratio(u128::MAX / DECIMAL_FRACTIONAL, 1u128),
        BalanceShiftDirection::Increasing
    )]
    fn test_balance_shift_new(
        #[case] balance: Decimal,
        #[case] balance_new: Decimal,
        #[case] expected_direction: BalanceShiftDirection,
    ) {
        let shift = BalanceShift::new(balance, balance_new).unwrap();

        // Test direction
        assert_eq!(shift.direction(), expected_direction);

        // Test range bounds
        let range = shift.range();
        assert_eq!(range.start().value(), balance.min(balance_new));
        assert_eq!(range.end().value(), balance.max(balance_new));
    }

    #[rstest]
    #[case::neutral_when_no_change(
        Decimal::from_ratio(1u128, 1u128),
        Decimal::from_ratio(1u128, 1u128),
        Range::new(Bound::Inclusive(Decimal::from_ratio(1u128, 1u128)), Bound::Inclusive(Decimal::from_ratio(1u128, 1u128))).unwrap(),
        BalanceShiftImpactType::Neutral
    )]
    #[case::rebalance_when_increasing_below_ideal(
        Decimal::from_ratio(1u128, 1u128),
        Decimal::from_ratio(2u128, 1u128),
        Range::new(Bound::Inclusive(Decimal::from_ratio(3u128, 1u128)), Bound::Inclusive(Decimal::from_ratio(4u128, 1u128))).unwrap(),
        BalanceShiftImpactType::Rebalance
    )]
    #[case::debalance_when_increasing_above_ideal(
        Decimal::from_ratio(3u128, 1u128),
        Decimal::from_ratio(4u128, 1u128),
        Range::new(Bound::Inclusive(Decimal::from_ratio(1u128, 1u128)), Bound::Inclusive(Decimal::from_ratio(2u128, 1u128))).unwrap(),
        BalanceShiftImpactType::Debalance
    )]
    #[case::rebalance_when_decreasing_above_ideal(
        Decimal::from_ratio(4u128, 1u128),
        Decimal::from_ratio(3u128, 1u128),
        Range::new(Bound::Inclusive(Decimal::from_ratio(1u128, 1u128)), Bound::Inclusive(Decimal::from_ratio(2u128, 1u128))).unwrap(),
        BalanceShiftImpactType::Rebalance
    )]
    #[case::debalance_when_decreasing_below_ideal(
        Decimal::from_ratio(2u128, 1u128),
        Decimal::from_ratio(1u128, 1u128),
        Range::new(Bound::Inclusive(Decimal::from_ratio(3u128, 1u128)), Bound::Inclusive(Decimal::from_ratio(4u128, 1u128))).unwrap(),
        BalanceShiftImpactType::Debalance
    )]
    #[case::neutral_when_moving_within_ideal(
        Decimal::from_ratio(2u128, 1u128),
        Decimal::from_ratio(3u128, 1u128),
        Range::new(Bound::Inclusive(Decimal::from_ratio(1u128, 1u128)), Bound::Inclusive(Decimal::from_ratio(4u128, 1u128))).unwrap(),
        BalanceShiftImpactType::Neutral
    )]
    fn test_get_impact_type(
        #[case] balance: Decimal,
        #[case] balance_new: Decimal,
        #[case] ideal: Range,
        #[case] expected: BalanceShiftImpactType,
    ) {
        let shift = BalanceShift::new(balance, balance_new).unwrap();
        assert_eq!(shift.get_impact_type(ideal), expected);
    }

    proptest! {
        #[test]
        fn test_get_impact_type_properties(
            balance in any::<u128>().prop_map(|x| Decimal::from_ratio(x / DECIMAL_FRACTIONAL, 1u128)),
            balance_new in any::<u128>().prop_map(|x| Decimal::from_ratio(x / DECIMAL_FRACTIONAL, 1u128)),
            ideal_start in any::<u128>().prop_map(|x| Decimal::from_ratio(x / DECIMAL_FRACTIONAL, 1u128)),
            ideal_end in any::<u128>().prop_map(|x| Decimal::from_ratio(x / DECIMAL_FRACTIONAL, 1u128)),
        ) {
            // Skip invalid ranges
            if ideal_start >= ideal_end {
                return Ok(());
            }

            let ideal = Range::new(Bound::Inclusive(ideal_start), Bound::Inclusive(ideal_end)).unwrap();
            let shift = BalanceShift::new(balance, balance_new).unwrap();
            let impact = shift.get_impact_type(ideal);

            // Property 1: Neutral direction always results in Neutral impact
            if shift.direction() == BalanceShiftDirection::Neutral {
                prop_assert_eq!(impact, BalanceShiftImpactType::Neutral);
            }

            // Property 2: If range is exactly equal to ideal, impact is Neutral
            if shift.range().start().value() == ideal.start().value()
                && shift.range().end().value() == ideal.end().value() {
                prop_assert_eq!(impact, BalanceShiftImpactType::Neutral);
            }

            // Property 3: If range is completely below ideal and increasing, it's Rebalance
            if shift.range().end().value() <= ideal.start().value()
                && shift.direction() == BalanceShiftDirection::Increasing {
                prop_assert_eq!(impact, BalanceShiftImpactType::Rebalance);
            }

            // Property 4: If range is completely above ideal and decreasing, it's Rebalance
            if shift.range().start().value() >= ideal.end().value()
                && shift.direction() == BalanceShiftDirection::Decreasing {
                prop_assert_eq!(impact, BalanceShiftImpactType::Rebalance);
            }
        }
    }
}
