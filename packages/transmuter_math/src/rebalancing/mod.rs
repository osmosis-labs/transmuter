pub mod balance_shift;
pub mod params;
pub mod range;
pub mod zone;

use crate::TransmuterMathError as Error;
use balance_shift::BalanceShift;
use cosmwasm_std::{Decimal, Int256, SignedDecimal256, StdError, StdResult, Uint128};
use params::RebalancingParams;

const DECIMAL_FRACTIONAL: Int256 = Int256::from_i128(1_000_000_000_000_000_000);

/// Compute fee or incentive adjustment for a single asset's balance movement.
///
/// This function calculates the incentive (if positive) or fee (if negative)
/// for a swap that moves an asset's balance from balance to balance_new. The goal is to
/// encourage movements toward the ideal balance range [ideal.start, ideal.end] and
/// discourage movements away from it.
pub fn compute_adjustment_value(
    balance: Decimal,
    balance_new: Decimal,
    balance_total: Uint128,
    params: RebalancingParams,
) -> Result<Int256, Error> {
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

    // Calculate the adjustment value
    let adjustment_value = total_effective_adjustment_rate.checked_mul(
        SignedDecimal256::from_atomics(Int256::from(balance_total), 0)?,
    )?;

    round_adjustment(adjustment_value)
}

/// Round a SignedDecimal256 to Int256 with appropriate rounding behavior:
/// - For positive values: round down (give less incentive)
/// - For negative values: round up (take more fee)
fn round_adjustment(adjustment: SignedDecimal256) -> Result<Int256, Error> {
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
    use params::RebalancingParams;
    use rstest::rstest;

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
        Uint128::new(1000)
    )]
    #[case::extreme_imbalance(
        vec![
            (Decimal::zero(), Decimal::zero()),
            (Decimal::zero(), Decimal::zero()),
            (Decimal::percent(100), Decimal::percent(100)),
        ],
        Uint128::new(1000)
    )]
    #[case::moving_to_balance(
        vec![
            (Decimal::percent(10), Decimal::percent(33)),
            (Decimal::percent(10), Decimal::percent(33)),
            (Decimal::percent(80), Decimal::percent(34)),
        ],
        Uint128::new(1000)
    )]
    fn test_compute_adjustment_value_extreme_cases_properties(
        #[case] balances: Vec<(Decimal, Decimal)>,
        #[case] balance_total: Uint128,
    ) {
        // Create extreme adjustment parameters with 100% rate
        let params = RebalancingParams::new(
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
        let adjustments: Vec<Int256> = balances
            .iter()
            .map(|(balance, balance_new)| {
                compute_adjustment_value(*balance, *balance_new, balance_total, params.clone())
                    .unwrap()
            })
            .collect();

        // Verify adjustments are within bounds
        for adj in &adjustments {
            assert!(adj.abs() <= Int256::from(balance_total));
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
        Uint128::new(1000),   // balance_total
        Int256::zero()        // expected_adjustment
    )]
    #[case::moving_into_ideal_range(
        Decimal::percent(10),  // balance
        Decimal::percent(33),  // balance_new
        Uint128::new(1000),   // balance_total
        Int256::from(11)      // expected_adjustment (positive, rounded down)
    )]
    #[case::moving_out_of_ideal_range(
        Decimal::percent(33),  // balance
        Decimal::percent(10),  // balance_new
        Uint128::new(1000),   // balance_total
        Int256::from(-11)     // expected_adjustment (negative, rounded up)
    )]
    #[case::small_movement_into_ideal(
        Decimal::percent(20),  // balance
        Decimal::percent(25),  // balance_new
        Uint128::new(1000),   // balance_total
        Int256::from(0)       // expected_adjustment (positive, rounded down)
    )]
    #[case::small_movement_out_of_ideal(
        Decimal::percent(25),  // balance
        Decimal::percent(20),  // balance_new
        Uint128::new(1000),   // balance_total
        Int256::from(-1)      // expected_adjustment (negative, rounded up)
    )]
    #[case::crossing_all_zones_into_ideal(
        Decimal::percent(5),   // balance (below critical lower)
        Decimal::percent(50),  // balance_new (into ideal range)
        Uint128::new(1000),   // balance_total
        Int256::from(16)     // expected_adjustment (positive, combines critical and strained rates)
    )]
    #[case::crossing_all_zones_out_of_ideal(
        Decimal::percent(50),  // balance (in ideal range)
        Decimal::percent(5),   // balance_new (below critical lower)
        Uint128::new(1000),   // balance_total
        Int256::from(-16)     // expected_adjustment (negative, combines critical and strained rates)
    )]
    #[case::crossing_critical_to_strained(
        Decimal::percent(5),   // balance (below critical lower)
        Decimal::percent(25),  // balance_new (into strained range)
        Uint128::new(1000),   // balance_total
        Int256::from(15)      // expected_adjustment (positive, critical rate)
    )]
    #[case::crossing_strained_to_critical(
        Decimal::percent(25),  // balance (in strained range)
        Decimal::percent(5),   // balance_new (below critical lower)
        Uint128::new(1000),   // balance_total
        Int256::from(-16)     // expected_adjustment (negative, critical rate)
    )]
    fn test_compute_adjustment_value(
        #[case] balance: Decimal,
        #[case] balance_new: Decimal,
        #[case] balance_total: Uint128,
        #[case] expected_adjustment: Int256,
    ) {
        let params = RebalancingParams::new(
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
            compute_adjustment_value(balance, balance_new, balance_total, params).unwrap();

        assert_eq!(adjustment, expected_adjustment);
    }
}
