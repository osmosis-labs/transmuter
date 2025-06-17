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

    // Convert to Uint128 with appropriate rounding
    if adjustment_value > SignedDecimal256::zero() {
        // For positive adjustments (incentives), round down to give less
        Ok(adjustment_value.atomics().checked_div(DECIMAL_FRACTIONAL)?)
    } else {
        // For negative adjustments (fees), round up to take more
        let atomics = adjustment_value.atomics();
        let truncated = atomics
            .checked_div(DECIMAL_FRACTIONAL)?
            .checked_mul(DECIMAL_FRACTIONAL)?;

        // If there is a remainder, because this is a negative value, truncated will be greater than the actual value.
        // So we need to subtract 1 to get the correct value.
        if truncated > atomics {
            Ok(truncated.checked_sub(Int256::from(1))?)
        } else {
            Ok(truncated)
        }
    }
}
