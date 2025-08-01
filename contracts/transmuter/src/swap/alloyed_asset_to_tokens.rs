use std::cmp::Ordering;

use cosmwasm_std::{
    coin, ensure, Addr, BankMsg, Coin, Deps, DepsMut, Env, Int256, Response, Uint128,
};
use osmosis_std::types::osmosis::tokenfactory::v1beta1::MsgBurn;

use crate::{
    alloyed_asset::swap_from_alloyed,
    asset::convert_amount,
    contract::Transmuter,
    corruptable::Corruptable as _,
    swap::common::{
        set_data_if_sudo, Adjustment, Entrypoint, SwapExactAmountInResponseData,
        SwapExactAmountOutResponseData,
    },
    transmuter_pool::TransmuterPool,
    ContractError,
};

#[derive(Debug)]
pub enum SwapFromAlloyedConstraint<'a> {
    ExactIn {
        token_out_denom: &'a str,
        token_out_min_amount: Uint128,
        token_in_amount: Uint128,
    },
    ExactOut {
        tokens_out: &'a [Coin],
        token_in_max_amount: Uint128,
    },
}

/// Determines where to burn alloyed assets from.
pub enum BurnTarget {
    /// Burn alloyed asset from the sender's account.
    /// This is used when the sender wants to exit pool
    /// forcing no funds attached in the process.
    SenderAccount,
    /// Burn alloyed assets from the sent funds.
    /// This is used when the sender wants to swap tokens for alloyed assets,
    /// since alloyed asset needs to be sent to the contract before swapping.
    SentFunds,
}

impl Transmuter {
    pub fn swap_alloyed_asset_to_tokens(
        &self,
        entrypoint: Entrypoint,
        constraint: SwapFromAlloyedConstraint,
        burn_target: BurnTarget,
        sender: Addr,
        mut deps: DepsMut,
        env: Env,
    ) -> Result<Response, ContractError> {
        let (mut pool, in_amount, tokens_out, adjustment, response) = match constraint {
            SwapFromAlloyedConstraint::ExactIn {
                token_out_denom,
                token_out_min_amount,
                token_in_amount,
            } => self.swap_alloyed_asset_to_tokens_exact_in(
                entrypoint,
                token_out_denom,
                token_out_min_amount,
                token_in_amount,
                &sender,
                deps.branch(),
            )?,
            SwapFromAlloyedConstraint::ExactOut {
                tokens_out,
                token_in_max_amount,
            } => self.swap_alloyed_asset_to_tokens_exact_out(
                entrypoint,
                tokens_out,
                token_in_max_amount,
                &sender,
                deps.branch(),
            )?,
        };

        // ensure tokens out has no zero value
        ensure!(
            tokens_out.iter().all(|coin| coin.amount > Uint128::zero()),
            ContractError::ZeroValueOperation {}
        );

        self.clean_up_drained_corrupted_assets(deps.storage, &mut pool)?;
        self.pool.save(deps.storage, &pool)?;

        // We need to burn alloyed asset, which is token in, as it is essentially exiting pool and burn LP token.
        let burn_alloyed_asset_msg = self.create_burn_alloyed_asset_msg(
            burn_target,
            &sender,
            constraint,
            in_amount,
            adjustment,
            deps.branch(),
            env,
        )?;

        // Send tokens out to the sender.
        let bank_send_msg = BankMsg::Send {
            to_address: sender.to_string(),
            amount: tokens_out,
        };

        Ok(response
            .add_message(burn_alloyed_asset_msg)
            .add_message(bank_send_msg))
    }

    fn swap_alloyed_asset_to_tokens_exact_in(
        &self,
        entrypoint: Entrypoint,
        token_out_denom: &str,
        token_out_min_amount: Uint128,
        token_in_amount: Uint128,
        sender: &Addr,
        mut deps: DepsMut,
    ) -> Result<(TransmuterPool, Uint128, Vec<Coin>, Adjustment, Response), ContractError> {
        let mut pool: TransmuterPool = self.pool.load(deps.storage)?;
        let response = Response::new();
        let std_norm_factor = pool.std_norm_factor()?;
        let token_out_norm_factor = pool
            .get_pool_asset_by_denom(token_out_denom)?
            .normalization_factor();
        let out_amount = swap_from_alloyed::out_amount_via_exact_in(
            token_in_amount,
            self.alloyed_asset.get_normalization_factor(deps.storage)?,
            token_out_norm_factor,
            token_out_min_amount,
        )?;

        let mut token_out = coin(out_amount.u128(), token_out_denom);
        let tokens_out = vec![token_out.clone()];
        let mut adjustment = Adjustment::None;

        // If all tokens out are corrupted assets and exit with all remaining liquidity
        // then ignore the limit and remove the corrupted assets from the pool
        if self.is_force_exit_corrupted_assets(&pool, &tokens_out) {
            pool.unchecked_exit_pool(&tokens_out)?;
        } else {
            let run_pool = |_: Deps, mut pool: TransmuterPool| {
                pool.exit_pool(&tokens_out)?;
                Ok((pool, token_out))
            };

            let rebalancing_adjustment =
                |_pool: TransmuterPool, token_out: Coin, total_adjustment_value: Int256| {
                    let (token_out, adjustment) = rebalancing_adjustment_for_exact_in(
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
                };

            (pool, token_out, adjustment) = self.rebalancer_pass(
                deps.branch(),
                pool,
                &sender,
                run_pool,
                rebalancing_adjustment,
            )?;
        }

        let response = set_data_if_sudo(
            response,
            &entrypoint,
            &SwapExactAmountInResponseData {
                token_out_amount: token_out.amount,
            },
        )?;

        let tokens_out = vec![token_out];

        Ok((pool, token_in_amount, tokens_out, adjustment, response))
    }

    fn swap_alloyed_asset_to_tokens_exact_out(
        &self,
        entrypoint: Entrypoint,
        tokens_out: &[Coin],
        token_in_max_amount: Uint128,
        sender: &Addr,
        mut deps: DepsMut,
    ) -> Result<(TransmuterPool, Uint128, Vec<Coin>, Adjustment, Response), ContractError> {
        let response = Response::new();
        let mut pool: TransmuterPool = self.pool.load(deps.storage)?;
        let tokens_out_with_norm_factor = pool.pair_coins_with_normalization_factor(tokens_out)?;

        let token_in_norm_factor = self.alloyed_asset.get_normalization_factor(deps.storage)?;
        let std_norm_factor = pool.std_norm_factor()?;
        let in_amount = swap_from_alloyed::in_amount_via_exact_out(
            token_in_max_amount,
            token_in_norm_factor,
            tokens_out_with_norm_factor,
        )?;

        let token_in_denom = self.alloyed_asset.get_alloyed_denom(deps.storage)?;
        let mut token_in = coin(in_amount.u128(), token_in_denom.clone());
        let mut adjustment = Adjustment::None;

        // If all tokens out are corrupted assets and exit with all remaining liquidity
        // then ignore the limit and remove the corrupted assets from the pool
        if self.is_force_exit_corrupted_assets(&pool, &tokens_out) {
            pool.unchecked_exit_pool(&tokens_out)?;
        } else {
            let run_pool = |_: Deps, mut pool: TransmuterPool| {
                pool.exit_pool(&tokens_out)?;
                Ok((pool, token_in.clone()))
            };

            let rebalancing_adjustment =
                |_pool: TransmuterPool, token_in: Coin, total_adjustment_value: Int256| {
                    let (token_in, adjustment) = rebalancing_adjustment_for_exact_out(
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
                };

            (pool, token_in, adjustment) = self.rebalancer_pass(
                deps.branch(),
                pool,
                &sender,
                run_pool,
                rebalancing_adjustment,
            )?;
        }

        let response = set_data_if_sudo(
            response,
            &entrypoint,
            &SwapExactAmountOutResponseData {
                token_in_amount: token_in.amount,
            },
        )?;

        Ok((
            pool,
            token_in.amount,
            tokens_out.to_vec(),
            adjustment,
            response,
        ))
    }

    /// Create MsgBurn to burn alloyed assets from the sender or sent funds
    /// based on the burn target.
    ///
    /// If the constraint is exact out, and the adjustment is deduct fee, it means we deduct fee from in amount, which is alloyed
    /// In that case we keep the fee portion in contract and burn the rest. Incentive pool accounting is handled within [Transmuter::rebalancer_pass].
    ///
    /// Keep burn amount as is otherwise.
    fn create_burn_alloyed_asset_msg(
        &self,
        burn_target: BurnTarget,
        sender: &Addr,
        constraint: SwapFromAlloyedConstraint,
        in_amount: Uint128,
        adjustment: Adjustment,
        deps: DepsMut,
        env: Env,
    ) -> Result<MsgBurn, ContractError> {
        let burn_from_address = match burn_target {
            BurnTarget::SenderAccount => {
                // Check if the sender's shares is sufficient to burn
                let shares = self.alloyed_asset.get_balance(deps.as_ref(), &sender)?;
                ensure!(
                    shares >= in_amount,
                    ContractError::InsufficientShares {
                        required: in_amount,
                        available: shares
                    }
                );

                Ok::<&Addr, ContractError>(&sender)
            }

            // Burn from the sent funds, funds are guaranteed to be sent via cw-pool mechanism
            // But to defend in depth, we still check the balance of the contract.
            // Theoretically, alloyed asset balance should always remain 0 before any tx since
            // it is always received and burned or minted and sent to another address.
            // Except for the case where the contract is funded with alloyed assets directly
            // that is not as part of transmuter mechanism.
            //
            // So it's safe to check just check that contract has enough alloyed assets to burn.
            // Since it's only being a loss for the actor that does not follow the normal mechanism.
            BurnTarget::SentFunds => {
                // get alloyed denom contract balance
                let alloyed_contract_balance = self
                    .alloyed_asset
                    .get_balance(deps.as_ref(), &env.contract.address)?;

                // ensure that alloyed contract balance is greater than in_amount
                ensure!(
                    alloyed_contract_balance >= in_amount,
                    ContractError::InsufficientShares {
                        required: in_amount,
                        available: alloyed_contract_balance
                    }
                );

                Ok(&env.contract.address)
            }
        }?
        .to_string();

        let burn_amount = match (constraint, adjustment) {
            // If the constraint is exact out, and the adjustment is deduct fee, it means we deduct fee from in amount, which is alloyed
            // In that case we keep the fee portion in contract and burn the rest
            (SwapFromAlloyedConstraint::ExactOut { .. }, Adjustment::DeductFee { fee }) => {
                in_amount.checked_sub(fee.amount)?
            }
            _ => in_amount,
        };

        let alloyed_asset_to_burn = coin(
            burn_amount.u128(),
            self.alloyed_asset.get_alloyed_denom(deps.storage)?,
        )
        .into();

        // burn alloyed assets
        Ok(MsgBurn {
            sender: env.contract.address.to_string(),
            amount: Some(alloyed_asset_to_burn),
            burn_from_address,
        })
    }

    /// Check if the tokens out are all corrupted assets and the pool is empty after exiting.
    /// If so, we can force exit the pool and remove the corrupted assets from the pool.
    fn is_force_exit_corrupted_assets(&self, pool: &TransmuterPool, tokens_out: &[Coin]) -> bool {
        let denoms_in_corrupted_asset_group = pool
            .asset_groups
            .iter()
            .flat_map(|(_, asset_group)| {
                if asset_group.is_corrupted() {
                    asset_group.denoms().to_vec()
                } else {
                    vec![]
                }
            })
            .collect::<Vec<_>>();

        tokens_out.iter().all(|coin| {
            let total_liquidity = pool
                .get_pool_asset_by_denom(&coin.denom)
                .map(|asset| asset.amount())
                .unwrap_or_default();
            let is_redeeming_total_liquidity = coin.amount == total_liquidity;
            let is_under_corrupted_asset_group =
                denoms_in_corrupted_asset_group.contains(&coin.denom);

            is_redeeming_total_liquidity
                && (is_under_corrupted_asset_group || pool.is_corrupted_asset(&coin.denom))
        })
    }
}

/// Rebalancing adjustment for exact in.
///
/// If adjustment value is negative, fee take from the token_out, so we require additional token_out to pay for the fee.
/// If adjustment value is positive, incentive is credited to the beneficiary, return the token_out as is.
/// If adjustment value is zero, no adjustment is made, return the token_out as is.
fn rebalancing_adjustment_for_exact_in(
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

/// Rebalancing adjustment for exact out.
///
/// If adjustment value is negative, fee take from the token_in, so we require additional token_in to pay for the fee.
/// If adjustment value is positive, incentive is credited to the beneficiary, return the token_in as is.
/// If adjustment value is zero, no adjustment is made, return the token_in as is.
fn rebalancing_adjustment_for_exact_out(
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
