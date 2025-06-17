pub mod adjustment_params;
pub mod balance_shift;
pub mod range;
pub mod zone;

use crate::TransmuterMathError as Error;
use adjustment_params::AdjustmentParams;
use balance_shift::BalanceShift;
use cosmwasm_std::{Decimal, SignedDecimal256, StdError, StdResult};

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

    Ok(adjustment.checked_mul(SignedDecimal256::from(balance_total))?)
}
