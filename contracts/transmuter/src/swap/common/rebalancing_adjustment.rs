use std::cmp::Ordering;

use cosmwasm_std::{coin, ensure, Coin, Int256, Uint128};

use crate::{
    asset::convert_amount,
    swap::common::{Adjustment, ContractError},
    transmuter_pool::TransmuterPool,
};

pub fn rebalancing_adjustment_for_exact_out(
    token_in_max_amount: Uint128,
    std_norm_factor: Uint128,
    token_in_norm_factor: Uint128,
) -> Box<dyn FnOnce(TransmuterPool, Coin, Int256) -> Result<(Coin, Adjustment), ContractError>> {
    Box::new(
        move |pool: TransmuterPool, token_in: Coin, total_adjustment_value: Int256| {
            // If adjustment value is negative, fee take from the token_in, so we require addtional token_in // to pay for the fee.
            // Otherwise, return the token_in as is
            let (token_in, adjustment) = adjust_exact_out(
                token_in,
                token_in_norm_factor,
                std_norm_factor,
                total_adjustment_value,
            )?;

            let token_in_amount = token_in.amount.clone();

            ensure!(
                token_in_amount <= token_in_max_amount,
                ContractError::ExcessiveRequiredTokenIn {
                    limit: token_in_max_amount,
                    required: token_in_amount,
                }
            );

            Ok((token_in, adjustment))
        },
    )
}

pub fn rebalancing_adjustment_for_exact_in(
    token_out_min_amount: Uint128,
    std_norm_factor: Uint128,
    token_out_norm_factor: Uint128,
) -> Box<dyn FnOnce(TransmuterPool, Coin, Int256) -> Result<(Coin, Adjustment), ContractError>> {
    Box::new(
        move |_pool: TransmuterPool, token_out: Coin, total_adjustment_value: Int256| {
            let (token_out, adjustment) = adjust_exact_in(
                token_out,
                token_out_norm_factor,
                std_norm_factor,
                total_adjustment_value,
            )?;

            ensure!(
                token_out.amount >= token_out_min_amount,
                ContractError::InsufficientTokenOut {
                    min_required: token_out_min_amount,
                    amount_out: token_out.amount,
                }
            );

            Ok((token_out, adjustment))
        },
    )
}

/// Adjust token out for exact in.
///
/// If adjustment value is negative, fee take from the token_out, so we require additional token_out to pay for the fee.
/// If adjustment value is positive, incentive is credited to the beneficiary, return the token_out as is.
/// If adjustment value is zero, no adjustment is made, return the token_out as is.
fn adjust_exact_in(
    token_out: Coin,
    token_out_norm_factor: Uint128,
    std_norm_factor: Uint128,
    total_adjustment_value: Int256,
) -> Result<(Coin, Adjustment), ContractError> {
    match total_adjustment_value.cmp(&Int256::zero()) {
        // negative adjustment value means fee deduction from token_out
        Ordering::Less => deduct_fee_from_token_out(
            &token_out,
            std_norm_factor,
            token_out_norm_factor,
            total_adjustment_value,
        ),
        // positive adjustment value means incentive credit to the beneficiary
        Ordering::Greater => credit_incentive(token_out, total_adjustment_value),
        // zero adjustment means no adjustment
        Ordering::Equal => Ok((token_out, Adjustment::None)),
    }
}

/// Adjust token in for exact out.
///
/// If adjustment value is negative, fee take from the token_in, so we require additional token_in to pay for the fee.
/// If adjustment value is positive, incentive is credited to the beneficiary, return the token_in as is.
/// If adjustment value is zero, no adjustment is made, return the token_in as is.
fn adjust_exact_out(
    token_in: Coin,
    token_in_norm_factor: Uint128,
    std_norm_factor: Uint128,
    total_adjustment_value: Int256,
) -> Result<(Coin, Adjustment), ContractError> {
    Ok(match total_adjustment_value.cmp(&Int256::zero()) {
        // negative adjustment value means fee deduction from token_in
        Ordering::Less => increase_and_deduct_fee_from_token_in(
            &token_in,
            token_in_norm_factor,
            std_norm_factor,
            total_adjustment_value,
        )?,
        // positive adjustment value means incentive credit to the beneficiary
        Ordering::Greater => credit_incentive(token_in, total_adjustment_value)?,
        // zero adjustment means no adjustment
        Ordering::Equal => (token_in, Adjustment::None),
    })
}

/// Deduct fee from the token out, used when adjustment is negative and token in needs to be exact.
fn deduct_fee_from_token_out(
    token_out: &Coin,
    std_norm_factor: Uint128,
    token_out_norm_factor: Uint128,
    total_adjustment_value: Int256,
) -> Result<(Coin, Adjustment), ContractError> {
    // Adjustment value has normalization factor of standard normalization factor. It needs conversion to token out normalization factor
    // to get the fee amount in token out.
    let fee_amount = convert_amount(
        total_adjustment_value.abs().try_into()?,
        std_norm_factor,
        token_out_norm_factor,
        // rounding up means slightly more fee than required, keeps the incentive pool healthy
        &crate::asset::Rounding::Up,
    )?;

    let token_out_amount: Uint128 = token_out.amount.checked_sub(fee_amount)?;
    let token_out = coin(token_out_amount.u128(), token_out.denom.clone());
    let token_out_denom = token_out.denom.clone();

    Ok((
        token_out,
        Adjustment::DeductFee {
            fee: coin(fee_amount.u128(), token_out_denom),
        },
    ))
}

/// Increase the token in amount and deduct fee from the additional token in, used when adjustment is positive and token out needs to be exact.
fn increase_and_deduct_fee_from_token_in(
    token_in: &Coin,
    token_in_norm_factor: Uint128,
    std_norm_factor: Uint128,
    total_adjustment_value: Int256,
) -> Result<(Coin, Adjustment), ContractError> {
    let fee = convert_amount(
        total_adjustment_value.abs().try_into()?,
        std_norm_factor,
        token_in_norm_factor,
        // rounding up means slightly more fee than required, keeps the incentive pool healthy
        &crate::asset::Rounding::Up,
    )?;

    let token_in_amount = token_in.amount.checked_add(fee)?;

    Ok((
        coin(token_in_amount.u128(), token_in.denom.clone()),
        Adjustment::DeductFee {
            fee: coin(fee.u128(), token_in.denom.clone()),
        },
    ))
}
fn credit_incentive(
    token_out: Coin,
    total_adjustment_value: Int256,
) -> Result<(Coin, Adjustment), ContractError> {
    Ok((
        token_out,
        Adjustment::CreditIncentive {
            incentive: total_adjustment_value.abs().try_into()?,
        },
    ))
}
