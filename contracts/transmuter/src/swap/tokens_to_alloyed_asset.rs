use std::cmp::Ordering;

use cosmwasm_std::{coin, ensure, Addr, Coin, Deps, DepsMut, Env, Int256, Response, Uint128};
use osmosis_std::types::osmosis::tokenfactory::v1beta1::MsgMint;

use crate::{
    alloyed_asset::swap_to_alloyed,
    asset::convert_amount,
    contract::Transmuter,
    swap::{
        set_data_if_sudo, Adjustment, Entrypoint, SwapExactAmountInResponseData,
        SwapExactAmountOutResponseData, SwapToAlloyedConstraint,
    },
    transmuter_pool::TransmuterPool,
    ContractError,
};

impl Transmuter {
    pub fn swap_tokens_to_alloyed_asset(
        &self,
        entrypoint: Entrypoint,
        constraint: SwapToAlloyedConstraint,
        mint_to_address: Addr,
        mut deps: DepsMut,
        env: Env,
    ) -> Result<Response, ContractError> {
        let pool: TransmuterPool = self.pool.load(deps.storage)?;
        let alloyed_denom = self.alloyed_asset.get_alloyed_denom(deps.storage)?;
        let alloyed_norm_factor = self.alloyed_asset.get_normalization_factor(deps.storage)?;

        let response = Response::new();

        let (pool, tokens_in, out_amount, response) = match constraint {
            SwapToAlloyedConstraint::ExactIn {
                tokens_in,
                token_out_min_amount,
            } => {
                let tokens_in_with_norm_factor =
                    pool.pair_coins_with_normalization_factor(tokens_in)?;
                let out_amount_before_fee = swap_to_alloyed::out_amount_via_exact_in(
                    tokens_in_with_norm_factor,
                    token_out_min_amount,
                    self.alloyed_asset.get_normalization_factor(deps.storage)?,
                )?;

                // we have now calculated tokens_in, out_amount (which is alloyed)
                // if it's exact_in:
                // - fee case: we deduct fee from out_amount
                // - incentive case: we credit incentive to the beneficiary

                let run_pool = |_deps: Deps, mut pool: TransmuterPool| {
                    pool.join_pool(&tokens_in)?;
                    let token_out = coin(out_amount_before_fee.u128(), alloyed_denom.clone());

                    Ok((pool, token_out))
                };

                let rebalancing_adjustment =
                    |pool: TransmuterPool, token_out: Coin, total_adjustment_value: Int256| {
                        let std_norm_factor = pool.std_norm_factor()?;
                        let token_out_norm_factor = alloyed_norm_factor;

                        let (token_out, adjustment) =
                            match total_adjustment_value.cmp(&Int256::zero()) {
                                Ordering::Less => {
                                    // deduct fee from token_out
                                    let fee_amount = convert_amount(
                                        total_adjustment_value.abs().try_into()?,
                                        std_norm_factor,
                                        token_out_norm_factor,
                                        // rounding up means slightly more fee than required, keeps the incentive pool healthy
                                        &crate::asset::Rounding::Up,
                                    )?;

                                    let token_out_amount: Uint128 =
                                        token_out.amount.checked_sub(fee_amount)?;
                                    let token_out =
                                        coin(token_out_amount.u128(), token_out.denom.clone());
                                    let token_out_denom = token_out.denom.clone();

                                    (
                                        token_out,
                                        Adjustment::DeductFee {
                                            fee: coin(fee_amount.u128(), token_out_denom),
                                        },
                                    )
                                }
                                Ordering::Greater => (
                                    token_out,
                                    Adjustment::CreditIncentive {
                                        incentive: total_adjustment_value.abs().try_into()?,
                                    },
                                ),
                                Ordering::Equal => (token_out, Adjustment::None),
                            };

                        ensure!(
                            token_out.amount >= token_out_min_amount,
                            ContractError::InsufficientTokenOut {
                                min_required: token_out_min_amount,
                                amount_out: token_out.amount,
                            }
                        );

                        Ok((token_out, adjustment))
                    };

                let (pool, token_out, adjustment) = self.rebalancer_pass(
                    deps.branch(),
                    pool,
                    &mint_to_address,
                    run_pool,
                    rebalancing_adjustment,
                )?;

                let response = set_data_if_sudo(
                    response,
                    &entrypoint,
                    &SwapExactAmountInResponseData {
                        token_out_amount: token_out.amount,
                    },
                )?;

                let response = match adjustment {
                    // fee is deducted from the minting token out, mint directly to the contract as it's already recorded as such in the incentive pool
                    Adjustment::DeductFee { fee } => response.add_message(MsgMint {
                        sender: env.contract.address.to_string(),
                        amount: Some(fee.into()),
                        mint_to_address: env.contract.address.to_string(),
                    }),
                    // incentive doesn't require minting or burning anything
                    Adjustment::CreditIncentive { .. } => response,
                    Adjustment::None => response,
                };

                (pool, tokens_in.to_owned(), token_out.amount, response)
            }

            SwapToAlloyedConstraint::ExactOut {
                token_in_denom,
                token_in_max_amount,
                token_out_amount,
            } => {
                let token_in_norm_factor = pool
                    .get_pool_asset_by_denom(token_in_denom)?
                    .normalization_factor();
                let in_amount_before_fee = swap_to_alloyed::in_amount_via_exact_out(
                    token_in_norm_factor,
                    token_in_max_amount,
                    token_out_amount,
                    self.alloyed_asset.get_normalization_factor(deps.storage)?,
                )?;
                let token_in = coin(in_amount_before_fee.u128(), token_in_denom);

                let run_pool = |_deps: Deps, mut pool: TransmuterPool| {
                    pool.join_pool(&[token_in.clone()])?;
                    Ok((pool, token_in))
                };

                let rebalancing_adjustment =
                    move |pool: TransmuterPool, token_in: Coin, total_adjustment_value: Int256| {
                        let std_norm_factor = pool.std_norm_factor()?;
                        let token_in_norm_factor = pool
                            .get_pool_asset_by_denom(&token_in_denom)?
                            .normalization_factor();

                        // If adjustment value is negative, fee take from the token_in, so we require addtional token_in to pay for the fee.
                        // Otherwise, return the token_in as is
                        let (token_in, adjustment) =
                            match total_adjustment_value.cmp(&Int256::zero()) {
                                // negative adjustment value means fee deduction from token_in
                                Ordering::Less => {
                                    let fee = convert_amount(
                                        total_adjustment_value.abs().try_into()?,
                                        std_norm_factor,
                                        token_in_norm_factor,
                                        // rounding up means slightly more fee than required, keeps the incentive pool healthy
                                        &crate::asset::Rounding::Up,
                                    )?;

                                    let token_in_amount = token_in.amount.checked_add(fee)?;

                                    (
                                        coin(token_in_amount.u128(), token_in.denom.clone()),
                                        Adjustment::DeductFee {
                                            fee: coin(fee.u128(), token_in.denom.clone()),
                                        },
                                    )
                                }
                                // positive adjustment value means incentive credit to the beneficiary
                                Ordering::Greater => (
                                    token_in,
                                    Adjustment::CreditIncentive {
                                        incentive: total_adjustment_value.abs().try_into()?,
                                    },
                                ),
                                // zero adjustment means no adjustment
                                Ordering::Equal => (token_in, Adjustment::None),
                            };

                        let token_in_amount = token_in.amount.clone();

                        ensure!(
                            token_in_amount <= token_in_max_amount,
                            ContractError::ExcessiveRequiredTokenIn {
                                limit: token_in_max_amount,
                                required: token_in_amount,
                            }
                        );

                        Ok((token_in, adjustment))
                    };

                // if it's exact_out
                // - fee case: we increase tokens in requirement, it's spreaded through all the tokens in
                // - incentive case: we credit incentive to the beneficiary
                let (pool, token_in, _adjustment) = self.rebalancer_pass(
                    deps.branch(),
                    pool,
                    &mint_to_address,
                    run_pool,
                    rebalancing_adjustment,
                )?;

                // Unlike exact in case where token out is alloyed, there is no separate mint target required here
                // Because fee is collected from the token_in
                let response = set_data_if_sudo(
                    response,
                    &entrypoint,
                    &SwapExactAmountOutResponseData {
                        token_in_amount: token_in.amount,
                    },
                )?;

                (pool, vec![token_in], token_out_amount, response)
            }
        };

        // ensure funds not empty
        ensure!(
            !tokens_in.is_empty(),
            ContractError::AtLeastSingleTokenExpected {}
        );

        // ensure funds does not have zero coin
        ensure!(
            tokens_in.iter().all(|coin| coin.amount > Uint128::zero()),
            ContractError::ZeroValueOperation {}
        );

        // no need for cleaning up drained corrupted assets here
        // since this function will only adding more underlying assets
        // rather than removing any of them

        self.pool.save(deps.storage, &pool)?;

        let alloyed_asset_out = coin(
            out_amount.u128(),
            self.alloyed_asset.get_alloyed_denom(deps.storage)?,
        );

        let response = response.add_message(MsgMint {
            sender: env.contract.address.to_string(),
            amount: Some(alloyed_asset_out.into()),
            mint_to_address: mint_to_address.to_string(),
        });

        Ok(response)
    }
}
