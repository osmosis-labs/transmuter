use std::collections::{BTreeMap, HashSet};

use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    coin, ensure, ensure_eq, to_json_binary, Addr, BankMsg, Coin, Decimal, Decimal256, Deps,
    DepsMut, Env, Response, StdError, Storage, Timestamp, Uint128, Uint256,
};
use osmosis_std::types::osmosis::tokenfactory::v1beta1::{MsgBurn, MsgMint};
use serde::Serialize;
use transmuter_math::rebalancing_incentive::{
    calculate_impact_factor, calculate_rebalancing_fee, ImpactFactor, ImpactFactorParamGroup,
};

use crate::{
    alloyed_asset::{swap_from_alloyed, swap_to_alloyed},
    contract::Transmuter,
    corruptable::Corruptable,
    scope::Scope,
    transmuter_pool::{AmountConstraint, TransmuterPool},
    ContractError,
};

#[derive(Debug, PartialEq, Eq)]
pub enum RebalancingIncentiveAction {
    CollectFee(Vec<Coin>),
    PayIncentive(Coin),
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

        let response = Response::new();

        let (tokens_in, out_amount, response) = match constraint {
            SwapToAlloyedConstraint::ExactIn {
                tokens_in,
                token_out_min_amount,
            } => {
                let tokens_in_with_norm_factor =
                    pool.pair_coins_with_normalization_factor(tokens_in)?;
                let out_amount = swap_to_alloyed::out_amount_via_exact_in(
                    tokens_in_with_norm_factor,
                    token_out_min_amount,
                    self.alloyed_asset.get_normalization_factor(deps.storage)?,
                )?;

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
                let in_amount = swap_to_alloyed::in_amount_via_exact_out(
                    token_in_norm_factor,
                    token_in_max_amount,
                    token_out_amount,
                    self.alloyed_asset.get_normalization_factor(deps.storage)?,
                )?;
                let tokens_in = vec![coin(in_amount.u128(), token_in_denom)];

                let response = set_data_if_sudo(
                    response,
                    &entrypoint,
                    &SwapExactAmountOutResponseData {
                        token_in_amount: in_amount,
                    },
                )?;

                (tokens_in, token_out_amount, response)
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

        let response = Response::new();

        let (in_amount, tokens_out, response) = match constraint {
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
                    self.alloyed_asset.get_normalization_factor(deps.storage)?,
                    token_out_norm_factor,
                    token_out_min_amount,
                )?;

                let response = set_data_if_sudo(
                    response,
                    &entrypoint,
                    &SwapExactAmountInResponseData {
                        token_out_amount: out_amount,
                    },
                )?;

                let tokens_out = vec![coin(out_amount.u128(), token_out_denom)];

                (token_in_amount, tokens_out, response)
            }
            SwapFromAlloyedConstraint::ExactOut {
                tokens_out,
                token_in_max_amount,
            } => {
                let tokens_out_with_norm_factor =
                    pool.pair_coins_with_normalization_factor(tokens_out)?;
                let in_amount = swap_from_alloyed::in_amount_via_exact_out(
                    token_in_max_amount,
                    self.alloyed_asset.get_normalization_factor(deps.storage)?,
                    tokens_out_with_norm_factor,
                )?;

                let response = set_data_if_sudo(
                    response,
                    &entrypoint,
                    &SwapExactAmountOutResponseData {
                        token_in_amount: in_amount,
                    },
                )?;

                (in_amount, tokens_out.to_vec(), response)
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

        let is_force_exit_corrupted_assets = tokens_out.iter().all(|coin| {
            let total_liquidity = pool
                .get_pool_asset_by_denom(&coin.denom)
                .map(|asset| asset.amount())
                .unwrap_or_default();

            let is_redeeming_total_liquidity = coin.amount == total_liquidity;
            let is_under_corrupted_asset_group =
                denoms_in_corrupted_asset_group.contains(&coin.denom);

            is_redeeming_total_liquidity
                && (is_under_corrupted_asset_group || pool.is_corrupted_asset(&coin.denom))
        });

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

        let bank_send_msg = BankMsg::Send {
            to_address: sender.to_string(),
            amount: tokens_out,
        };

        let alloyed_asset_to_burn = coin(
            in_amount.u128(),
            self.alloyed_asset.get_alloyed_denom(deps.storage)?,
        )
        .into();

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
        pool: TransmuterPool,
        run: F,
    ) -> Result<RebalancingIncentiveAction, ContractError>
    where
        F: FnOnce(TransmuterPool) -> Result<TransmuterPool, ContractError>,
    {
        let rebalancing_incentive_config = self.rebalancing_incentive_config.load(deps.storage)?;
        let ideal_balances = rebalancing_incentive_config.ideal_balances();
        let upper_limits = self.limiters.upper_limits(deps.storage, block_time)?;

        let prev_normalized_balance = pool.weights()?.unwrap_or_default(); // TODO: should we handle None case? -> write test with that case

        let pool = run(pool)?;

        let update_normalized_balance = pool.weights()?.unwrap_or_default();

        let implact_factor_param_groups: Vec<ImpactFactorParamGroup> = pool
            .scopes()?
            .into_iter()
            .map(|scope| {
                let ideal_balance = ideal_balances.get(&scope).copied().unwrap_or_default();

                ImpactFactorParamGroup::new(
                    prev_normalized_balance
                        .get(&scope)
                        .copied()
                        .unwrap_or_else(|| Decimal::zero()),
                    update_normalized_balance
                        .get(&scope)
                        .copied()
                        .unwrap_or_else(|| Decimal::zero()),
                    ideal_balance.lower,
                    ideal_balance.upper,
                    upper_limits
                        .get(&scope)
                        .copied()
                        .unwrap_or_else(|| Decimal::one()),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;

        let impact_factor = calculate_impact_factor(&implact_factor_param_groups)?;

        match impact_factor {
            ImpactFactor::Fee(impact_factor) => {
                if tokens_in.is_empty() {
                    return Ok(RebalancingIncentiveAction::None);
                }

                let mut fee_coins = vec![];
                for token_in in tokens_in {
                    let fee_amount: Uint128 = calculate_rebalancing_fee(
                        rebalancing_incentive_config.lambda,
                        impact_factor,
                        token_in.amount,
                    )?
                    .to_uint_ceil()
                    .try_into()?; // safe to convert to Uint128 as it's always less than the token amount

                    let fee_coin = coin(fee_amount.u128(), token_in.denom.clone());
                    fee_coins.push(fee_coin);
                }

                Ok(RebalancingIncentiveAction::CollectFee(fee_coins))
            }
            ImpactFactor::Incentive(_) => {
                todo!()
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
            incentive_pool::IncentivePool,
        },
        transmuter_pool::AssetGroup,
    };

    use super::*;
    use cosmwasm_std::{
        coin, coins,
        testing::{mock_dependencies, mock_env, MOCK_CONTRACT_ADDR},
    };
    use itertools::Itertools;
    use rstest::rstest;

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
                pool.clone(),
                |mut pool| {
                    pool.join_pool(&tokens_in)?;
                    Ok(pool)
                },
            )
            .unwrap();

        assert_eq!(action, RebalancingIncentiveAction::None);

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
                pool.clone(),
                |mut pool| {
                    pool.join_pool(&tokens_in)?;
                    Ok(pool)
                },
            )
            .unwrap();

        assert_eq!(action, RebalancingIncentiveAction::None);

        // move out of ideal balance less than lower bound incurs fee
        let tokens_in = vec![coin(500000000000000u128, "alloyed")]; // doesn't matter for this case
        let action = transmuter
            .rebalancing_incentive_pass(
                deps.as_mut(),
                block_time,
                &tokens_in,
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
            RebalancingIncentiveAction::CollectFee(coins(35355339059328u128, "alloyed"))
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
            RebalancingIncentiveAction::CollectFee(vec![
                coin(605510723214u128, "denom1"),
                coin(121102144643u128, "denom2"),
            ])
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
                pool.clone(),
                add_more_denom_1,
            )
            .unwrap();

        assert_eq!(
            action,
            RebalancingIncentiveAction::CollectFee(coins(7031250000000u128, "denom1"))
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
                pool.clone(),
                add_more_denom_1,
            )
            .unwrap();

        assert_eq!(
            action,
            RebalancingIncentiveAction::CollectFee(coins(50000000000000u128, "denom1"))
        );

        // // move into ideal balance is incentivized
        // let non_ideal_balance_pool = TransmuterPool {
        //     pool_assets: vec![
        //         Asset::new(Uint128::from(500000000000000u128), "denom1", 1u128).unwrap(),
        //         Asset::new(Uint128::from(3000000000000000u128), "denom2", 10u128).unwrap(),
        //         Asset::new(Uint128::from(1000000000000000u128), "denom3", 100u128).unwrap(),
        //     ],
        //     asset_groups: BTreeMap::from([(
        //         "group1".to_string(),
        //         AssetGroup::new(vec!["denom2".to_string(), "denom3".to_string()]),
        //     )]),
        // };

        // let impact_factor = transmuter
        //     .rebalancing_incentive_pass(
        //         deps.as_mut(),
        //         block_time,
        //         non_ideal_balance_pool,
        //         |mut pool| {
        //             pool.join_pool(&[coin(19000000000000000u128, "denom3")])?;
        //             Ok(pool)
        //         },
        //     )
        //     .unwrap();

        // assert_eq!(
        //     impact_factor,
        //     ImpactFactor::Incentive("0.006638554420904599".parse().unwrap())
        // );
    }
}
