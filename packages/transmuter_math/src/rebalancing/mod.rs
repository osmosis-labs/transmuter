pub mod balance_shift;
pub mod config;
pub mod range;
pub mod zone;

use crate::TransmuterMathError as Error;
use balance_shift::BalanceShift;
use config::RebalancingConfig;
use cosmwasm_std::{Decimal, Int256, SignedDecimal256, StdError, StdResult};

const DECIMAL_FRACTIONAL: Int256 = Int256::from_i128(1_000_000_000_000_000_000);

/// Compute fee or incentive adjustment for a single asset's balance movement.
///
/// This function calculates the incentive (if positive) or fee (if negative) rate
/// for a swap that moves an asset's balance from balance to balance_new. The goal is to
/// encourage movements toward the ideal balance range [ideal.start, ideal.end] and
/// discourage movements away from it.
///
/// The total effective adjustment rate is the sum of the effective adjustment rates for each zone.
/// The effective adjustment rate for a zone is the product of the zone's adjustment rate and the
/// zone's weight. The weight is the normalized balance of the asset in the zone.
pub fn compute_total_effective_adjustment_rate(
    balance: Decimal,
    balance_new: Decimal,
    params: RebalancingConfig,
) -> Result<SignedDecimal256, Error> {
    let balance_shift = BalanceShift::new(balance, balance_new)?;
    let ideal = params.ideal().clone();

    let total_effective_adjustment_rate = params
        .zones()
        .iter()
        .map(|zone| zone.compute_effective_adjustment_rate(&balance_shift, ideal))
        .collect::<StdResult<Vec<SignedDecimal256>>>()?
        .iter()
        .fold(Ok(SignedDecimal256::zero()), |acc, x| {
            acc.and_then(|sum| {
                sum.checked_add(*x)
                    .map_err(|_| StdError::generic_err("Overflow in adjustment sum"))
            })
        })?;

    Ok(total_effective_adjustment_rate)
}

/// Round a SignedDecimal256 to Int256 with appropriate rounding behavior:
/// - For positive values: round down (give less incentive)
/// - For negative values: round up (take more fee)
pub fn round_adjustment(adjustment: SignedDecimal256) -> Result<Int256, Error> {
    if adjustment > SignedDecimal256::zero() {
        // For positive adjustments (incentives), round down to give less
        Ok(adjustment.atomics().checked_div(DECIMAL_FRACTIONAL)?)
    } else {
        // For negative adjustments (fees), round up to take more
        let atomics = adjustment.atomics();
        let truncated = atomics.checked_div(DECIMAL_FRACTIONAL)?;
        let truncated_with_zeros = truncated.checked_mul(DECIMAL_FRACTIONAL)?;

        // If there is a remainder, because this is a negative value, truncated will be greater than the actual value.
        // So we need to subtract 1 to get the correct value.
        if truncated_with_zeros > atomics {
            Ok(truncated.checked_sub(Int256::from(1))?)
        } else {
            Ok(truncated)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::RebalancingConfig;
    use cosmwasm_std::Decimal256;
    use rstest::rstest;
    use std::str::FromStr;

    #[rstest]
    #[case(SignedDecimal256::percent(100), Int256::from(1))] // 1.0 -> 1
    #[case(SignedDecimal256::percent(150), Int256::from(1))] // 1.5 -> 1
    #[case(SignedDecimal256::percent(199), Int256::from(1))] // 1.99 -> 1
    #[case(SignedDecimal256::percent(200), Int256::from(2))] // 2.0 -> 2
    #[case(SignedDecimal256::percent(-100), Int256::from(-1))] // -1.0 -> -1
    #[case(SignedDecimal256::percent(-150), Int256::from(-2))] // -1.5 -> -2
    #[case(SignedDecimal256::percent(-199), Int256::from(-2))] // -1.99 -> -2
    #[case(SignedDecimal256::percent(-200), Int256::from(-2))] // -2.0 -> -2
    #[case(SignedDecimal256::zero(), Int256::zero())] // 0.0 -> 0
    fn test_round_adjustment(#[case] input: SignedDecimal256, #[case] expected: Int256) {
        assert_eq!(round_adjustment(input).unwrap(), expected);
    }

    #[rstest]
    #[case::balanced_state(
        vec![
            (Decimal::percent(33), Decimal::percent(33)),
            (Decimal::percent(33), Decimal::percent(33)),
            (Decimal::percent(34), Decimal::percent(34)),
        ],
    )]
    #[case::extreme_imbalance(
        vec![
            (Decimal::zero(), Decimal::zero()),
            (Decimal::zero(), Decimal::zero()),
            (Decimal::percent(100), Decimal::percent(100)),
        ],
    )]
    #[case::moving_to_balance(
        vec![
            (Decimal::percent(10), Decimal::percent(33)),
            (Decimal::percent(10), Decimal::percent(33)),
            (Decimal::percent(80), Decimal::percent(34)),
        ],
    )]
    fn test_compute_adjustment_value_extreme_cases_properties(
        #[case] balances: Vec<(Decimal, Decimal)>,
    ) {
        // Create extreme adjustment parameters with 100% rate
        let params = RebalancingConfig::new(
            Decimal::percent(70),  // ideal_upper
            Decimal::percent(30),  // ideal_lower
            Decimal::percent(80),  // critical_upper
            Decimal::percent(20),  // critical_lower
            Decimal::percent(100), // limit
            Decimal::percent(100), // adjustment_rate_strained
            Decimal::percent(100), // adjustment_rate_critical
        )
        .unwrap();

        // Calculate adjustments for each asset
        let adjustments: Vec<SignedDecimal256> = balances
            .iter()
            .map(|(balance, balance_new)| {
                compute_total_effective_adjustment_rate(*balance, *balance_new, params.clone())
                    .unwrap()
            })
            .collect();

        // Verify adjustments are within bounds
        for adj in &adjustments {
            assert!(adj.abs_diff(SignedDecimal256::zero()) <= Decimal256::one());
        }

        // Verify sum of balances is 100%
        let sum_old: Decimal = balances.iter().map(|(b, _)| *b).sum();
        let sum_new: Decimal = balances.iter().map(|(_, b)| *b).sum();
        assert_eq!(sum_old, Decimal::percent(100));
        assert_eq!(sum_new, Decimal::percent(100));
    }

    #[rstest]
    #[case::no_movement(
        Decimal::percent(50),  // balance
        Decimal::percent(50),  // balance_new
        SignedDecimal256::zero()        // expected_adjustment
    )]
    #[case::moving_into_ideal_range(
        Decimal::percent(10),  // balance
        Decimal::percent(33),  // balance_new
        SignedDecimal256::from_str("0.011").unwrap()      // expected_adjustment (11/1000 = 0.011)
    )]
    #[case::moving_out_of_ideal_range(
        Decimal::percent(33),  // balance
        Decimal::percent(10),  // balance_new
        SignedDecimal256::from_str("-0.011").unwrap()     // expected_adjustment (-11/1000 = -0.011)
    )]
    #[case::small_movement_into_ideal(
        Decimal::percent(20),  // balance
        Decimal::percent(25),  // balance_new
        SignedDecimal256::from_str("0.0005").unwrap()       // expected_adjustment (0.5/1000 = 0.0005)
    )]
    #[case::small_movement_out_of_ideal(
        Decimal::percent(25),  // balance
        Decimal::percent(20),  // balance_new
        SignedDecimal256::from_str("-0.0005").unwrap()      // expected_adjustment (-0.5/1000 = -0.0005)
    )]
    #[case::crossing_all_zones_into_ideal(
        Decimal::percent(5),   // balance (below critical lower)
        Decimal::percent(50),  // balance_new (into ideal range)
        SignedDecimal256::from_str("0.016").unwrap()     // expected_adjustment (16/1000 = 0.016)
    )]
    #[case::crossing_all_zones_out_of_ideal(
        Decimal::percent(50),  // balance (in ideal range)
        Decimal::percent(5),   // balance_new (below critical lower)
        SignedDecimal256::from_str("-0.016").unwrap()     // expected_adjustment (-16/1000 = -0.016)
    )]
    #[case::crossing_critical_to_strained(
        Decimal::percent(5),   // balance (below critical lower)
        Decimal::percent(25),  // balance_new (into strained range)
        SignedDecimal256::from_str("0.0155").unwrap()      // expected_adjustment (15.5/1000 = 0.0155)
    )]
    #[case::crossing_strained_to_critical(
        Decimal::percent(25),  // balance (in strained range)
        Decimal::percent(5),   // balance_new (below critical lower)
        SignedDecimal256::from_str("-0.0155").unwrap()     // expected_adjustment (-15.5/1000 = -0.0155)
    )]
    fn test_compute_adjustment_value(
        #[case] balance: Decimal,
        #[case] balance_new: Decimal,
        #[case] expected_adjustment: SignedDecimal256,
    ) {
        let params = RebalancingConfig::new(
            Decimal::percent(70),  // ideal_upper
            Decimal::percent(30),  // ideal_lower
            Decimal::percent(80),  // critical_upper
            Decimal::percent(20),  // critical_lower
            Decimal::percent(100), // limit
            Decimal::percent(1),   // adjustment_rate_strained
            Decimal::percent(10),  // adjustment_rate_critical
        )
        .unwrap();

        let adjustment =
            compute_total_effective_adjustment_rate(balance, balance_new, params).unwrap();

        assert_eq!(adjustment, expected_adjustment);
    }
}
