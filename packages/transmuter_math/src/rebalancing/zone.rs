use crate::rebalancing::{
    balance_shift::{BalanceShift, BalanceShiftImpactType},
    range::{Bound, Range},
};
use cosmwasm_std::{Decimal, SignedDecimal256, StdError, StdResult};
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
            range: Range::new(inclusive_if_zero(start), inclusive_if_zero(end)).unwrap(),
            adjustment_rate,
        }
    }

    /// Compute the adjustment rate for a given balance shift and ideal range accumulated within this zone.
    pub fn compute_adjustment_rate(
        &self,
        balance_shift: &BalanceShift,
        ideal: Range,
    ) -> StdResult<SignedDecimal256> {
        let Some(zoned_balance_shift) = balance_shift.overlap(self.range) else {
            return Ok(SignedDecimal256::zero());
        };

        let impact_type = zoned_balance_shift.get_impact_type(ideal);

        let unsigned_cumulative_adjustment = self
            .adjustment_rate
            .checked_mul(zoned_balance_shift.length())
            .map_err(|_| StdError::generic_err("Overflow in adjustment calculation"))?;

        let result = match impact_type {
            BalanceShiftImpactType::Debalance => {
                SignedDecimal256::from(unsigned_cumulative_adjustment).neg()
            }
            BalanceShiftImpactType::Rebalance => {
                SignedDecimal256::from(unsigned_cumulative_adjustment)
            }
            BalanceShiftImpactType::Neutral => SignedDecimal256::zero(),
        };

        Ok(result)
    }
}

fn inclusive_if_zero(bound: Bound) -> Bound {
    if bound.value() == Decimal::zero() {
        Bound::Inclusive(Decimal::zero())
    } else {
        bound
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use rstest::rstest;

    const DECIMAL_FRACTIONAL: u128 = 1_000_000_000_000_000_000u128;

    #[rstest]
    #[case::no_overlap(
        Zone::new(Bound::Inclusive(Decimal::percent(10)), Bound::Inclusive(Decimal::percent(20)), Decimal::percent(1)),
        BalanceShift::new(Decimal::percent(30), Decimal::percent(40)).unwrap(),
        Range::new(Bound::Inclusive(Decimal::percent(50)), Bound::Inclusive(Decimal::percent(60))).unwrap(),
        SignedDecimal256::zero()
    )]
    #[case::full_overlap_rebalance(
        Zone::new(Bound::Inclusive(Decimal::percent(10)), Bound::Inclusive(Decimal::percent(20)), Decimal::percent(1)),
        BalanceShift::new(Decimal::percent(5), Decimal::percent(15)).unwrap(),
        Range::new(Bound::Inclusive(Decimal::percent(30)), Bound::Inclusive(Decimal::percent(40))).unwrap(),
        SignedDecimal256::from(Decimal::from_ratio(5u128, 10000u128))  // 0.05%
    )]
    #[case::full_overlap_debalance(
        Zone::new(Bound::Inclusive(Decimal::percent(10)), Bound::Inclusive(Decimal::percent(20)), Decimal::percent(1)),
        BalanceShift::new(Decimal::percent(15), Decimal::percent(5)).unwrap(),
        Range::new(Bound::Inclusive(Decimal::percent(30)), Bound::Inclusive(Decimal::percent(40))).unwrap(),
        SignedDecimal256::from(Decimal::from_ratio(5u128, 10000u128)).neg()  // -0.05%
    )]
    #[case::partial_overlap_rebalance(
        Zone::new(Bound::Inclusive(Decimal::percent(10)), Bound::Inclusive(Decimal::percent(20)), Decimal::percent(1)),
        BalanceShift::new(Decimal::percent(5), Decimal::percent(15)).unwrap(),
        Range::new(Bound::Inclusive(Decimal::percent(30)), Bound::Inclusive(Decimal::percent(40))).unwrap(),
        SignedDecimal256::from(Decimal::from_ratio(5u128, 10000u128))  // 0.05%
    )]
    #[case::partial_overlap_debalance(
        Zone::new(Bound::Inclusive(Decimal::percent(10)), Bound::Inclusive(Decimal::percent(20)), Decimal::percent(1)),
        BalanceShift::new(Decimal::percent(15), Decimal::percent(5)).unwrap(),
        Range::new(Bound::Inclusive(Decimal::percent(30)), Bound::Inclusive(Decimal::percent(40))).unwrap(),
        SignedDecimal256::from(Decimal::from_ratio(5u128, 10000u128)).neg()  // -0.05%
    )]
    #[case::neutral_impact(
        Zone::new(Bound::Inclusive(Decimal::percent(10)), Bound::Inclusive(Decimal::percent(20)), Decimal::percent(1)),
        BalanceShift::new(Decimal::percent(15), Decimal::percent(15)).unwrap(),
        Range::new(Bound::Inclusive(Decimal::percent(30)), Bound::Inclusive(Decimal::percent(40))).unwrap(),
        SignedDecimal256::zero()
    )]
    #[case::zero_adjustment_rate(
        Zone::new(Bound::Inclusive(Decimal::percent(10)), Bound::Inclusive(Decimal::percent(20)), Decimal::zero()),
        BalanceShift::new(Decimal::percent(5), Decimal::percent(15)).unwrap(),
        Range::new(Bound::Inclusive(Decimal::percent(30)), Bound::Inclusive(Decimal::percent(40))).unwrap(),
        SignedDecimal256::zero()
    )]
    #[case::exclusive_bounds(
        Zone::new(Bound::Exclusive(Decimal::percent(10)), Bound::Exclusive(Decimal::percent(20)), Decimal::percent(1)),
        BalanceShift::new(Decimal::percent(11), Decimal::percent(19)).unwrap(),
        Range::new(Bound::Inclusive(Decimal::percent(30)), Bound::Inclusive(Decimal::percent(40))).unwrap(),
        SignedDecimal256::from(Decimal::from_ratio(8u128, 10000u128))  // 0.08%
    )]
    #[case::mixed_bounds(
        Zone::new(Bound::Inclusive(Decimal::percent(10)), Bound::Exclusive(Decimal::percent(20)), Decimal::percent(1)),
        BalanceShift::new(Decimal::percent(10), Decimal::percent(19)).unwrap(),
        Range::new(Bound::Inclusive(Decimal::percent(30)), Bound::Inclusive(Decimal::percent(40))).unwrap(),
        SignedDecimal256::from(Decimal::from_ratio(9u128, 10000u128))  // 0.09%
    )]
    #[case::zone_specific_rebalance(
        Zone::new(Bound::Inclusive(Decimal::percent(10)), Bound::Inclusive(Decimal::percent(20)), Decimal::percent(1)),
        BalanceShift::new(Decimal::percent(5), Decimal::percent(25)).unwrap(),  // Crosses multiple zones
        Range::new(Bound::Inclusive(Decimal::percent(30)), Bound::Inclusive(Decimal::percent(40))).unwrap(),
        SignedDecimal256::from(Decimal::from_ratio(10u128, 10000u128))  // 0.1% - only counts the rebalance part in this zone
    )]
    #[case::zone_specific_debalance(
        Zone::new(Bound::Inclusive(Decimal::percent(10)), Bound::Inclusive(Decimal::percent(20)), Decimal::percent(1)),
        BalanceShift::new(Decimal::percent(25), Decimal::percent(5)).unwrap(),  // Crosses multiple zones
        Range::new(Bound::Inclusive(Decimal::percent(30)), Bound::Inclusive(Decimal::percent(40))).unwrap(),
        SignedDecimal256::from(Decimal::from_ratio(10u128, 10000u128)).neg()  // -0.1% - only counts the debalance part in this zone
    )]
    #[case::zone_specific_neutral(
        Zone::new(Bound::Inclusive(Decimal::percent(10)), Bound::Inclusive(Decimal::percent(20)), Decimal::percent(1)),
        BalanceShift::new(Decimal::percent(15), Decimal::percent(25)).unwrap(),  // Crosses multiple zones
        Range::new(Bound::Inclusive(Decimal::percent(15)), Bound::Inclusive(Decimal::percent(25))).unwrap(),
        SignedDecimal256::zero()  // Neutral because this zone overlaps with ideal range
    )]
    #[case::start_from_ideal_rebalance(
        Zone::new(Bound::Inclusive(Decimal::percent(10)), Bound::Inclusive(Decimal::percent(20)), Decimal::percent(1)),
        BalanceShift::new(Decimal::percent(15), Decimal::percent(5)).unwrap(),  // Starts from ideal (15%) and moves down
        Range::new(Bound::Inclusive(Decimal::percent(15)), Bound::Inclusive(Decimal::percent(25))).unwrap(),
        SignedDecimal256::from(Decimal::from_ratio(5u128, 10000u128)).neg()  // -0.05% - debalance because moving away from ideal
    )]
    #[case::start_from_ideal_debalance(
        Zone::new(Bound::Inclusive(Decimal::percent(10)), Bound::Inclusive(Decimal::percent(20)), Decimal::percent(1)),
        BalanceShift::new(Decimal::percent(15), Decimal::percent(25)).unwrap(),  // Starts from ideal (15%) and moves up
        Range::new(Bound::Inclusive(Decimal::percent(5)), Bound::Inclusive(Decimal::percent(15))).unwrap(),
        SignedDecimal256::from(Decimal::from_ratio(5u128, 10000u128)).neg()  // -0.05% - debalance because moving away from ideal
    )]
    #[case::start_from_ideal_neutral(
        Zone::new(Bound::Inclusive(Decimal::percent(10)), Bound::Inclusive(Decimal::percent(20)), Decimal::percent(1)),
        BalanceShift::new(Decimal::percent(15), Decimal::percent(17)).unwrap(),  // Starts from ideal (15%) and moves within ideal
        Range::new(Bound::Inclusive(Decimal::percent(15)), Bound::Inclusive(Decimal::percent(25))).unwrap(),
        SignedDecimal256::zero()  // Neutral because moving within ideal range
    )]
    fn test_compute_adjustment_rate(
        #[case] zone: Zone,
        #[case] balance_shift: BalanceShift,
        #[case] ideal: Range,
        #[case] expected: SignedDecimal256,
    ) {
        assert_eq!(
            zone.compute_adjustment_rate(&balance_shift, ideal),
            Ok(expected)
        );
    }

    proptest! {
        #[test]
        fn test_adjustment_rate_properties(
            zone_start in any::<u128>().prop_map(|x| Decimal::from_ratio(x % DECIMAL_FRACTIONAL, DECIMAL_FRACTIONAL)),
            zone_end in any::<u128>().prop_map(|x| Decimal::from_ratio(x % DECIMAL_FRACTIONAL, DECIMAL_FRACTIONAL)),
            adjustment_rate in any::<u128>().prop_map(|x| Decimal::from_ratio(x % DECIMAL_FRACTIONAL, DECIMAL_FRACTIONAL)),
            shift_start in any::<u128>().prop_map(|x| Decimal::from_ratio(x % DECIMAL_FRACTIONAL, DECIMAL_FRACTIONAL)),
            shift_end in any::<u128>().prop_map(|x| Decimal::from_ratio(x % DECIMAL_FRACTIONAL, DECIMAL_FRACTIONAL)),
            ideal_start in any::<u128>().prop_map(|x| Decimal::from_ratio(x % DECIMAL_FRACTIONAL, DECIMAL_FRACTIONAL)),
            ideal_end in any::<u128>().prop_map(|x| Decimal::from_ratio(x % DECIMAL_FRACTIONAL, DECIMAL_FRACTIONAL)),
        ) {
            // Skip invalid ranges
            if zone_start >= zone_end || shift_start >= shift_end || ideal_start >= ideal_end {
                return Ok(());
            }

            let zone = Zone::new(
                Bound::Inclusive(zone_start),
                Bound::Inclusive(zone_end),
                adjustment_rate
            );
            let balance_shift = BalanceShift::new(shift_start, shift_end).unwrap();
            let ideal = Range::new(
                Bound::Inclusive(ideal_start),
                Bound::Inclusive(ideal_end)
            ).unwrap();

            let adjustment = zone.compute_adjustment_rate(&balance_shift, ideal).unwrap();

            // Property 1: Zero adjustment rate always results in zero adjustment
            if adjustment_rate.is_zero() {
                prop_assert_eq!(adjustment, SignedDecimal256::zero());
            }

            // Property 2: No overlap always results in zero adjustment
            if zone_end <= shift_start || zone_start >= shift_end {
                prop_assert_eq!(adjustment, SignedDecimal256::zero());
            }

            // Property 3: Neutral impact type always results in zero adjustment
            if balance_shift.get_impact_type(ideal) == BalanceShiftImpactType::Neutral {
                prop_assert_eq!(adjustment, SignedDecimal256::zero());
            }

            // Property 4: Debalance and Rebalance are opposites
            if balance_shift.get_impact_type(ideal) == BalanceShiftImpactType::Debalance
            || balance_shift.get_impact_type(ideal) == BalanceShiftImpactType::Rebalance {
                let rebalance_shift = BalanceShift::new(shift_end, shift_start).unwrap();
                let rebalance_adjustment = zone.compute_adjustment_rate(&rebalance_shift, ideal).unwrap();
                prop_assert_eq!(adjustment, rebalance_adjustment.neg());
            }
        }
    }
}
