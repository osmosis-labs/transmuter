use crate::{
    contract::Transmuter,
    swap::{construct_scope_value_pairs, Adjustment},
    transmuter_pool::TransmuterPool,
    ContractError,
};
use cosmwasm_std::{Addr, Deps, DepsMut, Int256, SignedDecimal256, Uint128};
use std::{cmp::Ordering, collections::BTreeMap};
use transmuter_math::rebalancing::{
    compute_total_effective_adjustment_rate, config::RebalancingConfig, round_adjustment,
};

impl Transmuter {
    pub fn rebalancer_pass<RunPoolOutput, RunPool, RebalancingAdjustment>(
        &self,
        deps: DepsMut,
        pool: TransmuterPool,
        beneficiary: &Addr,
        run_pool: RunPool,
        rebalancing_adjustment: RebalancingAdjustment,
    ) -> Result<(TransmuterPool, RunPoolOutput, Adjustment), ContractError>
    where
        RunPool:
            FnOnce(Deps, TransmuterPool) -> Result<(TransmuterPool, RunPoolOutput), ContractError>,
        RebalancingAdjustment: FnOnce(
            TransmuterPool,
            RunPoolOutput,
            Int256,
        )
            -> Result<(RunPoolOutput, Adjustment), ContractError>,
    {
        let prev_asset_weights = pool.asset_weights()?.unwrap_or_default();
        let prev_asset_group_weights = pool.asset_group_weights()?.unwrap_or_default();

        // total normalized amount of the asset in the pool
        let total_balance_before = pool.normalized_total_balance()?;

        let (pool, output) = run_pool(deps.as_ref(), pool)?;

        // in case one of token in or out is alloyed, balance_total is updated
        let total_balance_after = pool.normalized_total_balance()?;

        let total_normalized_incentive_pool_balance =
            self.total_normalized_incentive_pool_balance(deps.storage, &pool)?;

        let total_incentive_credits = self
            .incentive_pool
            .get_total_incentive_credits(deps.storage)?;

        // available incentive that can be distributed
        let available_incentive =
            total_normalized_incentive_pool_balance.saturating_sub(total_incentive_credits);

        // check limits only if pool assets are not zero, calculate adjustment value
        let mut total_adjustment_rate = SignedDecimal256::zero();
        if let Some(updated_asset_weights) = pool.asset_weights()? {
            if let Some(updated_asset_group_weights) = pool.asset_group_weights()? {
                let scope_value_pairs = construct_scope_value_pairs(
                    prev_asset_weights,
                    updated_asset_weights,
                    prev_asset_group_weights,
                    updated_asset_group_weights,
                )?;

                // find total adjustment reqruied to move to all assets to ideal balance
                let mut total_adjustment_to_ideal_required = SignedDecimal256::zero();
                for (scope, (prev_weight, _)) in scope_value_pairs.clone() {
                    let rebalancing_config =
                        self.rebalancer.get_config_by_scope(deps.storage, &scope)?;
                    if let Some(rebalancing_config) = rebalancing_config {
                        let nearest_ideal_weight =
                            rebalancing_config.nearest_ideal_weight(prev_weight);

                        // find adjustment in order to move weight from current to nearest ideal weight
                        // it will always return 0 or positive value as it moves towards ideal weight
                        let adjustment = compute_total_effective_adjustment_rate(
                            prev_weight,
                            nearest_ideal_weight,
                            rebalancing_config,
                        )?;

                        total_adjustment_to_ideal_required =
                            total_adjustment_to_ideal_required.checked_add(adjustment)?;
                    }
                }

                let total_balance =
                    SignedDecimal256::from_atomics(Int256::from(total_balance_before), 0)?;
                let total_incentive_required_for_rebalance = round_adjustment(
                    total_adjustment_to_ideal_required.checked_mul(total_balance)?,
                )?
                .abs_diff(Int256::zero()); // absolute value

                // incentive pool is unhealthy if total incentive required for rebalance is greater than avaialable incentive pool
                let is_incentive_pool_unhealthy =
                    total_incentive_required_for_rebalance > available_incentive;

                for (scope, (prev_weight, updated_weight)) in scope_value_pairs.clone() {
                    let rebalancing_config =
                        self.rebalancer.get_config_by_scope(deps.storage, &scope)?;
                    if let Some(rebalancing_config) = rebalancing_config {
                        let adjustment = compute_total_effective_adjustment_rate(
                            prev_weight,
                            updated_weight,
                            rebalancing_config.clone(),
                        )?;

                        // raise strained rate to equal to critical if incentive pool is unhealthy and it's a fee case
                        let adjustment = if adjustment < SignedDecimal256::zero() {
                            let rebalancing_config = RebalancingConfig {
                                adjustment_rate_strained: if is_incentive_pool_unhealthy {
                                    rebalancing_config.adjustment_rate_critical
                                } else {
                                    rebalancing_config.adjustment_rate_strained
                                },
                                ..rebalancing_config
                            };

                            compute_total_effective_adjustment_rate(
                                prev_weight,
                                updated_weight,
                                rebalancing_config,
                            )?
                        } else {
                            adjustment
                        };

                        total_adjustment_rate = total_adjustment_rate.checked_add(adjustment)?;
                    }
                }

                // TODO: have a way to skip limit check here
                self.rebalancer
                    .check_limits(deps.storage, scope_value_pairs)?;
            }
        }

        // if total balance is updated, it means that one of token in or out is alloyed
        let total_balance = if total_balance_before != total_balance_after {
            match total_adjustment_rate.cmp(&SignedDecimal256::zero()) {
                // Incentivized case:
                // Incentives are paid from a pool of previously collected fees. It is logical to scale the reward based on the state of the pool *before* the user's helpful contribution.
                // This provides a fair reward relative to the pool's history and prevents a single large, helpful deposit from draining a disproportionate amount of the incentive fund.
                Ordering::Greater => total_balance_before,
                // Fee deduction case:
                // This keeps the incentive pool healthy by ensuring that the incentive is proportional to the pool's size.
                // - For **harmful joins** (liquidity addition), the fee is based on `total_balance_after`. This ensures the penalty is proportional to the new, larger pool size that the user has unbalanced.
                // - For **harmful exits** (liquidity withdrawal), the fee is based on `total_balance_before`. This ensures the penalty is proportional to the state of the pool *before* it was damaged by the withdrawal.
                Ordering::Less => total_balance_before.max(total_balance_after),

                // No adjustment, so this doesn't matter
                Ordering::Equal => Uint128::zero(),
            }
        } else {
            total_balance_before
        };

        let total_balance = SignedDecimal256::from_atomics(Int256::from(total_balance), 0)?;
        let total_adjustment_value = total_adjustment_rate.checked_mul(total_balance)?;

        let (output, adjustment) = rebalancing_adjustment(
            pool.clone(),
            output,
            round_adjustment(total_adjustment_value)?,
        )?;

        match adjustment {
            Adjustment::DeductFee { ref fee } => {
                self.incentive_pool.add_tokens(deps.storage, &fee)?;
            }
            Adjustment::CreditIncentive { incentive } => {
                let mut pool_denom_factors = pool
                    .pool_assets
                    .iter()
                    .map(|asset| (asset.denom().to_string(), asset.normalization_factor()))
                    .collect::<BTreeMap<_, _>>();

                let alloyed_denom = self.alloyed_asset.get_alloyed_denom(deps.storage)?;
                let alloyed_norm_factor =
                    self.alloyed_asset.get_normalization_factor(deps.storage)?;
                pool_denom_factors.insert(alloyed_denom, alloyed_norm_factor);

                self.incentive_pool.credit_incentive(
                    deps.storage,
                    &beneficiary,
                    incentive,
                    &pool_denom_factors,
                )?;
            }
            Adjustment::None => {}
        }

        Ok((pool, output, adjustment))
    }
}
