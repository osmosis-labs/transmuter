pub mod adjustment_params;
pub mod balance_shift;
pub mod range;
pub mod zone;

use crate::TransmuterMathError as Error;
use adjustment_params::AdjustmentParams;
use balance_shift::BalanceShift;
use cosmwasm_std::{Decimal, Int256, SignedDecimal256, StdError, StdResult, Uint128};

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
    params: AdjustmentParams,
) -> Result<Int256, Error> {
    let balance_shift = BalanceShift::new(balance, balance_new)?;
    let ideal = params.ideal().clone();

    let adjustment = params
        .zones()
        .iter()
        .map(|zone| zone.compute_adjustment_rate(&balance_shift, ideal))
        .collect::<StdResult<Vec<SignedDecimal256>>>()?
        .iter()
        .fold(Ok(SignedDecimal256::zero()), |acc, x| {
            acc.and_then(|sum| {
                sum.checked_add(*x)
                    .map_err(|_| StdError::generic_err("Overflow in adjustment sum"))
            })
        })?;

    // Calculate the adjustment value
    let adjustment_value = adjustment.checked_mul(SignedDecimal256::from_atomics(
        Int256::from(balance_total),
        0,
    )?)?;

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
}
