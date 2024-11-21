use std::collections::{BTreeMap, HashSet};

use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    coin, coins, ensure, ensure_eq, to_json_binary, Addr, BankMsg, Coin, Coins, Decimal, Deps,
    DepsMut, Env, Response, StdError, Storage, Timestamp, Uint128, Uint256,
};
use osmosis_std::types::osmosis::tokenfactory::v1beta1::{MsgBurn, MsgMint};
use serde::Serialize;
use transmuter_math::rebalancing_incentive::{
    calculate_impact_factor, calculate_rebalancing_fee, calculate_rebalancing_incentive,
    ImpactFactor, ImpactFactorParamGroup,
};

use crate::{
    alloyed_asset::{swap_from_alloyed, swap_to_alloyed},
    asset::{convert_amount, Rounding},
    contract::Transmuter,
    corruptable::Corruptable,
    scope::Scope,
    transmuter_pool::{AmountConstraint, TransmuterPool},
    ContractError,
};

#[derive(Debug, PartialEq, Eq)]
pub enum RebalancingIncentiveAction {
    CollectFee(Coins),
    DistributeIncentive(Coin),
    None,
}

/// Swap fee is hardcoded to zero intentionally.
pub const SWAP_FEE: Decimal = Decimal::zero();

impl Transmuter {
    /// Getting the [SwapVariant] of the swap operation
    /// assuming the swap token is not
    pub fn swap_variant(
        &self,
        token_in_denom: &str,
        token_out_denom: &str,
        deps: Deps,
    ) -> Result<SwapVariant, ContractError> {
        ensure!(
            token_in_denom != token_out_denom,
            ContractError::SameDenomNotAllowed {
                denom: token_in_denom.to_string()
            }
        );

        let alloyed_denom = self.alloyed_asset.get_alloyed_denom(deps.storage)?;
        let alloyed_denom = alloyed_denom.as_str();

        if alloyed_denom == token_in_denom {
            return Ok(SwapVariant::AlloyedToToken);
        }

        if alloyed_denom == token_out_denom {
            return Ok(SwapVariant::TokenToAlloyed);
        }

        Ok(SwapVariant::TokenToToken)
    }

    pub fn swap_tokens_to_alloyed_asset(
        &self,
        entrypoint: Entrypoint,
        constraint: SwapToAlloyedConstraint,
        mint_to_address: Addr,
        mut deps: DepsMut,
        env: Env,
    ) -> Result<Response, ContractError> {
        let mut pool: TransmuterPool = self.pool.load(deps.storage)?;
        let alloyed_denom = self.alloyed_asset.get_alloyed_denom(deps.storage)?;
        let alloyed_denom_normalization_factor =
            self.alloyed_asset.get_normalization_factor(deps.storage)?;

        let response = Response::new();

        let (tokens_in, out_amount, response) = match constraint {
            SwapToAlloyedConstraint::ExactIn {
                tokens_in,
                token_out_min_amount,
            } => {
                // ensure funds does not have zero coin
                // this is a pre-guard to avoid unnecessary computation
                // TODO: move this to constraint validation
                ensure!(
                    tokens_in.iter().all(|coin| coin.amount > Uint128::zero()),
                    ContractError::ZeroValueOperation {}
                );

                let action = self.rebalancing_incentive_pass(
                    deps.branch(),
                    env.block.time,
                    &tokens_in,
                    &alloyed_denom,
                    pool.clone(),
                    |mut pool| {
                        pool.join_pool(&tokens_in)?;
                        Ok(pool)
                    },
                )?;

                let mut tokens_in = Coins::try_from(tokens_in.to_owned())?;

                let (fee, incentive) = match action {
                    RebalancingIncentiveAction::CollectFee(fee_coins) => (Some(fee_coins), None),
                    RebalancingIncentiveAction::DistributeIncentive(incentive_coin) => {
                        (None, Some(incentive_coin))
                    }
                    RebalancingIncentiveAction::None => (None, None),
                };

                // if fee is present, subtract the fee from tokens_in
                if let Some(fee_coins) = fee {
                    for coin in fee_coins {
                        tokens_in.sub(coin)?;
                    }
                }

                let tokens_in = tokens_in.to_vec();

                let tokens_in_with_norm_factor =
                    pool.pair_coins_with_normalization_factor(&tokens_in)?;
                let mut out_amount = swap_to_alloyed::out_amount_via_exact_in(
                    tokens_in_with_norm_factor,
                    token_out_min_amount,
                    alloyed_denom_normalization_factor,
                )?;

                // if incentive is present, add the incentive to out_amount
                if let Some(Coin { amount, .. }) = incentive {
                    out_amount = out_amount.checked_add(amount)?;
                }

                let response = set_data_if_sudo(
                    response,
                    &entrypoint,
                    &SwapExactAmountInResponseData {
                        token_out_amount: out_amount,
                    },
                )?;

                (tokens_in.to_owned(), out_amount, response)
            }

            SwapToAlloyedConstraint::ExactOut {
                token_in_denom,
                token_in_max_amount,
                token_out_amount,
            } => {
                let token_in_norm_factor = pool
                    .get_pool_asset_by_denom(token_in_denom)?
                    .normalization_factor();

                // convert amount out (alloyed term) to match value in token in denom term
                let in_amount = convert_amount(
                    token_out_amount,
                    alloyed_denom_normalization_factor,
                    token_in_norm_factor,
                    &Rounding::Up,
                )?;

                let tokens_in = vec![coin(in_amount.u128(), token_in_denom)];
                let action = self.rebalancing_incentive_pass(
                    deps.branch(),
                    env.block.time,
                    &tokens_in,
                    &alloyed_denom,
                    pool.clone(),
                    |mut pool| {
                        pool.join_pool(&tokens_in)?;
                        Ok(pool)
                    },
                )?;

                let mut tokens_in = Coins::try_from(tokens_in)?;
                let (fee, incentive) = match action {
                    RebalancingIncentiveAction::CollectFee(fee_coins) => (Some(fee_coins), None),
                    RebalancingIncentiveAction::DistributeIncentive(incentive_coin) => {
                        (None, Some(incentive_coin))
                    }
                    RebalancingIncentiveAction::None => (None, None),
                };

                // if fee is present, subtract the fee from tokens_in
                if let Some(fee_coins) = fee {
                    for coin in fee_coins {
                        tokens_in.sub(coin)?;
                    }
                }

                let in_amount = tokens_in.amount_of(token_in_denom);

                ensure!(
                    in_amount <= token_in_max_amount,
                    ContractError::ExcessiveRequiredTokenIn {
                        limit: token_in_max_amount,
                        required: in_amount
                    }
                );

                let token_out_amount = convert_amount(
                    in_amount,
                    token_in_norm_factor,
                    alloyed_denom_normalization_factor,
                    &Rounding::Down,
                )?;

                let response = set_data_if_sudo(
                    response,
                    &entrypoint,
                    &SwapExactAmountOutResponseData {
                        token_in_amount: in_amount,
                    },
                )?;

                (tokens_in.to_vec(), token_out_amount, response)
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

        (pool, _) = self.limiters_pass(deps.branch(), env.block.time, pool, |_, mut pool| {
            pool.join_pool(&tokens_in)?;
            Ok((pool, ()))
        })?;

        // no need for cleaning up drained corrupted assets here
        // since this function will only adding more underlying assets
        // rather than removing any of them

        self.pool.save(deps.storage, &pool)?;

        let alloyed_asset_out = coin(out_amount.u128(), alloyed_denom);

        let response = response.add_message(MsgMint {
            sender: env.contract.address.to_string(),
            amount: Some(alloyed_asset_out.into()),
            mint_to_address: mint_to_address.to_string(),
        });

        Ok(response)
    }

    pub fn swap_alloyed_asset_to_tokens(
        &self,
        entrypoint: Entrypoint,
        constraint: SwapFromAlloyedConstraint,
        burn_target: BurnTarget,
        sender: Addr,
        mut deps: DepsMut,
        env: Env,
    ) -> Result<Response, ContractError> {
        let mut pool: TransmuterPool = self.pool.load(deps.storage)?;
        let alloyed_denom = self.alloyed_asset.get_alloyed_denom(deps.storage)?;
        let alloyed_denom_normalization_factor =
            self.alloyed_asset.get_normalization_factor(deps.storage)?;
        let response = Response::new();

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

        let check_is_force_exit_corrupted_assets = |tokens_out: &[Coin]| {
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
        };

        let (in_amount, tokens_out, incentive, response) = match constraint {
            SwapFromAlloyedConstraint::ExactIn {
                token_out_denom,
                token_out_min_amount,
                token_in_amount,
            } => {
                let token_out_norm_factor = pool
                    .get_pool_asset_by_denom(token_out_denom)?
                    .normalization_factor();
                let out_amount = swap_from_alloyed::out_amount_via_exact_in(
                    token_in_amount,
                    alloyed_denom_normalization_factor,
                    token_out_norm_factor,
                    Uint128::zero(),
                )?;

                let is_force_exit_corrupted_assets = check_is_force_exit_corrupted_assets(&[coin(
                    out_amount.u128(),
                    token_out_denom,
                )]);

                // no rebalancing incentive if force exit corrupted assets
                let action = if is_force_exit_corrupted_assets {
                    RebalancingIncentiveAction::None
                } else {
                    self.rebalancing_incentive_pass(
                        deps.branch(),
                        env.block.time,
                        &coins(token_in_amount.u128(), alloyed_denom.clone()),
                        &token_out_denom,
                        pool.clone(),
                        |mut pool| {
                            pool.exit_pool(&coins(out_amount.u128(), token_out_denom))?;
                            Ok(pool)
                        },
                    )?
                };

                let mut tokens_in =
                    Coins::try_from(coins(token_in_amount.u128(), alloyed_denom.clone()))?;

                let (fee, incentive) = match action {
                    RebalancingIncentiveAction::CollectFee(fee_coins) => (Some(fee_coins), None),
                    RebalancingIncentiveAction::DistributeIncentive(incentive_coin) => {
                        (None, Some(incentive_coin))
                    }
                    RebalancingIncentiveAction::None => (None, None),
                };

                // if fee is present, subtract the fee from tokens_in
                if let Some(fee_coins) = fee {
                    for coin in fee_coins {
                        tokens_in.sub(coin)?;
                    }
                }

                let token_in_amount = tokens_in.amount_of(&alloyed_denom);

                let token_out_norm_factor = pool
                    .get_pool_asset_by_denom(token_out_denom)?
                    .normalization_factor();

                let incentive_amount = incentive.clone().map(|c| c.amount).unwrap_or_default();

                let out_amount = swap_from_alloyed::out_amount_via_exact_in(
                    token_in_amount,
                    self.alloyed_asset.get_normalization_factor(deps.storage)?,
                    token_out_norm_factor,
                    token_out_min_amount.checked_sub(incentive_amount)?,
                )?;

                let response = set_data_if_sudo(
                    response,
                    &entrypoint,
                    &SwapExactAmountInResponseData {
                        // only add it here since returned token out is used for actual exit pool
                        token_out_amount: out_amount.checked_add(incentive_amount)?,
                    },
                )?;

                let tokens_out = vec![coin(out_amount.u128(), token_out_denom)];

                (token_in_amount, tokens_out, incentive, response)
            }
            SwapFromAlloyedConstraint::ExactOut {
                tokens_out,
                token_in_max_amount,
            } => {
                let is_force_exit_corrupted_assets =
                    check_is_force_exit_corrupted_assets(&tokens_out);

                let tokens_out_with_norm_factor =
                    pool.pair_coins_with_normalization_factor(tokens_out)?;

                let in_amount = tokens_out_with_norm_factor.iter().try_fold(
                    Uint128::zero(),
                    |acc, (Coin { amount, .. }, norm_factor)| -> Result<_, ContractError> {
                        let converted_amount = convert_amount(
                            *amount,
                            *norm_factor,
                            alloyed_denom_normalization_factor,
                            &Rounding::Up,
                        )?;
                        Ok(acc.checked_add(converted_amount)?)
                    },
                )?;

                let action = if is_force_exit_corrupted_assets {
                    RebalancingIncentiveAction::None
                } else {
                    self.rebalancing_incentive_pass(
                        deps.branch(),
                        env.block.time,
                        &coins(in_amount.u128(), alloyed_denom.clone()),
                        &tokens_out[0].denom, // TODO: support multiple tokens out
                        pool.clone(),
                        |mut pool| {
                            pool.exit_pool(&tokens_out)?;
                            Ok(pool)
                        },
                    )?
                };

                let mut tokens_in =
                    Coins::try_from(coins(in_amount.u128(), alloyed_denom.clone()))?;

                let (fee, incentive) = match action {
                    RebalancingIncentiveAction::CollectFee(fee_coins) => (Some(fee_coins), None),
                    RebalancingIncentiveAction::DistributeIncentive(incentive_coin) => {
                        (None, Some(incentive_coin))
                    }
                    RebalancingIncentiveAction::None => (None, None),
                };

                // if fee is present, subtract the fee from tokens_in
                if let Some(fee_coins) = fee {
                    for coin in fee_coins {
                        tokens_in.sub(coin)?;
                    }
                }

                let in_amount = tokens_in.amount_of(&alloyed_denom);

                ensure!(
                    in_amount <= token_in_max_amount,
                    ContractError::ExcessiveRequiredTokenIn {
                        limit: token_in_max_amount,
                        required: in_amount
                    }
                );

                let response = set_data_if_sudo(
                    response,
                    &entrypoint,
                    &SwapExactAmountOutResponseData {
                        token_in_amount: in_amount,
                    },
                )?;

                (in_amount, tokens_out.to_vec(), incentive, response)
            }
        };

        // ensure tokens out has no zero value
        ensure!(
            tokens_out.iter().all(|coin| coin.amount > Uint128::zero()),
            ContractError::ZeroValueOperation {}
        );

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

        let is_force_exit_corrupted_assets = check_is_force_exit_corrupted_assets(&tokens_out);

        // If all tokens out are corrupted assets and exit with all remaining liquidity
        // then ignore the limiters and remove the corrupted assets from the pool
        if is_force_exit_corrupted_assets {
            pool.unchecked_exit_pool(&tokens_out)?;

            // change limiter needs reset if force redemption since it gets by passed
            // the current state will not be accurate

            let asset_weights_iter = pool
                .asset_weights()?
                .unwrap_or_default()
                .into_iter()
                .map(|(denom, weight)| (Scope::denom(&denom).key(), weight));
            let asset_group_weights_iter = pool
                .asset_group_weights()?
                .unwrap_or_default()
                .into_iter()
                .map(|(label, weight)| (Scope::asset_group(&label).key(), weight));

            self.limiters.reset_change_limiter_states(
                deps.storage,
                env.block.time,
                asset_weights_iter.chain(asset_group_weights_iter),
            )?;
        } else {
            (pool, _) =
                self.limiters_pass(deps.branch(), env.block.time, pool, |_, mut pool| {
                    pool.exit_pool(&tokens_out)?;
                    Ok((pool, ()))
                })?;
        }

        self.clean_up_drained_corrupted_assets(deps.storage, &mut pool)?;

        self.pool.save(deps.storage, &pool)?;

        let mut tokens_out: Coins = Coins::try_from(tokens_out)?;
        if let Some(incentive) = incentive {
            tokens_out.add(incentive)?;
        }
        let tokens_out = tokens_out.to_vec();

        let bank_send_msg = BankMsg::Send {
            to_address: sender.to_string(),
            amount: tokens_out,
        };

        let alloyed_asset_to_burn = coin(in_amount.u128(), alloyed_denom).into();

        // burn alloyed assets
        let burn_msg = MsgBurn {
            sender: env.contract.address.to_string(),
            amount: Some(alloyed_asset_to_burn),
            burn_from_address,
        };

        Ok(response.add_message(burn_msg).add_message(bank_send_msg))
    }

    pub fn swap_non_alloyed_exact_amount_in(
        &self,
        token_in: Coin,
        token_out_denom: &str,
        token_out_min_amount: Uint128,
        sender: Addr,
        mut deps: DepsMut,
        env: Env,
    ) -> Result<Response, ContractError> {
        let pool = self.pool.load(deps.storage)?;

        let (mut pool, actual_token_out) =
            self.limiters_pass(deps.branch(), env.block.time, pool, |deps, pool| {
                let (pool, actual_token_out) =
                    self.out_amt_given_in(deps, pool, token_in, token_out_denom)?;

                // ensure token_out amount is greater than or equal to token_out_min_amount
                ensure!(
                    actual_token_out.amount >= token_out_min_amount,
                    ContractError::InsufficientTokenOut {
                        min_required: token_out_min_amount,
                        amount_out: actual_token_out.amount
                    }
                );

                Ok((pool, actual_token_out))
            })?;

        self.clean_up_drained_corrupted_assets(deps.storage, &mut pool)?;

        // save pool
        self.pool.save(deps.storage, &pool)?;

        let send_token_out_to_sender_msg = BankMsg::Send {
            to_address: sender.to_string(),
            amount: vec![actual_token_out.clone()],
        };

        let swap_result = SwapExactAmountInResponseData {
            token_out_amount: actual_token_out.amount,
        };

        Ok(Response::new()
            .add_message(send_token_out_to_sender_msg)
            .set_data(to_json_binary(&swap_result)?))
    }

    pub fn swap_non_alloyed_exact_amount_out(
        &self,
        token_in_denom: &str,
        token_in_max_amount: Uint128,
        token_out: Coin,
        sender: Addr,
        mut deps: DepsMut,
        env: Env,
    ) -> Result<Response, ContractError> {
        let pool = self.pool.load(deps.storage)?;

        let (mut pool, actual_token_in) =
            self.limiters_pass(deps.branch(), env.block.time, pool, |deps, pool| {
                let (pool, actual_token_in) = self.in_amt_given_out(
                    deps,
                    pool,
                    token_out.clone(),
                    token_in_denom.to_string(),
                )?;

                ensure!(
                    actual_token_in.amount <= token_in_max_amount,
                    ContractError::ExcessiveRequiredTokenIn {
                        limit: token_in_max_amount,
                        required: actual_token_in.amount,
                    }
                );

                Ok((pool, actual_token_in))
            })?;

        self.clean_up_drained_corrupted_assets(deps.storage, &mut pool)?;

        // save pool
        self.pool.save(deps.storage, &pool)?;

        let send_token_out_to_sender_msg = BankMsg::Send {
            to_address: sender.to_string(),
            amount: vec![token_out],
        };

        let swap_result = SwapExactAmountOutResponseData {
            token_in_amount: actual_token_in.amount,
        };

        Ok(Response::new()
            .add_message(send_token_out_to_sender_msg)
            .set_data(to_json_binary(&swap_result)?))
    }

    pub fn in_amt_given_out(
        &self,
        deps: Deps,
        mut pool: TransmuterPool,
        token_out: Coin,
        token_in_denom: String,
    ) -> Result<(TransmuterPool, Coin), ContractError> {
        let swap_variant = self.swap_variant(&token_in_denom, &token_out.denom, deps)?;

        Ok(match swap_variant {
            SwapVariant::TokenToAlloyed => {
                let token_in_norm_factor = pool
                    .get_pool_asset_by_denom(&token_in_denom)?
                    .normalization_factor();

                let token_in_amount = swap_to_alloyed::in_amount_via_exact_out(
                    token_in_norm_factor,
                    Uint128::MAX,
                    token_out.amount,
                    self.alloyed_asset.get_normalization_factor(deps.storage)?,
                )?;
                let token_in = coin(token_in_amount.u128(), token_in_denom);
                pool.join_pool(&[token_in.clone()])?;
                (pool, token_in)
            }
            SwapVariant::AlloyedToToken => {
                let token_out_norm_factor = pool
                    .get_pool_asset_by_denom(&token_out.denom)?
                    .normalization_factor();

                let token_in_amount = swap_from_alloyed::in_amount_via_exact_out(
                    Uint128::MAX,
                    self.alloyed_asset.get_normalization_factor(deps.storage)?,
                    vec![(token_out.clone(), token_out_norm_factor)],
                )?;
                let token_in = coin(token_in_amount.u128(), token_in_denom);
                pool.exit_pool(&[token_out])?;
                (pool, token_in)
            }
            SwapVariant::TokenToToken => {
                let (token_in, actual_token_out) = pool.transmute(
                    AmountConstraint::exact_out(token_out.amount),
                    &token_in_denom,
                    &token_out.denom,
                )?;

                // ensure that actual_token_out is equal to token_out
                ensure_eq!(
                    token_out,
                    actual_token_out,
                    ContractError::InvalidTokenOutAmount {
                        expected: token_out.amount,
                        actual: actual_token_out.amount
                    }
                );

                (pool, token_in)
            }
        })
    }

    pub fn out_amt_given_in(
        &self,
        deps: Deps,
        mut pool: TransmuterPool,
        token_in: Coin,
        token_out_denom: &str,
    ) -> Result<(TransmuterPool, Coin), ContractError> {
        let swap_variant = self.swap_variant(&token_in.denom, token_out_denom, deps)?;

        Ok(match swap_variant {
            SwapVariant::TokenToAlloyed => {
                let token_in_norm_factor = pool
                    .get_pool_asset_by_denom(&token_in.denom)?
                    .normalization_factor();

                let token_out_amount = swap_to_alloyed::out_amount_via_exact_in(
                    vec![(token_in.clone(), token_in_norm_factor)],
                    Uint128::zero(),
                    self.alloyed_asset.get_normalization_factor(deps.storage)?,
                )?;
                let token_out = coin(token_out_amount.u128(), token_out_denom);
                pool.join_pool(&[token_in])?;
                (pool, token_out)
            }
            SwapVariant::AlloyedToToken => {
                let token_out_norm_factor = pool
                    .get_pool_asset_by_denom(token_out_denom)?
                    .normalization_factor();

                let token_out_amount = swap_from_alloyed::out_amount_via_exact_in(
                    token_in.amount,
                    self.alloyed_asset.get_normalization_factor(deps.storage)?,
                    token_out_norm_factor,
                    Uint128::zero(),
                )?;
                let token_out = coin(token_out_amount.u128(), token_out_denom);
                pool.exit_pool(&[token_out.clone()])?;
                (pool, token_out)
            }
            SwapVariant::TokenToToken => {
                let (actual_token_in, token_out) = pool.transmute(
                    AmountConstraint::exact_in(token_in.amount),
                    &token_in.denom,
                    token_out_denom,
                )?;

                // ensure that actual_token_in is equal to token_in
                ensure_eq!(
                    token_in,
                    actual_token_in,
                    ContractError::InvalidTokenInAmount {
                        expected: token_in.amount,
                        actual: actual_token_in.amount
                    }
                );

                (pool, token_out)
            }
        })
    }

    pub fn limiters_pass<T, F>(
        &self,
        deps: DepsMut,
        block_time: Timestamp,
        pool: TransmuterPool,
        run: F,
    ) -> Result<(TransmuterPool, T), ContractError>
    where
        F: FnOnce(Deps, TransmuterPool) -> Result<(TransmuterPool, T), ContractError>,
    {
        let prev_asset_weights = pool.asset_weights()?.unwrap_or_default();
        let prev_asset_group_weights = pool.asset_group_weights()?.unwrap_or_default();

        let (pool, payload) = run(deps.as_ref(), pool)?;

        // check and update limiters only if pool assets are not zero
        if let Some(updated_asset_weights) = pool.asset_weights()? {
            if let Some(updated_asset_group_weights) = pool.asset_group_weights()? {
                let scope_value_pairs = construct_scope_value_pairs(
                    prev_asset_weights,
                    updated_asset_weights,
                    prev_asset_group_weights,
                    updated_asset_group_weights,
                )?;

                self.limiters.check_limits_and_update(
                    deps.storage,
                    scope_value_pairs,
                    block_time,
                )?;
            }
        }

        Ok((pool, payload))
    }

    pub fn rebalancing_incentive_pass<F>(
        &self,
        deps: DepsMut,
        block_time: Timestamp,
        tokens_in: &[Coin],
        token_out_denom: &str,
        pool: TransmuterPool,
        run: F,
    ) -> Result<RebalancingIncentiveAction, ContractError>
    where
        F: FnOnce(TransmuterPool) -> Result<TransmuterPool, ContractError>,
    {
        let rebalancing_incentive_config = self.rebalancing_incentive_config.load(deps.storage)?;
        let ideal_balances = rebalancing_incentive_config.ideal_balances();
        let upper_limits = self.limiters.upper_limits(deps.storage, block_time)?;
        let alloyed_denom = self.alloyed_asset.get_alloyed_denom(deps.storage)?;
        let alloyed_normalization_factor =
            self.alloyed_asset.get_normalization_factor(deps.storage)?;

        let prev_normalized_balance = pool.weights()?.unwrap_or_default();

        let pool = run(pool)?;

        let update_normalized_balance = pool.weights()?.unwrap_or_default();

        let implact_factor_param_groups: Vec<ImpactFactorParamGroup> = pool
            .scopes()?
            .into_iter()
            .map(|scope| {
                let ideal_balance = ideal_balances.get(&scope).copied().unwrap_or_default();
                let prev_normalized_balance = prev_normalized_balance
                    .get(&scope)
                    .copied()
                    .unwrap_or_else(|| Decimal::zero());

                let updated_normalized_balance = update_normalized_balance
                    .get(&scope)
                    .copied()
                    .unwrap_or_else(|| Decimal::zero());

                let upper_limit = upper_limits
                    .get(&scope)
                    .copied()
                    .unwrap_or_else(|| Decimal::one());

                Ok::<_, ContractError>(ImpactFactorParamGroup::new(
                    prev_normalized_balance,
                    updated_normalized_balance,
                    ideal_balance.lower,
                    ideal_balance.upper,
                    upper_limit,
                )?)
            })
            .collect::<Result<Vec<_>, _>>()?;

        let impact_factor = calculate_impact_factor(&implact_factor_param_groups)?;

        match impact_factor {
            ImpactFactor::Fee(impact_factor) => {
                let mut incentive_pool = self.incentive_pool.load(deps.storage)?;
                if tokens_in.is_empty() {
                    return Ok(RebalancingIncentiveAction::None);
                }

                let mut fee_coins = Coins::default();
                for token_in in tokens_in {
                    let fee_amount: Uint128 = calculate_rebalancing_fee(
                        rebalancing_incentive_config.lambda,
                        impact_factor,
                        token_in.amount,
                    )?
                    .to_uint_ceil()
                    .try_into()?; // safe to convert to Uint128 as it's always less than the token amount

                    let fee_coin = coin(fee_amount.u128(), token_in.denom.clone());

                    incentive_pool.collect(fee_coin.clone())?;
                    fee_coins.add(fee_coin)?;
                }

                self.incentive_pool.save(deps.storage, &incentive_pool)?;

                Ok(RebalancingIncentiveAction::CollectFee(fee_coins))
            }
            ImpactFactor::Incentive(impact_factor) => {
                let mut incentive_pool = self.incentive_pool.load(deps.storage)?;

                let token_out_incentive_balance = incentive_pool
                    .balances
                    .get(token_out_denom)
                    .cloned()
                    .unwrap_or_default();

                // normalized and sum total amount in
                let std_amount_in = pool
                    .normalize_coins(
                        tokens_in,
                        alloyed_denom.clone(),
                        alloyed_normalization_factor,
                    )?
                    .into_iter()
                    .map(|(_, amount)| amount)
                    .fold(Ok(Uint128::zero()), |acc, amount| {
                        acc.and_then(|acc| acc.checked_add(amount))
                    })?;

                // denormalize amount in back to the token out denom norm
                let amount_in_as_incentive_token_norm = pool.denormalize_amount(
                    std_amount_in,
                    token_out_denom.to_string(),
                    alloyed_denom,
                    alloyed_normalization_factor,
                )?;

                let lambda_hist = token_out_incentive_balance.historical_lambda;
                let lambda_curr = rebalancing_incentive_config.lambda;
                let incentive_pool_hist =
                    token_out_incentive_balance.historical_lambda_collected_balance;
                let incentive_pool_curr =
                    token_out_incentive_balance.current_lambda_collected_balance;

                let incentive_amount = calculate_rebalancing_incentive(
                    impact_factor,
                    amount_in_as_incentive_token_norm,
                    lambda_hist,
                    lambda_curr,
                    incentive_pool_hist,
                    incentive_pool_curr,
                )?;

                let incentive_coin = coin(incentive_amount.u128(), token_out_denom.to_string());

                incentive_pool.deduct(incentive_coin.clone())?;
                self.incentive_pool.save(deps.storage, &incentive_pool)?;

                Ok(RebalancingIncentiveAction::DistributeIncentive(
                    incentive_coin,
                ))
            }
            ImpactFactor::None => Ok(RebalancingIncentiveAction::None),
        }
    }

    pub fn ensure_valid_swap_fee(&self, swap_fee: Decimal) -> Result<(), ContractError> {
        // ensure swap fee is the same as one from get_swap_fee which essentially is always 0
        // in case where the swap fee mismatch, it can cause the pool to be imbalanced
        ensure_eq!(
            swap_fee,
            SWAP_FEE,
            ContractError::InvalidSwapFee {
                expected: SWAP_FEE,
                actual: swap_fee
            }
        );
        Ok(())
    }

    /// remove corrupted assets from the pool & deregister all limiters for that denom
    /// when each corrupted asset is all redeemed
    fn clean_up_drained_corrupted_assets(
        &self,
        storage: &mut dyn Storage,
        pool: &mut TransmuterPool,
    ) -> Result<(), ContractError> {
        let mut rebalancing_incentive_config = self.rebalancing_incentive_config.load(storage)?;
        // remove corrupted assets
        for corrupted in pool.clone().corrupted_assets() {
            if corrupted.amount().is_zero() {
                pool.remove_asset(corrupted.denom())?;
                let scope = Scope::denom(corrupted.denom());
                rebalancing_incentive_config.remove_ideal_balance(scope.clone());
                self.limiters
                    .uncheck_deregister_all_for_scope(storage, scope)?;
            }
        }

        // remove assets from asset groups
        for (label, asset_group) in pool.clone().asset_groups {
            // if asset group is corrupted
            if asset_group.is_corrupted() {
                // remove asset from pool if amount is zero.
                // removing asset here will also remove it from the asset group
                let mut total_amount = Uint256::zero();
                for denom in asset_group.denoms() {
                    let amount = pool.get_pool_asset_by_denom(denom)?.amount();
                    total_amount = total_amount.checked_add(amount.into())?;
                    if amount.is_zero() {
                        pool.remove_asset(denom)?;
                        rebalancing_incentive_config.remove_ideal_balance(Scope::denom(denom));
                    }
                }

                if total_amount.is_zero() {
                    rebalancing_incentive_config.remove_ideal_balance(Scope::asset_group(&label));
                }

                // remove asset group is removed
                // remove limiters for asset group as well
                if pool.asset_groups.get(&label).is_none() {
                    self.limiters
                        .uncheck_deregister_all_for_scope(storage, Scope::asset_group(&label))?;
                }
            }
        }

        self.rebalancing_incentive_config
            .save(storage, &rebalancing_incentive_config)?;

        Ok(())
    }
}

fn construct_scope_value_pairs(
    prev_asset_weights: BTreeMap<String, Decimal>,
    updated_asset_weights: BTreeMap<String, Decimal>,
    prev_asset_group_weights: BTreeMap<String, Decimal>,
    updated_asset_group_weights: BTreeMap<String, Decimal>,
) -> Result<Vec<(Scope, (Decimal, Decimal))>, StdError> {
    let mut scope_value_pairs: Vec<(Scope, (Decimal, Decimal))> = Vec::new();

    let denoms = prev_asset_weights
        .keys()
        .chain(updated_asset_weights.keys())
        .collect::<HashSet<_>>();

    let asset_groups = prev_asset_group_weights
        .keys()
        .chain(updated_asset_group_weights.keys())
        .collect::<HashSet<_>>();

    for denom in denoms {
        let prev_weight = prev_asset_weights
            .get(denom)
            .copied()
            .unwrap_or(Decimal::zero());
        let updated_weight = updated_asset_weights
            .get(denom)
            .copied()
            .unwrap_or(Decimal::zero());
        scope_value_pairs.push((Scope::denom(denom), (prev_weight, updated_weight)));
    }

    for asset_group in asset_groups {
        let prev_weight = prev_asset_group_weights
            .get(asset_group)
            .copied()
            .unwrap_or(Decimal::zero());
        let updated_weight = updated_asset_group_weights
            .get(asset_group)
            .copied()
            .unwrap_or(Decimal::zero());
        scope_value_pairs.push((
            Scope::asset_group(asset_group),
            (prev_weight, updated_weight),
        ));
    }

    Ok(scope_value_pairs)
}

/// Possible variants of swap, depending on the input and output tokens
#[derive(PartialEq, Debug)]
pub enum SwapVariant {
    /// Swap any token to alloyed asset
    TokenToAlloyed,

    /// Swap alloyed asset to any token
    AlloyedToToken,

    /// Swap any token to any token
    TokenToToken,
}

pub enum Entrypoint {
    Exec,
    Sudo,
}

pub fn set_data_if_sudo<T>(
    response: Response,
    entrypoint: &Entrypoint,
    data: &T,
) -> Result<Response, StdError>
where
    T: Serialize + ?Sized,
{
    Ok(match entrypoint {
        Entrypoint::Sudo => response.set_data(to_json_binary(data)?),
        Entrypoint::Exec => response,
    })
}

#[cw_serde]
/// Fixing token in amount makes token amount out varies
pub struct SwapExactAmountInResponseData {
    pub token_out_amount: Uint128,
}

#[cw_serde]
/// Fixing token out amount makes token amount in varies
pub struct SwapExactAmountOutResponseData {
    pub token_in_amount: Uint128,
}

#[derive(Debug)]
pub enum SwapToAlloyedConstraint<'a> {
    ExactIn {
        tokens_in: &'a [Coin],
        token_out_min_amount: Uint128,
    },
    ExactOut {
        token_in_denom: &'a str,
        token_in_max_amount: Uint128,
        token_out_amount: Uint128,
    },
}

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

#[cfg(test)]
mod tests {
    use crate::{
        asset::Asset,
        corruptable::Corruptable,
        limiter::LimiterParams,
        rebalancing_incentive::{
            config::{IdealBalance, RebalancingIncentiveConfig},
            incentive_pool::{IncentivePool, IncentivePoolBalance},
        },
        transmuter_pool::AssetGroup,
    };

    use super::*;
    use cosmwasm_std::{
        coin, coins, from_json,
        testing::{
            mock_dependencies, mock_env, MockApi, MockQuerier, MockStorage, MOCK_CONTRACT_ADDR,
        },
        Empty, OwnedDeps, SubMsg,
    };
    use itertools::Itertools;
    use rstest::{fixture, rstest};

    #[rstest]
    #[case("denom1", "denom2", Ok(SwapVariant::TokenToToken))]
    #[case("denom2", "denom1", Ok(SwapVariant::TokenToToken))]
    #[case("denom1", "denom1", Err(ContractError::SameDenomNotAllowed {
        denom: "denom1".to_string()
    }))]
    #[case("denom1", "alloyed", Ok(SwapVariant::TokenToAlloyed))]
    #[case("alloyed", "denom1", Ok(SwapVariant::AlloyedToToken))]
    #[case("alloyed", "alloyed", Err(ContractError::SameDenomNotAllowed {
        denom: "alloyed".to_string()
    }))]
    fn test_swap_variant(
        #[case] denom1: &str,
        #[case] denom2: &str,
        #[case] res: Result<SwapVariant, ContractError>,
    ) {
        let mut deps = cosmwasm_std::testing::mock_dependencies();
        let transmuter = Transmuter::new();
        transmuter
            .alloyed_asset
            .set_alloyed_denom(&mut deps.storage, &"alloyed".to_string())
            .unwrap();

        assert_eq!(transmuter.swap_variant(denom1, denom2, deps.as_ref()), res);
    }

    #[rstest]
    #[case(
        Entrypoint::Exec,
        SwapToAlloyedConstraint::ExactIn {
            tokens_in: &[coin(100, "denom1")],
            token_out_min_amount: Uint128::one(),
        },
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgMint {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(10000u128, "alloyed").into()),
                mint_to_address: "addr1".to_string()
            })),
    )]
    #[case(
        Entrypoint::Sudo,
        SwapToAlloyedConstraint::ExactIn {
            tokens_in: &[coin(100, "denom1")],
            token_out_min_amount: Uint128::one(),
        },
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .set_data(to_json_binary(&SwapExactAmountInResponseData {
                token_out_amount: Uint128::new(10000u128)
            }).unwrap())
            .add_message(MsgMint {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(10000u128, "alloyed").into()),
                mint_to_address: "addr1".to_string()
            })),
    )]
    #[case(
        Entrypoint::Exec,
        SwapToAlloyedConstraint::ExactOut {
            token_in_denom: "denom1",
            token_in_max_amount: Uint128::new(100),
            token_out_amount: Uint128::new(10000u128)
        },
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgMint {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(10000u128, "alloyed").into()),
                mint_to_address: "addr1".to_string()
            })),
    )]
    #[case(
        Entrypoint::Sudo,
        SwapToAlloyedConstraint::ExactOut {
            token_in_denom: "denom1",
            token_in_max_amount: Uint128::new(100),
            token_out_amount: Uint128::new(10000u128)
        },
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .set_data(to_json_binary(&SwapExactAmountOutResponseData {
                token_in_amount: Uint128::new(100u128)
            }).unwrap())
            .add_message(MsgMint {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(10000u128, "alloyed").into()),
                mint_to_address: "addr1".to_string()
            })),
    )]
    fn test_swap_tokens_to_alloyed_asset(
        #[case] entrypoint: Entrypoint,
        #[case] constraint: SwapToAlloyedConstraint,
        #[case] mint_to_address: Addr,
        #[case] expected_res: Result<Response, ContractError>,
    ) {
        let mut deps = mock_dependencies();
        let transmuter = Transmuter::new();

        transmuter
            .rebalancing_incentive_config
            .save(&mut deps.storage, &RebalancingIncentiveConfig::default())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_alloyed_denom(&mut deps.storage, &"alloyed".to_string())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_normalization_factor(&mut deps.storage, 100u128.into())
            .unwrap();

        transmuter
            .pool
            .save(
                &mut deps.storage,
                &TransmuterPool {
                    pool_assets: vec![
                        Asset::new(Uint128::from(1000u128), "denom1", 1u128).unwrap(),
                        Asset::new(Uint128::from(1000u128), "denom2", 10u128).unwrap(),
                    ],
                    asset_groups: BTreeMap::new(),
                },
            )
            .unwrap();

        let res = transmuter.swap_tokens_to_alloyed_asset(
            entrypoint,
            constraint,
            mint_to_address,
            deps.as_mut(),
            mock_env(),
        );

        assert_eq!(res, expected_res);
    }

    #[rstest]
    #[case(
        Entrypoint::Exec,
        SwapFromAlloyedConstraint::ExactIn {
            token_out_denom: "denom1",
            token_out_min_amount: Uint128::from(1u128),
            token_in_amount: Uint128::from(100u128),
        },
        BurnTarget::SenderAccount,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgBurn {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(100u128, "alloyed").into()),
                burn_from_address: "addr1".to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(1u128, "denom1")]
            }))
    )]
    #[case(
        Entrypoint::Sudo,
        SwapFromAlloyedConstraint::ExactIn {
            token_out_denom: "denom1",
            token_out_min_amount: Uint128::from(1u128),
            token_in_amount: Uint128::from(100u128),
        },
        BurnTarget::SentFunds,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgBurn {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(100u128, "alloyed").into()),
                burn_from_address: MOCK_CONTRACT_ADDR.to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(1u128, "denom1")]
            })
            .set_data(to_json_binary(&SwapExactAmountInResponseData {
                token_out_amount: Uint128::from(1u128)
            }).unwrap()))
    )]
    #[case(
        Entrypoint::Exec,
        SwapFromAlloyedConstraint::ExactOut {
            tokens_out: &[coin(1u128, "denom1")],
            token_in_max_amount: Uint128::from(100u128),
        },
        BurnTarget::SenderAccount,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgBurn {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(100u128, "alloyed").into()),
                burn_from_address: "addr1".to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(1u128, "denom1")]
            }))
    )]
    #[case(
        Entrypoint::Sudo,
        SwapFromAlloyedConstraint::ExactOut {
            tokens_out: &[coin(1u128, "denom1")],
            token_in_max_amount: Uint128::from(100u128),
        },
        BurnTarget::SentFunds,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgBurn {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(100u128, "alloyed").into()),
                burn_from_address: MOCK_CONTRACT_ADDR.to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(1u128, "denom1")]
            })
            .set_data(to_json_binary(&SwapExactAmountOutResponseData {
                token_in_amount: Uint128::from(100u128)
            }).unwrap()))
    )]
    fn test_swap_alloyed_asset_to_tokens(
        #[case] entrypoint: Entrypoint,
        #[case] constraint: SwapFromAlloyedConstraint,
        #[case] burn_target: BurnTarget,
        #[case] sender: Addr,
        #[case] expected_res: Result<Response, ContractError>,
    ) {
        let alloyed_holder = match burn_target {
            BurnTarget::SenderAccount => sender.to_string(),
            BurnTarget::SentFunds => MOCK_CONTRACT_ADDR.to_string(),
        };

        let mut deps = cosmwasm_std::testing::mock_dependencies_with_balances(&[(
            alloyed_holder.as_str(),
            &[coin(110000000000000u128, "alloyed")],
        )]);

        let transmuter = Transmuter::new();

        transmuter
            .rebalancing_incentive_config
            .save(&mut deps.storage, &RebalancingIncentiveConfig::default())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_alloyed_denom(&mut deps.storage, &"alloyed".to_string())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_normalization_factor(&mut deps.storage, 100u128.into())
            .unwrap();

        transmuter
            .pool
            .save(
                &mut deps.storage,
                &TransmuterPool {
                    pool_assets: vec![
                        Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(),
                        Asset::new(Uint128::from(1000000000000u128), "denom2", 10u128).unwrap(),
                    ],
                    asset_groups: BTreeMap::new(),
                },
            )
            .unwrap();

        let res = transmuter.swap_alloyed_asset_to_tokens(
            entrypoint,
            constraint,
            burn_target,
            sender,
            deps.as_mut(),
            mock_env(),
        );

        assert_eq!(res, expected_res);

        let pool = transmuter.pool.load(&deps.storage).unwrap();

        for denom in ["denom1", "denom2"] {
            assert!(pool.has_denom(denom))
        }
    }

    #[rstest]
    #[case(
        Entrypoint::Sudo,
        SwapFromAlloyedConstraint::ExactOut {
            tokens_out: &[coin(1000000000000u128, "denom1")],
            token_in_max_amount: Uint128::from(100000000000000u128),
        },
        vec!["denom1"],
        vec!["denom1"],
        BurnTarget::SentFunds,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgBurn {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(100000000000000u128, "alloyed").into()),
                burn_from_address: MOCK_CONTRACT_ADDR.to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(1000000000000u128, "denom1")]
            })
            .set_data(to_json_binary(&SwapExactAmountOutResponseData {
                token_in_amount: Uint128::from(100000000000000u128)
            }).unwrap()))
    )]
    #[case(
        Entrypoint::Sudo,
        SwapFromAlloyedConstraint::ExactIn {
            token_out_denom: "denom1",
            token_out_min_amount: 1000000000000u128.into(),
            token_in_amount: 100000000000000u128.into(),
        },
        vec!["denom1"],
        vec!["denom1"],
        BurnTarget::SentFunds,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgBurn {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(100000000000000u128, "alloyed").into()),
                burn_from_address: MOCK_CONTRACT_ADDR.to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(1000000000000u128, "denom1")]
            })
            .set_data(to_json_binary(&SwapExactAmountInResponseData {
                token_out_amount: 1000000000000u128.into(),
            }).unwrap()))
    )]
    #[case(
        Entrypoint::Exec,
        SwapFromAlloyedConstraint::ExactIn {
            token_out_denom: "denom1",
            token_out_min_amount: 1000000000000u128.into(),
            token_in_amount: 100000000000000u128.into(),
        },
        vec!["denom1"],
        vec!["denom1"],
        BurnTarget::SenderAccount,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgBurn {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(100000000000000u128, "alloyed").into()),
                burn_from_address: "addr1".to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(1000000000000u128, "denom1")]
            }))
    )]
    #[case(
        Entrypoint::Exec,
        SwapFromAlloyedConstraint::ExactOut {
            tokens_out: &[coin(1000000000000u128, "denom1")],
            token_in_max_amount: Uint128::from(100000000000000u128),
        },
        vec!["denom1"],
        vec!["denom1"],
        BurnTarget::SenderAccount,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgBurn {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(100000000000000u128, "alloyed").into()),
                burn_from_address: "addr1".to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(1000000000000u128, "denom1")]
            }))
    )]
    #[case(
        Entrypoint::Sudo,
        SwapFromAlloyedConstraint::ExactOut {
            tokens_out: &[coin(1000000000000u128, "denom1"), coin(1000000000000u128, "denom2")],
            token_in_max_amount: Uint128::from(110000000000000u128),
        },
        vec!["denom1", "denom2"],
        vec!["denom1", "denom2"],
        BurnTarget::SentFunds,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgBurn {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(110000000000000u128, "alloyed").into()),
                burn_from_address: MOCK_CONTRACT_ADDR.to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(1000000000000u128, "denom1"), coin(1000000000000u128, "denom2")]
            })
            .set_data(to_json_binary(&SwapExactAmountOutResponseData {
                token_in_amount: Uint128::from(110000000000000u128),
            }).unwrap()))
    )]
    #[case(
        Entrypoint::Sudo,
        SwapFromAlloyedConstraint::ExactOut {
            tokens_out: &[coin(1000000000000u128, "denom1"), coin(500000000000u128, "denom2")],
            token_in_max_amount: Uint128::from(105000000000000u128),
        },
        vec!["denom1", "denom2"],
        vec!["denom1"],
        BurnTarget::SentFunds,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgBurn {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(105000000000000u128, "alloyed").into()),
                burn_from_address: MOCK_CONTRACT_ADDR.to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(1000000000000u128, "denom1"), coin(500000000000u128, "denom2")],
            })
            .set_data(to_json_binary(&SwapExactAmountOutResponseData {
                token_in_amount: Uint128::from(105000000000000u128),
            }).unwrap()))
    )]
    fn test_swap_alloyed_asset_to_tokens_with_corrupted_assets(
        #[case] entrypoint: Entrypoint,
        #[case] constraint: SwapFromAlloyedConstraint,
        #[case] corrupted_denoms: Vec<&str>,
        #[case] removed_denoms: Vec<&str>,
        #[case] burn_target: BurnTarget,
        #[case] sender: Addr,
        #[case] expected_res: Result<Response, ContractError>,
    ) {
        let alloyed_holder = match burn_target {
            BurnTarget::SenderAccount => sender.to_string(),
            BurnTarget::SentFunds => MOCK_CONTRACT_ADDR.to_string(),
        };

        let mut deps = cosmwasm_std::testing::mock_dependencies_with_balances(&[(
            alloyed_holder.as_str(),
            &[coin(210000000000000u128, "alloyed")],
        )]);

        let transmuter = Transmuter::new();

        transmuter
            .rebalancing_incentive_config
            .save(&mut deps.storage, &RebalancingIncentiveConfig::default())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_alloyed_denom(&mut deps.storage, &"alloyed".to_string())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_normalization_factor(&mut deps.storage, 100u128.into())
            .unwrap();

        let mut pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(), // 1000000000000 * 100
                Asset::new(Uint128::from(1000000000000u128), "denom2", 10u128).unwrap(), // 1000000000000 * 10
                Asset::new(Uint128::from(1000000000000u128), "denom3", 1u128).unwrap(), // 1000000000000 * 100
            ],
            asset_groups: BTreeMap::new(),
        };

        let all_denoms = pool
            .clone()
            .pool_assets
            .into_iter()
            .map(|asset| asset.denom().to_string())
            .collect::<Vec<String>>();

        for denom in corrupted_denoms {
            pool.mark_corrupted_asset(denom).unwrap();
        }

        transmuter.pool.save(&mut deps.storage, &pool).unwrap();

        for denom in all_denoms.clone() {
            transmuter
                .limiters
                .register(
                    &mut deps.storage,
                    Scope::denom(denom.as_str()),
                    "static",
                    LimiterParams::StaticLimiter {
                        upper_limit: Decimal::percent(100),
                    },
                )
                .unwrap();
        }

        let res = transmuter.swap_alloyed_asset_to_tokens(
            entrypoint,
            constraint,
            burn_target,
            sender,
            deps.as_mut(),
            mock_env(),
        );

        assert_eq!(res, expected_res);

        // all drained denoms that are corrupted should not be in the pool
        let pool = transmuter.pool.load(&deps.storage).unwrap();

        for denom in all_denoms {
            if removed_denoms.contains(&denom.as_str()) {
                assert!(
                    !pool.has_denom(denom.as_str()),
                    "must not contain {} since it's corrupted and drained",
                    denom
                );

                // limiters should be removed
                assert!(
                    transmuter
                        .limiters
                        .list_limiters_by_scope(&deps.storage, &Scope::denom(denom.as_str()))
                        .unwrap()
                        .is_empty(),
                    "must not contain limiter for {} since it's corrupted and drained",
                    denom
                );
            } else {
                assert!(
                    pool.has_denom(denom.as_str()),
                    "must contain {} since it's not corrupted or not drained",
                    denom
                );

                // limiters should be removed
                assert!(
                    !transmuter
                        .limiters
                        .list_limiters_by_scope(&deps.storage, &Scope::denom(denom.as_str()))
                        .unwrap()
                        .is_empty(),
                    "must contain limiter for {} since it's not corrupted or not drained",
                    denom
                );
            }
        }
    }

    #[test]
    fn test_swap_non_alloyed_exact_amount_in_with_corrupted_assets() {
        let mut deps = mock_dependencies();
        let transmuter = Transmuter::new();

        transmuter
            .rebalancing_incentive_config
            .save(&mut deps.storage, &RebalancingIncentiveConfig::default())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_alloyed_denom(&mut deps.storage, &"alloyed".to_string())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_normalization_factor(&mut deps.storage, 100u128.into())
            .unwrap();

        let mut pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(), // 1000000000000 * 100
                Asset::new(Uint128::from(1000000000000u128), "denom2", 10u128).unwrap(), // 1000000000000 * 10
                Asset::new(Uint128::from(1000000000000u128), "denom3", 1u128).unwrap(), // 1000000000000 * 100
            ],
            asset_groups: BTreeMap::new(),
        };

        let all_denoms = pool
            .clone()
            .pool_assets
            .into_iter()
            .map(|asset| asset.denom().to_string())
            .collect::<Vec<_>>();

        pool.mark_corrupted_asset("denom1").unwrap();

        transmuter.pool.save(&mut deps.storage, &pool).unwrap();

        for denom in all_denoms.clone() {
            transmuter
                .limiters
                .register(
                    &mut deps.storage,
                    Scope::denom(denom.as_str()),
                    "static",
                    LimiterParams::StaticLimiter {
                        upper_limit: Decimal::percent(100),
                    },
                )
                .unwrap();
        }

        transmuter
            .swap_non_alloyed_exact_amount_in(
                coin(1000000000000, "denom3"),
                "denom1",
                1000000000000u128.into(),
                deps.api.addr_make("sender"),
                deps.as_mut(),
                mock_env(),
            )
            .unwrap();

        // all drained denoms that are corrupted should not be in the pool
        let pool = transmuter.pool.load(&deps.storage).unwrap();

        let denoms = pool
            .pool_assets
            .into_iter()
            .map(|a| a.denom().to_string())
            .collect_vec();

        assert_eq!(denoms, vec!["denom2", "denom3"]);

        let limiter_denoms = transmuter
            .limiters
            .list_limiters(&deps.storage)
            .unwrap()
            .into_iter()
            .map(|((denom, _), _)| denom)
            .unique()
            .collect_vec();

        assert_eq!(
            limiter_denoms,
            vec![Scope::denom("denom2").key(), Scope::denom("denom3").key()]
        );
    }

    #[test]
    fn test_swap_non_alloyed_exact_amount_out_with_corrupted_assets() {
        let mut deps = mock_dependencies();
        let transmuter = Transmuter::new();

        transmuter
            .rebalancing_incentive_config
            .save(&mut deps.storage, &RebalancingIncentiveConfig::default())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_alloyed_denom(&mut deps.storage, &"alloyed".to_string())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_normalization_factor(&mut deps.storage, 100u128.into())
            .unwrap();

        let mut pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(), // 1000000000000 * 100
                Asset::new(Uint128::from(1000000000000u128), "denom2", 10u128).unwrap(), // 1000000000000 * 10
                Asset::new(Uint128::from(1000000000000u128), "denom3", 1u128).unwrap(), // 1000000000000 * 100
            ],
            asset_groups: BTreeMap::new(),
        };

        let all_denoms = pool
            .clone()
            .pool_assets
            .into_iter()
            .map(|asset| asset.denom().to_string())
            .collect::<Vec<_>>();

        pool.mark_corrupted_asset("denom1").unwrap();

        transmuter.pool.save(&mut deps.storage, &pool).unwrap();

        for denom in all_denoms.clone() {
            transmuter
                .limiters
                .register(
                    &mut deps.storage,
                    Scope::denom(denom.as_str()),
                    "static",
                    LimiterParams::StaticLimiter {
                        upper_limit: Decimal::percent(100),
                    },
                )
                .unwrap();
        }

        transmuter
            .swap_non_alloyed_exact_amount_out(
                "denom3",
                1000000000000u128.into(),
                coin(1000000000000, "denom1"),
                deps.api.addr_make("sender"),
                deps.as_mut(),
                mock_env(),
            )
            .unwrap();

        // all drained denoms that are corrupted should not be in the pool
        let pool = transmuter.pool.load(&deps.storage).unwrap();

        let denoms = pool
            .pool_assets
            .into_iter()
            .map(|a| a.denom().to_string())
            .collect_vec();

        assert_eq!(denoms, vec!["denom2", "denom3"]);

        let limiter_denoms = transmuter
            .limiters
            .list_limiters(&deps.storage)
            .unwrap()
            .into_iter()
            .map(|((denom, _), _)| denom)
            .unique()
            .collect_vec();

        assert_eq!(
            limiter_denoms,
            vec![Scope::denom("denom2").key(), Scope::denom("denom3").key()]
        );
    }

    #[rstest]
    #[case(
        coin(100u128, "denom1"),
        "denom2",
        1000u128,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(1000u128, "denom2")]
            })
            .set_data(to_json_binary(&SwapExactAmountInResponseData {
                token_out_amount: Uint128::from(1000u128)
            }).unwrap()))
    )]
    #[case(
        coin(100u128, "denom2"),
        "denom1",
        10u128,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(10u128, "denom1")]
            })
            .set_data(to_json_binary(&SwapExactAmountInResponseData {
                token_out_amount: Uint128::from(10u128)
            }).unwrap()))
    )]
    #[case(
        coin(100u128, "denom2"),
        "denom1",
        100u128,
        Addr::unchecked("addr1"),
        Err(ContractError::InsufficientTokenOut {
            min_required: 100u128.into(),
            amount_out: 10u128.into()
        })
    )]
    #[case(
        coin(100000000001u128, "denom1"),
        "denom2",
        1000000000010u128,
        Addr::unchecked("addr1"),
        Err(ContractError::InsufficientPoolAsset {
            required: coin(1000000000010u128, "denom2"),
            available: coin(1000000000000u128, "denom2"),
        })
    )]
    fn test_swap_non_alloyed_exact_amount_in(
        #[case] token_in: Coin,
        #[case] token_out_denom: &str,
        #[case] token_out_min_amount: u128,
        #[case] sender: Addr,
        #[case] expected_res: Result<Response, ContractError>,
    ) {
        let mut deps = cosmwasm_std::testing::mock_dependencies_with_balances(&[(
            sender.to_string().as_str(),
            &[coin(2000000000000u128, "alloyed")],
        )]);

        let transmuter = Transmuter::new();

        transmuter
            .rebalancing_incentive_config
            .save(&mut deps.storage, &RebalancingIncentiveConfig::default())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_alloyed_denom(&mut deps.storage, &"alloyed".to_string())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_normalization_factor(&mut deps.storage, 100u128.into())
            .unwrap();

        transmuter
            .pool
            .save(
                &mut deps.storage,
                &TransmuterPool {
                    pool_assets: vec![
                        Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(),
                        Asset::new(Uint128::from(1000000000000u128), "denom2", 10u128).unwrap(),
                    ],
                    asset_groups: BTreeMap::new(),
                },
            )
            .unwrap();

        let res = transmuter.swap_non_alloyed_exact_amount_in(
            token_in.clone(),
            token_out_denom,
            token_out_min_amount.into(),
            sender,
            deps.as_mut(),
            mock_env(),
        );

        assert_eq!(res, expected_res);
    }

    #[rstest]
    #[case(
        "denom1",
        100u128,
        coin(1000u128, "denom2"),
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(1000u128, "denom2")]
            })
            .set_data(to_json_binary(&SwapExactAmountOutResponseData {
                token_in_amount: 100u128.into()
            }).unwrap()))
    )]
    #[case(
        "denom2",
        100u128,
        coin(10u128, "denom1"),
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(10u128, "denom1")]
            })
            .set_data(to_json_binary(&SwapExactAmountOutResponseData {
                token_in_amount: 100u128.into()
            }).unwrap()))
    )]
    #[case(
        "denom2",
        100u128,
        coin(100u128, "denom1"),
        Addr::unchecked("addr1"),
        Err(ContractError::ExcessiveRequiredTokenIn {
            limit: 100u128.into(),
            required: 1000u128.into()
        })
    )]
    #[case(
        "denom1",
        100000000001u128,
        coin(1000000000010u128, "denom2"),
        Addr::unchecked("addr1"),
        Err(ContractError::InsufficientPoolAsset {
            required: coin(1000000000010u128, "denom2"),
            available: coin(1000000000000u128, "denom2"),
        })
    )]
    fn test_swap_non_alloyed_exact_amount_out(
        #[case] token_in_denom: &str,
        #[case] token_in_max_amount: u128,
        #[case] token_out: Coin,
        #[case] sender: Addr,
        #[case] expected_res: Result<Response, ContractError>,
    ) {
        let mut deps = cosmwasm_std::testing::mock_dependencies_with_balances(&[(
            sender.to_string().as_str(),
            &[coin(2000000000000u128, "alloyed")],
        )]);

        let transmuter = Transmuter::new();
        transmuter
            .rebalancing_incentive_config
            .save(&mut deps.storage, &RebalancingIncentiveConfig::default())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_alloyed_denom(&mut deps.storage, &"alloyed".to_string())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_normalization_factor(&mut deps.storage, 100u128.into())
            .unwrap();

        transmuter
            .pool
            .save(
                &mut deps.storage,
                &TransmuterPool {
                    pool_assets: vec![
                        Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(),
                        Asset::new(Uint128::from(1000000000000u128), "denom2", 10u128).unwrap(),
                    ],
                    asset_groups: BTreeMap::new(),
                },
            )
            .unwrap();

        let res = transmuter.swap_non_alloyed_exact_amount_out(
            token_in_denom,
            token_in_max_amount.into(),
            token_out,
            sender,
            deps.as_mut(),
            mock_env(),
        );

        assert_eq!(res, expected_res);
    }

    #[rstest]
    #[case::empty(
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        vec![],
    )]
    #[case::no_prev_asset_weights(
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::from([
            ("eth.axl".to_string(), Decimal::percent(40)),
            ("eth.wh".to_string(), Decimal::percent(40)),
            ("wsteth.axl".to_string(), Decimal::percent(20)),
        ]),
        BTreeMap::new(),
        vec![
            (Scope::denom("eth.axl"), (Decimal::zero(), Decimal::percent(40))),
            (Scope::denom("eth.wh"), (Decimal::zero(), Decimal::percent(40))),
            (Scope::denom("wsteth.axl"), (Decimal::zero(), Decimal::percent(20))),
        ],
    )]
    #[case::no_updated_asset_weights(
        BTreeMap::from([
            ("eth.axl".to_string(), Decimal::percent(20)),
            ("eth.wh".to_string(), Decimal::percent(60)),
            ("wsteth.axl".to_string(), Decimal::percent(20)),
        ]),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        vec![
            (Scope::denom("eth.axl"), (Decimal::percent(20), Decimal::zero())),
            (Scope::denom("eth.wh"), (Decimal::percent(60), Decimal::zero())),
            (Scope::denom("wsteth.axl"), (Decimal::percent(20), Decimal::zero())),
        ],
    )]
    #[case(
        BTreeMap::from([
            ("eth.axl".to_string(), Decimal::percent(20)),
            ("eth.wh".to_string(), Decimal::percent(60)),
            ("wsteth.axl".to_string(), Decimal::percent(20)),
        ]),
        BTreeMap::from([
            ("axelar".to_string(), Decimal::percent(40)),
            ("wormhole".to_string(), Decimal::percent(60)),
        ]),
        BTreeMap::from([
            ("eth.axl".to_string(), Decimal::percent(40)),
            ("eth.wh".to_string(), Decimal::percent(40)),
            ("wsteth.axl".to_string(), Decimal::percent(20)),
        ]),
        BTreeMap::from([
            ("axelar".to_string(), Decimal::percent(60)),
            ("wormhole".to_string(), Decimal::percent(40)),
        ]),
        vec![
            (Scope::denom("eth.axl"), (Decimal::percent(20), Decimal::percent(40))),
            (Scope::denom("eth.wh"), (Decimal::percent(60), Decimal::percent(40))),
            (Scope::denom("wsteth.axl"), (Decimal::percent(20), Decimal::percent(20))),
            (Scope::asset_group("axelar"), (Decimal::percent(40), Decimal::percent(60))),
            (Scope::asset_group("wormhole"), (Decimal::percent(60), Decimal::percent(40))),
        ],
    )]
    fn test_construct_scope_value_pairs(
        #[case] prev_asset_weights: BTreeMap<String, Decimal>,
        #[case] prev_asset_group_weights: BTreeMap<String, Decimal>,
        #[case] updated_asset_weights: BTreeMap<String, Decimal>,
        #[case] updated_asset_group_weights: BTreeMap<String, Decimal>,
        #[case] expected_scope_value_pairs: Vec<(Scope, (Decimal, Decimal))>,
    ) {
        let mut scope_value_pairs = construct_scope_value_pairs(
            prev_asset_weights,
            updated_asset_weights,
            prev_asset_group_weights,
            updated_asset_group_weights,
        )
        .unwrap();

        // assert by disregard order
        scope_value_pairs.sort_by_key(|(scope, _)| scope.key());
        let mut expected_scope_value_pairs = expected_scope_value_pairs;
        expected_scope_value_pairs.sort_by_key(|(scope, _)| scope.key());

        assert_eq!(scope_value_pairs, expected_scope_value_pairs);
    }

    #[test]
    fn test_clean_up_drained_corrupted_denom() {
        let sender = Addr::unchecked("addr1");
        let mut deps = cosmwasm_std::testing::mock_dependencies_with_balances(&[(
            sender.to_string().as_str(),
            &[coin(2000000000000u128, "alloyed")],
        )]);

        let transmuter = Transmuter::new();
        transmuter
            .alloyed_asset
            .set_alloyed_denom(&mut deps.storage, &"alloyed".to_string())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_normalization_factor(&mut deps.storage, 100u128.into())
            .unwrap();

        let init_pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(),
                Asset::new(Uint128::from(1000000000000u128), "denom2", 10u128).unwrap(),
                Asset::new(Uint128::from(1000000000000u128), "denom3", 100u128).unwrap(),
            ],
            asset_groups: BTreeMap::from([(
                "group1".to_string(),
                AssetGroup::new(vec!["denom2".to_string(), "denom3".to_string()]),
            )]),
        };
        transmuter.pool.save(&mut deps.storage, &init_pool).unwrap();

        let rebalancing_incentive_config = RebalancingIncentiveConfig {
            lambda: Decimal::percent(10),
            ideal_balances: vec![
                (
                    Scope::denom("denom1"),
                    IdealBalance::new(Decimal::percent(10), Decimal::percent(50)),
                ),
                (
                    Scope::denom("denom2"),
                    IdealBalance::new(Decimal::percent(10), Decimal::percent(30)),
                ),
                (
                    Scope::denom("denom3"),
                    IdealBalance::new(Decimal::percent(10), Decimal::percent(20)),
                ),
            ]
            .into_iter()
            .collect(),
        };
        transmuter
            .rebalancing_incentive_config
            .save(&mut deps.storage, &rebalancing_incentive_config)
            .unwrap();

        // Register a limiter for the group
        transmuter
            .limiters
            .register(
                &mut deps.storage,
                Scope::asset_group("group1"),
                "limiter1",
                LimiterParams::StaticLimiter {
                    upper_limit: Decimal::one(),
                },
            )
            .unwrap();

        let mut pool = transmuter.pool.load(&deps.storage).unwrap();
        assert_eq!(pool, init_pool);

        // mark denom2 as corrupted
        pool.mark_corrupted_asset("denom2").unwrap();
        transmuter.pool.save(&mut deps.storage, &pool).unwrap();

        // Drain denom2 from the pool
        pool.exit_pool(&[coin(1000000000000u128, "denom2")])
            .unwrap();
        transmuter.pool.save(&mut deps.storage, &pool).unwrap();

        let res = transmuter.clean_up_drained_corrupted_assets(&mut deps.storage, &mut pool);
        assert_eq!(res, Ok(()));

        // Check that the pool remains unchanged except for the drained assets
        let expected_pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(),
                Asset::new(Uint128::from(1000000000000u128), "denom3", 100u128).unwrap(),
            ],
            asset_groups: BTreeMap::from([(
                "group1".to_string(),
                AssetGroup::new(vec!["denom3".to_string()]),
            )]),
        };
        assert_eq!(pool, expected_pool);

        // Check that the limiter for group1 is still registered
        let limiters = transmuter.limiters.list_limiters(&deps.storage).unwrap();
        assert_eq!(limiters.len(), 1);

        // Save the updated pool
        transmuter.pool.save(&mut deps.storage, &pool).unwrap();

        // Drain denom3 from the pool
        pool.exit_pool(&[coin(1000000000000u128, "denom3")])
            .unwrap();

        let res = transmuter.clean_up_drained_corrupted_assets(&mut deps.storage, &mut pool);
        assert_eq!(res, Ok(()));

        // Check that the pool remains unchanged
        let expected_pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(),
                Asset::new(Uint128::zero(), "denom3", 100u128).unwrap(),
            ],
            asset_groups: BTreeMap::from([(
                "group1".to_string(),
                AssetGroup::new(vec!["denom3".to_string()]),
            )]),
        };
        assert_eq!(pool, expected_pool);

        // Check that the ideal balance for group1 is still registered
        let config = transmuter
            .rebalancing_incentive_config
            .load(&deps.storage)
            .unwrap();
        assert_eq!(
            config,
            RebalancingIncentiveConfig {
                lambda: Decimal::percent(10),
                ideal_balances: vec![
                    (
                        Scope::denom("denom1"),
                        IdealBalance::new(Decimal::percent(10), Decimal::percent(50)),
                    ),
                    (
                        Scope::denom("denom3"),
                        IdealBalance::new(Decimal::percent(10), Decimal::percent(20)),
                    ),
                ]
                .into_iter()
                .collect(),
            }
        );
    }

    #[test]
    fn test_clean_up_drained_corrupted_denom_not_corrupted() {
        let sender = Addr::unchecked("addr1");
        let mut deps = cosmwasm_std::testing::mock_dependencies_with_balances(&[(
            sender.to_string().as_str(),
            &[coin(2000000000000u128, "alloyed")],
        )]);

        let transmuter = Transmuter::new();
        transmuter
            .alloyed_asset
            .set_alloyed_denom(&mut deps.storage, &"alloyed".to_string())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_normalization_factor(&mut deps.storage, 100u128.into())
            .unwrap();

        let init_pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(),
                Asset::new(Uint128::from(1000000000000u128), "denom2", 10u128).unwrap(),
                Asset::new(Uint128::from(1000000000000u128), "denom3", 100u128).unwrap(),
            ],
            asset_groups: BTreeMap::from([(
                "group1".to_string(),
                AssetGroup::new(vec!["denom2".to_string(), "denom3".to_string()]),
            )]),
        };
        transmuter.pool.save(&mut deps.storage, &init_pool).unwrap();

        let rebalancing_incentive_config = RebalancingIncentiveConfig {
            lambda: Decimal::percent(10),
            ideal_balances: vec![
                (
                    Scope::denom("denom1"),
                    IdealBalance::new(Decimal::percent(10), Decimal::percent(50)),
                ),
                (
                    Scope::denom("denom2"),
                    IdealBalance::new(Decimal::percent(10), Decimal::percent(30)),
                ),
                (
                    Scope::denom("denom3"),
                    IdealBalance::new(Decimal::percent(10), Decimal::percent(20)),
                ),
            ]
            .into_iter()
            .collect(),
        };
        transmuter
            .rebalancing_incentive_config
            .save(&mut deps.storage, &rebalancing_incentive_config)
            .unwrap();

        // Register a limiter for the group
        transmuter
            .limiters
            .register(
                &mut deps.storage,
                Scope::asset_group("group1"),
                "limiter1",
                LimiterParams::StaticLimiter {
                    upper_limit: Decimal::one(),
                },
            )
            .unwrap();

        let mut pool = transmuter.pool.load(&deps.storage).unwrap();
        assert_eq!(pool, init_pool);

        // Drain denom2 from the pool
        pool.exit_pool(&[coin(1000000000000u128, "denom2")])
            .unwrap();
        transmuter.pool.save(&mut deps.storage, &pool).unwrap();

        let res = transmuter.clean_up_drained_corrupted_assets(&mut deps.storage, &mut pool);
        assert_eq!(res, Ok(()));

        // Check that the pool remains unchanged
        let expected_pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(),
                Asset::new(Uint128::zero(), "denom2", 10u128).unwrap(),
                Asset::new(Uint128::from(1000000000000u128), "denom3", 100u128).unwrap(),
            ],
            asset_groups: BTreeMap::from([(
                "group1".to_string(),
                AssetGroup::new(vec!["denom2".to_string(), "denom3".to_string()]),
            )]),
        };
        assert_eq!(pool, expected_pool);

        // Check that the limiter for group1 is still registered
        let limiters = transmuter.limiters.list_limiters(&deps.storage).unwrap();
        assert_eq!(limiters.len(), 1);

        // Save the updated pool
        transmuter.pool.save(&mut deps.storage, &pool).unwrap();

        // Drain denom3 from the pool
        pool.exit_pool(&[coin(1000000000000u128, "denom3")])
            .unwrap();

        let res = transmuter.clean_up_drained_corrupted_assets(&mut deps.storage, &mut pool);
        assert_eq!(res, Ok(()));

        // Check that the pool remains unchanged except for the drained assets
        let expected_pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(),
                Asset::new(Uint128::zero(), "denom2", 10u128).unwrap(),
                Asset::new(Uint128::zero(), "denom3", 100u128).unwrap(),
            ],
            asset_groups: BTreeMap::from([(
                "group1".to_string(),
                AssetGroup::new(vec!["denom2".to_string(), "denom3".to_string()]),
            )]),
        };
        assert_eq!(pool, expected_pool);

        // Check that the limiter for group1 is still registered
        let limiters = transmuter.limiters.list_limiters(&deps.storage).unwrap();
        assert_eq!(limiters.len(), 1);

        // Check that the ideal balance for group1 is still registered
        let config = transmuter
            .rebalancing_incentive_config
            .load(&deps.storage)
            .unwrap();
        assert_eq!(config, rebalancing_incentive_config);
    }

    #[test]
    fn test_clean_up_drained_corrupted_assets_group() {
        let sender = Addr::unchecked("addr1");
        let mut deps = cosmwasm_std::testing::mock_dependencies_with_balances(&[(
            sender.to_string().as_str(),
            &[coin(2000000000000u128, "alloyed")],
        )]);

        let transmuter = Transmuter::new();
        transmuter
            .alloyed_asset
            .set_alloyed_denom(&mut deps.storage, &"alloyed".to_string())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_normalization_factor(&mut deps.storage, 100u128.into())
            .unwrap();

        let init_pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(),
                Asset::new(Uint128::from(1000000000000u128), "denom2", 10u128).unwrap(),
                Asset::new(Uint128::from(1000000000000u128), "denom3", 100u128).unwrap(),
            ],
            asset_groups: BTreeMap::from([(
                "group1".to_string(),
                AssetGroup::new(vec!["denom2".to_string(), "denom3".to_string()])
                    .mark_as_corrupted()
                    .clone(),
            )]),
        };
        transmuter.pool.save(&mut deps.storage, &init_pool).unwrap();

        let rebalancing_incentive_config = RebalancingIncentiveConfig {
            lambda: Decimal::percent(10),
            ideal_balances: vec![
                (
                    Scope::denom("denom1"),
                    IdealBalance::new(Decimal::percent(10), Decimal::percent(50)),
                ),
                (
                    Scope::denom("denom2"),
                    IdealBalance::new(Decimal::percent(10), Decimal::percent(30)),
                ),
                (
                    Scope::denom("denom3"),
                    IdealBalance::new(Decimal::percent(10), Decimal::percent(20)),
                ),
            ]
            .into_iter()
            .collect(),
        };
        transmuter
            .rebalancing_incentive_config
            .save(&mut deps.storage, &rebalancing_incentive_config)
            .unwrap();

        // Register limiters for group1
        transmuter
            .limiters
            .register(
                &mut deps.storage,
                Scope::asset_group("group1"),
                "1w",
                LimiterParams::StaticLimiter {
                    upper_limit: Decimal::percent(60),
                },
            )
            .unwrap();

        let mut pool = transmuter.pool.load(&deps.storage).unwrap();
        let res = transmuter.clean_up_drained_corrupted_assets(&mut deps.storage, &mut pool);
        assert_eq!(res, Ok(()));

        pool = transmuter.pool.load(&deps.storage).unwrap();
        assert_eq!(pool, init_pool);

        pool.exit_pool(&[coin(1000000000000u128, "denom2")])
            .unwrap();
        transmuter.pool.save(&mut deps.storage, &pool).unwrap();

        let res = transmuter.clean_up_drained_corrupted_assets(&mut deps.storage, &mut pool);
        assert_eq!(res, Ok(()));

        let expected_pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(),
                Asset::new(Uint128::from(1000000000000u128), "denom3", 100u128).unwrap(),
            ],
            asset_groups: BTreeMap::from([(
                "group1".to_string(),
                AssetGroup::new(vec!["denom3".to_string()])
                    .mark_as_corrupted()
                    .clone(),
            )]),
        };
        assert_eq!(pool, expected_pool);

        // Check that the limiter for group1 is still registered
        let limiters = transmuter.limiters.list_limiters(&deps.storage).unwrap();
        assert_eq!(limiters.len(), 1);

        // Save the updated pool
        transmuter.pool.save(&mut deps.storage, &pool).unwrap();

        pool.exit_pool(&[coin(1000000000000u128, "denom3")])
            .unwrap();

        let res = transmuter.clean_up_drained_corrupted_assets(&mut deps.storage, &mut pool);
        assert_eq!(res, Ok(()));

        let expected_pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(),
            ],
            asset_groups: BTreeMap::new(),
        };
        assert_eq!(pool, expected_pool);

        // Check that the limiter for group1 is removed
        let limiters = transmuter.limiters.list_limiters(&deps.storage).unwrap();
        assert_eq!(limiters.len(), 0);

        // Check that the ideal balance for group1 is removed
        let config = transmuter
            .rebalancing_incentive_config
            .load(&deps.storage)
            .unwrap();
        assert_eq!(
            config,
            RebalancingIncentiveConfig {
                lambda: Decimal::percent(10),
                ideal_balances: vec![(
                    Scope::denom("denom1"),
                    IdealBalance::new(Decimal::percent(10), Decimal::percent(50)),
                ),]
                .into_iter()
                .collect(),
            }
        );
    }

    #[test]
    fn test_clean_up_drained_corrupted_assets_group_not_corrupted() {
        let mut deps = mock_dependencies();
        let transmuter = Transmuter::new();

        // Initialize the pool with non-corrupted assets and groups
        let init_pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(),
                Asset::new(Uint128::from(1000000000000u128), "denom2", 10u128).unwrap(),
                Asset::new(Uint128::from(1000000000000u128), "denom3", 100u128).unwrap(),
            ],
            asset_groups: BTreeMap::from([(
                "group1".to_string(),
                AssetGroup::new(vec!["denom2".to_string(), "denom3".to_string()]),
            )]),
        };

        transmuter.pool.save(&mut deps.storage, &init_pool).unwrap();

        let rebalancing_incentive_config = RebalancingIncentiveConfig {
            lambda: Decimal::percent(10),
            ideal_balances: vec![
                (
                    Scope::denom("denom1"),
                    IdealBalance::new(Decimal::percent(10), Decimal::percent(50)),
                ),
                (
                    Scope::denom("denom2"),
                    IdealBalance::new(Decimal::percent(10), Decimal::percent(30)),
                ),
                (
                    Scope::denom("denom3"),
                    IdealBalance::new(Decimal::percent(10), Decimal::percent(20)),
                ),
            ]
            .into_iter()
            .collect(),
        };
        transmuter
            .rebalancing_incentive_config
            .save(&mut deps.storage, &rebalancing_incentive_config)
            .unwrap();

        // Register a limiter for the group
        transmuter
            .limiters
            .register(
                &mut deps.storage,
                Scope::asset_group("group1"),
                "limiter1",
                LimiterParams::StaticLimiter {
                    upper_limit: Decimal::one(),
                },
            )
            .unwrap();

        let mut pool = transmuter.pool.load(&deps.storage).unwrap();
        assert_eq!(pool, init_pool);

        // Drain denom2 from the pool
        pool.exit_pool(&[coin(1000000000000u128, "denom2")])
            .unwrap();
        transmuter.pool.save(&mut deps.storage, &pool).unwrap();

        let res = transmuter.clean_up_drained_corrupted_assets(&mut deps.storage, &mut pool);
        assert_eq!(res, Ok(()));

        // Check that the pool remains unchanged
        let expected_pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(),
                Asset::new(Uint128::zero(), "denom2", 10u128).unwrap(),
                Asset::new(Uint128::from(1000000000000u128), "denom3", 100u128).unwrap(),
            ],
            asset_groups: BTreeMap::from([(
                "group1".to_string(),
                AssetGroup::new(vec!["denom2".to_string(), "denom3".to_string()]),
            )]),
        };
        assert_eq!(pool, expected_pool);

        // Check that the limiter for group1 is still registered
        let limiters = transmuter.limiters.list_limiters(&deps.storage).unwrap();
        assert_eq!(limiters.len(), 1);

        // Save the updated pool
        transmuter.pool.save(&mut deps.storage, &pool).unwrap();

        // Drain denom3 from the pool
        pool.exit_pool(&[coin(1000000000000u128, "denom3")])
            .unwrap();

        let res = transmuter.clean_up_drained_corrupted_assets(&mut deps.storage, &mut pool);
        assert_eq!(res, Ok(()));

        // Check that the pool remains unchanged except for the drained assets
        let expected_pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(),
                Asset::new(Uint128::zero(), "denom2", 10u128).unwrap(),
                Asset::new(Uint128::zero(), "denom3", 100u128).unwrap(),
            ],
            asset_groups: BTreeMap::from([(
                "group1".to_string(),
                AssetGroup::new(vec!["denom2".to_string(), "denom3".to_string()]),
            )]),
        };
        assert_eq!(pool, expected_pool);

        // Check that the limiter for group1 is still registered
        let limiters = transmuter.limiters.list_limiters(&deps.storage).unwrap();
        assert_eq!(limiters.len(), 1);

        // Check that the ideal balance for group1 is still registered
        let config = transmuter
            .rebalancing_incentive_config
            .load(&deps.storage)
            .unwrap();
        assert_eq!(config, rebalancing_incentive_config);
    }

    #[test]
    fn test_rebalancing_incentive_pass() {
        let mut deps = mock_dependencies();
        let transmuter = Transmuter::new();

        let pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(500000000000000u128), "denom1", 1u128).unwrap(),
                Asset::new(Uint128::from(3000000000000000u128), "denom2", 10u128).unwrap(),
                Asset::new(Uint128::from(20000000000000000u128), "denom3", 100u128).unwrap(),
            ],
            asset_groups: BTreeMap::from([(
                "group1".to_string(),
                AssetGroup::new(vec!["denom2".to_string(), "denom3".to_string()]),
            )]),
        };

        transmuter
            .alloyed_asset
            .set_alloyed_denom(&mut deps.storage, &"alloyed".to_string())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_normalization_factor(&mut deps.storage, Uint128::new(1))
            .unwrap();

        transmuter.pool.save(&mut deps.storage, &pool).unwrap();
        transmuter
            .rebalancing_incentive_config
            .save(&mut deps.storage, &RebalancingIncentiveConfig::default())
            .unwrap();
        transmuter
            .incentive_pool
            .save(&mut deps.storage, &IncentivePool::default())
            .unwrap();

        let block_time = Timestamp::from_seconds(1000000);
        let pool = transmuter.pool.load(&deps.storage).unwrap();
        let tokens_in = vec![coin(1000000000000u128, "denom1")];
        let action = transmuter
            .rebalancing_incentive_pass(
                deps.as_mut(),
                block_time,
                &tokens_in,
                "denom2",
                pool.clone(),
                |mut pool| {
                    pool.join_pool(&tokens_in)?;
                    Ok(pool)
                },
            )
            .unwrap();

        assert_eq!(action, RebalancingIncentiveAction::None);

        let incentive_pool = transmuter.incentive_pool.load(&deps.storage).unwrap();
        assert_eq!(incentive_pool, IncentivePool::default());

        let rebalancing_incentive_config = RebalancingIncentiveConfig {
            lambda: Decimal::percent(10),
            ideal_balances: vec![
                (
                    Scope::denom("denom1"),
                    IdealBalance::new(Decimal::percent(10), Decimal::percent(60)),
                ),
                (
                    Scope::asset_group("group1"),
                    IdealBalance::new(Decimal::percent(20), Decimal::percent(60)),
                ),
            ]
            .into_iter()
            .collect(),
        };

        transmuter
            .rebalancing_incentive_config
            .save(&mut deps.storage, &rebalancing_incentive_config)
            .unwrap();

        // move within ideal balance does not incur fee or incentivized
        let tokens_in = vec![coin(100000000000000u128, "denom1")];
        let action = transmuter
            .rebalancing_incentive_pass(
                deps.as_mut(),
                block_time,
                &tokens_in,
                "denom2",
                pool.clone(),
                |mut pool| {
                    pool.join_pool(&tokens_in)?;
                    Ok(pool)
                },
            )
            .unwrap();

        assert_eq!(action, RebalancingIncentiveAction::None);

        let incentive_pool = transmuter.incentive_pool.load(&deps.storage).unwrap();
        assert_eq!(incentive_pool, IncentivePool::default());

        // move out of ideal balance less than lower bound incurs fee
        let tokens_in = vec![coin(500000000000000u128, "alloyed")]; // doesn't matter for this case
        let action = transmuter
            .rebalancing_incentive_pass(
                deps.as_mut(),
                block_time,
                &tokens_in,
                "denom1",
                pool.clone(),
                |mut pool| {
                    pool.exit_pool(&[coin(500000000000000u128, "denom1")])?;

                    Ok(pool)
                },
            )
            .unwrap();

        // ceil(500000000000000 * 0.707106781186547524 * 10%) = 35355339059328
        assert_eq!(
            action,
            RebalancingIncentiveAction::CollectFee(
                coins(35355339059328u128, "alloyed").try_into().unwrap()
            )
        );

        let incentive_pool = transmuter.incentive_pool.load(&deps.storage).unwrap();
        assert_eq!(
            incentive_pool,
            IncentivePool {
                balances: vec![(
                    "alloyed".to_string(),
                    IncentivePoolBalance::new(35355339059328u128)
                )]
                .into_iter()
                .collect(),
            }
        );

        // more token ins
        let tokens_in = vec![
            coin(500000000000000u128, "denom1"),
            coin(100000000000000u128, "denom2"),
        ];
        let action = transmuter
            .rebalancing_incentive_pass(
                deps.as_mut(),
                block_time,
                &tokens_in,
                "alloyed",
                pool.clone(),
                |mut pool| {
                    pool.join_pool(&tokens_in)?;

                    Ok(pool)
                },
            )
            .unwrap();

        // ceil(500000000000000 * 0.012110214464277872 * 10%) = 605510723214u128
        // ceil(100000000000000 * 0.012110214464277872 * 10%) = 121102144643u128
        assert_eq!(
            action,
            RebalancingIncentiveAction::CollectFee(
                vec![
                    coin(605510723214u128, "denom1"),
                    coin(121102144643u128, "denom2"),
                ]
                .try_into()
                .unwrap()
            )
        );

        let incentive_pool = transmuter.incentive_pool.load(&deps.storage).unwrap();
        assert_eq!(
            incentive_pool,
            IncentivePool {
                balances: vec![
                    (
                        "alloyed".to_string(),
                        IncentivePoolBalance::new(35355339059328u128)
                    ),
                    (
                        "denom1".to_string(),
                        IncentivePoolBalance::new(605510723214u128)
                    ),
                    (
                        "denom2".to_string(),
                        IncentivePoolBalance::new(121102144643u128)
                    ),
                ]
                .into_iter()
                .collect(),
            }
        );

        let add_more_denom_1_tokens_in = vec![coin(1000000000000000u128, "denom1")];
        let add_more_denom_1 = |mut pool: TransmuterPool| {
            pool.join_pool(&add_more_denom_1_tokens_in)?;
            Ok(pool)
        };

        // move out of ideal balance more than upper bound incurs fee
        let action = transmuter
            .rebalancing_incentive_pass(
                deps.as_mut(),
                block_time,
                &add_more_denom_1_tokens_in,
                "alloyed",
                pool.clone(),
                add_more_denom_1,
            )
            .unwrap();

        assert_eq!(
            action,
            RebalancingIncentiveAction::CollectFee(
                coins(7031250000000u128, "denom1").try_into().unwrap()
            )
        );

        let incentive_pool = transmuter.incentive_pool.load(&deps.storage).unwrap();
        assert_eq!(
            incentive_pool,
            IncentivePool {
                balances: vec![
                    (
                        "alloyed".to_string(),
                        IncentivePoolBalance::new(35355339059328u128)
                    ),
                    (
                        "denom1".to_string(),
                        IncentivePoolBalance::new(605510723214u128 + 7031250000000u128)
                    ),
                    (
                        "denom2".to_string(),
                        IncentivePoolBalance::new(121102144643u128)
                    ),
                ]
                .into_iter()
                .collect(),
            }
        );

        // having limiter makes the fee more dramatic
        transmuter
            .limiters
            .register(
                &mut deps.storage,
                Scope::denom("denom1"),
                "limiter1",
                LimiterParams::StaticLimiter {
                    upper_limit: Decimal::percent(75),
                },
            )
            .unwrap();
        let action = transmuter
            .rebalancing_incentive_pass(
                deps.as_mut(),
                block_time,
                &add_more_denom_1_tokens_in,
                "alloyed",
                pool.clone(),
                add_more_denom_1,
            )
            .unwrap();

        assert_eq!(
            action,
            RebalancingIncentiveAction::CollectFee(
                coins(50000000000000u128, "denom1").try_into().unwrap()
            )
        );

        let incentive_pool = transmuter.incentive_pool.load(&deps.storage).unwrap();
        assert_eq!(
            incentive_pool,
            IncentivePool {
                balances: vec![
                    (
                        "alloyed".to_string(),
                        IncentivePoolBalance::new(35355339059328u128)
                    ),
                    (
                        "denom1".to_string(),
                        IncentivePoolBalance::new(
                            605510723214u128 + 7031250000000u128 + 50000000000000u128
                        )
                    ),
                    (
                        "denom2".to_string(),
                        IncentivePoolBalance::new(121102144643u128)
                    ),
                ]
                .into_iter()
                .collect(),
            }
        );

        // reset incentive pool
        transmuter
            .incentive_pool
            .save(&mut deps.storage, &IncentivePool::default())
            .unwrap();

        // try collect fee on moves that equal to upcoming incentivize move
        let non_ideal_balance_pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(500000000000000u128), "denom1", 1u128).unwrap(),
                Asset::new(Uint128::from(3000000000000000u128), "denom2", 10u128).unwrap(),
                Asset::new(
                    Uint128::from(1000000000000000u128 + 19000000000000000u128),
                    "denom3",
                    100u128,
                )
                .unwrap(),
            ],
            asset_groups: BTreeMap::from([(
                "group1".to_string(),
                AssetGroup::new(vec!["denom2".to_string(), "denom3".to_string()]),
            )]),
        };
        let tokens_out = vec![coin(19000000000000000u128, "denom3")];
        let action = transmuter
            .rebalancing_incentive_pass(
                deps.as_mut(),
                block_time,
                &coins(190000000000000u128, "alloyed"), // conversion: denom3:alloyed = 100:1
                "denom3",
                non_ideal_balance_pool,
                |mut pool| {
                    pool.exit_pool(&tokens_out)?;
                    Ok(pool)
                },
            )
            .unwrap();

        assert_eq!(
            action,
            RebalancingIncentiveAction::CollectFee(
                vec![coin(126132533998, "alloyed")].try_into().unwrap()
            )
        );

        let incentive_pool = transmuter.incentive_pool.load(&deps.storage).unwrap();
        assert_eq!(
            incentive_pool,
            IncentivePool {
                balances: vec![(
                    "alloyed".to_string(),
                    IncentivePoolBalance::new(126132533998u128)
                ),]
                .into_iter()
                .collect(),
            }
        );

        // move into denom ideal balance is incentivized
        let non_ideal_balance_pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(500000000000000u128), "denom1", 1u128).unwrap(),
                Asset::new(Uint128::from(3000000000000000u128), "denom2", 10u128).unwrap(),
                Asset::new(Uint128::from(1000000000000000u128), "denom3", 100u128).unwrap(),
            ],
            asset_groups: BTreeMap::from([(
                "group1".to_string(),
                AssetGroup::new(vec!["denom2".to_string(), "denom3".to_string()]),
            )]),
        };

        let tokens_in = vec![coin(19000000000000000u128, "denom3")];
        let action = transmuter
            .rebalancing_incentive_pass(
                deps.as_mut(),
                block_time,
                &tokens_in,
                "alloyed",
                non_ideal_balance_pool,
                |mut pool| {
                    pool.join_pool(&tokens_in)?;
                    Ok(pool)
                },
            )
            .unwrap();

        // lambda * f * amount_in = 10% * 0.006638554420904599 * 19000000000000000 = 12613253399718.7381
        // round down and since std norm = 100 and alloyed norm = 1, actual incentive = 12613253399718 / 100 = 126132533997
        assert_eq!(
            action,
            RebalancingIncentiveAction::DistributeIncentive(coin(126132533997, "alloyed"))
        );

        let incentive_pool = transmuter.incentive_pool.load(&deps.storage).unwrap();
        assert_eq!(
            incentive_pool,
            IncentivePool {
                balances: vec![("alloyed".to_string(), IncentivePoolBalance::new(1u128))]
                    .into_iter()
                    .collect(),
            }
        );

        // make incentive pool contains old lambda
        transmuter
            .incentive_pool
            .save(
                &mut deps.storage,
                &IncentivePool {
                    balances: vec![(
                        "denom2".to_string(),
                        IncentivePoolBalance {
                            historical_lambda: Decimal::percent(20),
                            historical_lambda_collected_balance: Uint256::from(1000000u128),
                            current_lambda_collected_balance: Uint256::from(
                                1000000000000000000000000u128,
                            ),
                        },
                    )]
                    .into_iter()
                    .collect(),
                },
            )
            .unwrap();

        let non_ideal_balance_pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(100000000000000u128), "denom1", 1u128).unwrap(),
                Asset::new(Uint128::from(7000000000000000u128), "denom2", 10u128).unwrap(),
                Asset::new(Uint128::from(20000000000000000u128), "denom3", 100u128).unwrap(),
            ],
            asset_groups: BTreeMap::from([(
                "group1".to_string(),
                AssetGroup::new(vec!["denom2".to_string(), "denom3".to_string()]),
            )]),
        };

        let action = transmuter
            .rebalancing_incentive_pass(
                deps.as_mut(),
                block_time,
                &coins(400000000000000u128, "denom1"),
                "denom2",
                non_ideal_balance_pool,
                |mut pool| {
                    pool.transmute(
                        AmountConstraint::ExactIn(400000000000000u128.into()),
                        "denom1",
                        "denom2",
                    )?;
                    Ok(pool)
                },
            )
            .unwrap();

        // incentive_hist = min(0.2 * 0.28125 * 400000000000000 * 10, 1000000) = 1000000
        // rem = 0.2 * 0.28125 * 400000000000000 * 10 - 1000000 = 224,999,999,000,000
        // incentive_curr = min(224,999,999,000,000 * 0.1 / 0.2, 1000000000000000000000000) = 112,499,999,500,000
        // incentive = 112,499,999,500,000 + 1000000 = 112,500,000,500,000
        assert_eq!(
            action,
            RebalancingIncentiveAction::DistributeIncentive(coin(112500000500000u128, "denom2"))
        );
    }

    // TODO: test the following with rebalancing incentive pass (x (incentive, fee))
    // swap_tokens_to_alloyed_assets
    // swap_alloyed_asset_to_token
    // swap_non_alloyed_exact_amount_in
    // swap_non_alloyed_exact_amount_out

    type MockDeps = OwnedDeps<MockStorage, MockApi, MockQuerier, Empty>;

    #[rstest]
    fn test_swap_between_token_and_alloyed_exact_in_with_incentive_mechanism_balanced_start(
        #[from(deps_with_incentive_config_balanced)] mut deps: MockDeps,
    ) {
        let transmuter = Transmuter::new();
        let mint_to_address = deps.api.addr_make("mint_to_address");
        let env = mock_env();

        // swap expect fee
        let amount_in = 1000000000000000u128;
        let expected_fee_amount = 7031250000000u128;
        let amount_in_after_fee = amount_in - expected_fee_amount;
        let expected_out_amount = (amount_in - expected_fee_amount) * 10u128;

        let res = transmuter
            .swap_tokens_to_alloyed_asset(
                Entrypoint::Sudo,
                SwapToAlloyedConstraint::ExactIn {
                    tokens_in: &coins(amount_in, "denom1"),
                    token_out_min_amount: expected_out_amount.into(),
                },
                mint_to_address.clone(),
                deps.as_mut(),
                env.clone(),
            )
            .unwrap();

        assert_eq!(
            res.messages,
            vec![SubMsg::new(MsgMint {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(expected_out_amount, "alloyed").into()),
                mint_to_address: mint_to_address.to_string(),
            }),]
        );

        assert_eq!(
            from_json::<SwapExactAmountInResponseData>(res.data.unwrap()).unwrap(),
            SwapExactAmountInResponseData {
                token_out_amount: expected_out_amount.into(),
            }
        );

        let pool = transmuter.pool.load(&deps.storage).unwrap();
        assert_eq!(
            pool.pool_assets,
            vec![
                Asset::new(
                    Uint128::from(500000000000000u128 + amount_in_after_fee),
                    "denom1",
                    1u128
                )
                .unwrap(),
                Asset::new(Uint128::from(3000000000000000u128), "denom2", 10u128).unwrap(),
                Asset::new(Uint128::from(20000000000000000u128), "denom3", 100u128).unwrap(),
            ]
        );

        let incentive_pool = transmuter.incentive_pool.load(&deps.storage).unwrap();
        assert_eq!(
            incentive_pool,
            IncentivePool {
                balances: vec![(
                    "denom1".to_string(),
                    IncentivePoolBalance::new(expected_fee_amount)
                )]
                .into_iter()
                .collect(),
            }
        );

        deps.querier
            .bank
            .update_balance(MOCK_CONTRACT_ADDR, coins(expected_out_amount, "alloyed"));

        // // swap expect incentive
        // // incentive diff due to fee calculation happens before actual pool update and reduce the token in,
        // // thus the actual pool movement is actually smaller than one used to calculate fee
        let incentive_diff = 131303841126u128;
        let amount_in = amount_in_after_fee * 10u128;
        let expected_incentive_amount = expected_fee_amount - incentive_diff;
        let expected_out_amount = amount_in_after_fee + expected_incentive_amount;
        let sender = mint_to_address.clone();

        let res = transmuter
            .swap_alloyed_asset_to_tokens(
                Entrypoint::Sudo,
                SwapFromAlloyedConstraint::ExactIn {
                    token_out_denom: "denom1",
                    token_out_min_amount: expected_out_amount.into(),
                    token_in_amount: amount_in.into(),
                },
                BurnTarget::SentFunds,
                sender.clone(),
                deps.as_mut(),
                env.clone(),
            )
            .unwrap();

        let pool = transmuter.pool.load(&deps.storage).unwrap();
        assert_eq!(
            pool.pool_assets,
            vec![
                Asset::new(Uint128::from(500000000000000u128), "denom1", 1u128).unwrap(),
                Asset::new(Uint128::from(3000000000000000u128), "denom2", 10u128).unwrap(),
                Asset::new(Uint128::from(20000000000000000u128), "denom3", 100u128).unwrap(),
            ]
        );

        let incentive_pool = transmuter.incentive_pool.load(&deps.storage).unwrap();
        assert_eq!(
            incentive_pool,
            IncentivePool {
                balances: vec![(
                    "denom1".to_string(),
                    IncentivePoolBalance::new(incentive_diff)
                )]
                .into_iter()
                .collect(),
            }
        );

        assert_eq!(
            from_json::<SwapExactAmountInResponseData>(res.data.unwrap()).unwrap(),
            SwapExactAmountInResponseData {
                token_out_amount: expected_out_amount.into(),
            }
        );

        assert_eq!(
            res.messages,
            vec![
                SubMsg::new(MsgBurn {
                    sender: MOCK_CONTRACT_ADDR.to_string(),
                    amount: Some(coin(amount_in, "alloyed").into()),
                    burn_from_address: MOCK_CONTRACT_ADDR.to_string(),
                }),
                SubMsg::new(BankMsg::Send {
                    to_address: sender.to_string(),
                    amount: vec![coin(expected_out_amount, "denom1")],
                })
            ]
        );
    }

    #[rstest]
    fn test_swap_between_token_and_alloyed_exact_out_with_incentive_mechanism_balanced_start(
        #[from(deps_with_incentive_config_balanced)] mut deps: MockDeps,
    ) {
        let transmuter = Transmuter::new();
        let mint_to_address = deps.api.addr_make("mint_to_address");
        let env = mock_env();

        // swap expect fee
        let amount_in = 1000000000000000u128;
        let amount_out = 1000000000000000u128 * 10u128;
        let expected_fee_amount = 7031250000000u128;
        let amount_in_after_fee = amount_in - expected_fee_amount;
        let expected_out_amount = amount_out - (expected_fee_amount * 10u128);

        let res = transmuter
            .swap_tokens_to_alloyed_asset(
                Entrypoint::Sudo,
                SwapToAlloyedConstraint::ExactOut {
                    token_in_denom: "denom1",
                    token_in_max_amount: amount_in_after_fee.into(),
                    token_out_amount: amount_out.into(),
                },
                mint_to_address.clone(),
                deps.as_mut(),
                env.clone(),
            )
            .unwrap();

        assert_eq!(
            res.messages,
            vec![SubMsg::new(MsgMint {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(expected_out_amount, "alloyed").into()),
                mint_to_address: mint_to_address.to_string(),
            }),]
        );

        assert_eq!(
            from_json::<SwapExactAmountOutResponseData>(res.data.unwrap()).unwrap(),
            SwapExactAmountOutResponseData {
                token_in_amount: amount_in_after_fee.into(),
            }
        );

        let pool = transmuter.pool.load(&deps.storage).unwrap();
        assert_eq!(
            pool.pool_assets,
            vec![
                Asset::new(
                    Uint128::from(500000000000000u128 + amount_in_after_fee),
                    "denom1",
                    1u128
                )
                .unwrap(),
                Asset::new(Uint128::from(3000000000000000u128), "denom2", 10u128).unwrap(),
                Asset::new(Uint128::from(20000000000000000u128), "denom3", 100u128).unwrap(),
            ]
        );

        let incentive_pool = transmuter.incentive_pool.load(&deps.storage).unwrap();
        assert_eq!(
            incentive_pool,
            IncentivePool {
                balances: vec![(
                    "denom1".to_string(),
                    IncentivePoolBalance::new(expected_fee_amount)
                )]
                .into_iter()
                .collect(),
            }
        );

        deps.querier
            .bank
            .update_balance(MOCK_CONTRACT_ADDR, coins(expected_out_amount, "alloyed"));

        // swap expect incentive
        // incentive diff due to fee calculation happens before actual pool update and reduce the token in,
        // thus the actual pool movement is actually smaller than one used to calculate fee
        let incentive_diff = 131303841126u128;
        let amount_in = amount_in_after_fee * 10u128;
        let input_amount_out = amount_in_after_fee;
        let expected_incentive_amount = expected_fee_amount - incentive_diff;
        let expected_out_amount = input_amount_out + expected_incentive_amount;
        let sender = mint_to_address.clone();

        let res = transmuter
            .swap_alloyed_asset_to_tokens(
                Entrypoint::Sudo,
                SwapFromAlloyedConstraint::ExactOut {
                    tokens_out: &coins(input_amount_out, "denom1"),
                    token_in_max_amount: amount_in.into(),
                },
                BurnTarget::SentFunds,
                sender.clone(),
                deps.as_mut(),
                env.clone(),
            )
            .unwrap();

        let pool = transmuter.pool.load(&deps.storage).unwrap();
        assert_eq!(
            pool.pool_assets,
            vec![
                Asset::new(Uint128::from(500000000000000u128), "denom1", 1u128).unwrap(),
                Asset::new(Uint128::from(3000000000000000u128), "denom2", 10u128).unwrap(),
                Asset::new(Uint128::from(20000000000000000u128), "denom3", 100u128).unwrap(),
            ]
        );

        let incentive_pool = transmuter.incentive_pool.load(&deps.storage).unwrap();
        assert_eq!(
            incentive_pool,
            IncentivePool {
                balances: vec![(
                    "denom1".to_string(),
                    IncentivePoolBalance::new(incentive_diff)
                )]
                .into_iter()
                .collect(),
            }
        );

        assert_eq!(
            from_json::<SwapExactAmountOutResponseData>(res.data.unwrap()).unwrap(),
            SwapExactAmountOutResponseData {
                token_in_amount: amount_in.into(),
            }
        );

        assert_eq!(
            res.messages,
            vec![
                SubMsg::new(MsgBurn {
                    sender: MOCK_CONTRACT_ADDR.to_string(),
                    amount: Some(coin(amount_in, "alloyed").into()),
                    burn_from_address: MOCK_CONTRACT_ADDR.to_string(),
                }),
                SubMsg::new(BankMsg::Send {
                    to_address: sender.to_string(),
                    amount: vec![coin(expected_out_amount, "denom1")],
                })
            ]
        );
    }

    fn deps_with_incentive_config(pool_assets: Vec<Asset>) -> MockDeps {
        let mut deps = mock_dependencies();
        let transmuter = Transmuter::new();
        transmuter
            .rebalancing_incentive_config
            .save(&mut deps.storage, &RebalancingIncentiveConfig::default())
            .unwrap();

        let pool = TransmuterPool {
            pool_assets,
            asset_groups: BTreeMap::from([(
                "group1".to_string(),
                AssetGroup::new(vec!["denom2".to_string(), "denom3".to_string()]),
            )]),
        };
        transmuter.pool.save(&mut deps.storage, &pool).unwrap();

        transmuter
            .alloyed_asset
            .set_alloyed_denom(&mut deps.storage, &"alloyed".to_string())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_normalization_factor(&mut deps.storage, Uint128::new(10))
            .unwrap();

        let rebalancing_incentive_config = RebalancingIncentiveConfig {
            lambda: Decimal::percent(10),
            ideal_balances: vec![
                (
                    Scope::denom("denom1"),
                    IdealBalance::new(Decimal::percent(10), Decimal::percent(60)),
                ),
                (
                    Scope::asset_group("group1"),
                    IdealBalance::new(Decimal::percent(20), Decimal::percent(60)),
                ),
            ]
            .into_iter()
            .collect(),
        };

        transmuter
            .rebalancing_incentive_config
            .save(&mut deps.storage, &rebalancing_incentive_config)
            .unwrap();

        transmuter
            .incentive_pool
            .save(&mut deps.storage, &IncentivePool::default())
            .unwrap();

        deps
    }

    #[fixture]
    fn deps_with_incentive_config_balanced() -> MockDeps {
        deps_with_incentive_config(vec![
            Asset::new(Uint128::from(500000000000000u128), "denom1", 1u128).unwrap(),
            Asset::new(Uint128::from(3000000000000000u128), "denom2", 10u128).unwrap(),
            Asset::new(Uint128::from(20000000000000000u128), "denom3", 100u128).unwrap(),
        ])
    }

    // TODO:
    // - on exact out, discount with calculated incentive on token in rather than adding to token out
    // - handle multiple token out case
    // - add test for imbalanced start
    //   - test_swap_between_token_and_alloyed_exact_out_with_incentive_mechanism_imbalanced_start
    //   - test_swap_between_token_and_alloyed_exact_in_with_incentive_mechanism_imbalanced_start
    // - add test for swap token <-> token
}
