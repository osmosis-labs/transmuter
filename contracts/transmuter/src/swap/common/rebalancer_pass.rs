use crate::{
    contract::Transmuter,
    scope::Scope,
    swap::common::{construct_scope_value_pairs, Adjustment},
    transmuter_pool::TransmuterPool,
    ContractError,
};
use cosmwasm_std::{Coin, Decimal, Deps, DepsMut, Int256, SignedDecimal256, Uint128, Uint256};
use std::cmp::Ordering;
use transmuter_math::rebalancing::{
    compute_total_effective_adjustment_rate, config::RebalancingConfig, round_adjustment,
};

impl Transmuter {
    /// Executes core rebalancing logic that applies rebalancing adjustments to a pool operation and check limits.
    ///
    /// This function is a core component of the transmuter's rebalancing mechanism. It:
    /// - Captures the pool state before the operation
    /// - Executes the provided pool operation
    /// - Calculates rebalancing adjustments based on weight changes
    /// - Applies incentives or fees based on whether the operation helps or harms pool balance
    /// - Updates the incentive pool accordingly
    /// - Checks limits
    pub fn rebalancer_pass<RunPool, RebalancingAdjustment>(
        &self,
        deps: DepsMut,
        pool: TransmuterPool,
        run_pool: RunPool,
        rebalancing_adjustment: RebalancingAdjustment,
    ) -> Result<(TransmuterPool, Coin, Adjustment), ContractError>
    where
        RunPool: FnOnce(Deps, TransmuterPool) -> Result<(TransmuterPool, Coin), ContractError>,
        RebalancingAdjustment: FnOnce(Coin, Int256) -> Result<(Coin, Adjustment), ContractError>,
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

                // compute total incentive required to rebalance the pool to ideal balance
                let total_incentive_required_to_rebalance = self
                    .compute_total_incentive_required_to_rebalance(
                        deps.as_ref(),
                        &scope_value_pairs,
                        total_balance_before,
                    )?;

                // incentive pool is unhealthy if total incentive required to rebalance
                // is greater than avaialable incentive pool
                let is_incentive_pool_unhealthy =
                    total_incentive_required_to_rebalance > total_normalized_incentive_pool_balance;

                // compute total adjustment rate based on the incentive pool health
                total_adjustment_rate = self.compute_total_adjustment_rate(
                    deps.as_ref(),
                    &scope_value_pairs,
                    is_incentive_pool_unhealthy,
                )?;

                // check limits
                self.rebalancer
                    .check_limits(deps.storage, scope_value_pairs)?;
            }
        }

        // Compute total adjustment value. It's used to apply incentives or fees to the output.
        let total_adjustment_value = self.compute_total_adjustment_value(
            total_adjustment_rate,
            total_balance_before,
            total_balance_after,
        )?;

        // Apply adjustment effect on the swap output and return the adjustment information
        let (adjusted_output, adjustment) =
            rebalancing_adjustment(output.clone(), total_adjustment_value)?;

        match adjustment {
            Adjustment::Incentivize { ref incentive } => {
                self.incentive_pool
                    .remove_tokens(deps.storage, &incentive)?;

                // TODO: if not enought, preceed with internal swap, we can run rebalancing pass recursively with a flag
            }
            Adjustment::DeductFee { ref fee } => {
                self.incentive_pool.add_tokens(deps.storage, &fee)?;
            }
            Adjustment::None => {}
        }

        Ok((pool, adjusted_output, adjustment))
    }

    /// Compute total incentive required to rebalance the pool to ideal balance.
    ///
    /// This function calculates the total incentive required to rebalance the pool to its ideal balance.
    /// It considers the current weights of the assets and the ideal weights defined in the rebalancing configuration.
    /// The function returns the total normalized incentive required.
    fn compute_total_incentive_required_to_rebalance(
        &self,
        deps: Deps,
        scope_value_pairs: &Vec<(Scope, (Decimal, Decimal))>,
        total_balance_before: Uint128,
    ) -> Result<Uint256, ContractError> {
        // find total adjustment reqruied to move to all assets to ideal balance
        let mut total_adjustment_to_ideal_required = SignedDecimal256::zero();
        for (scope, (prev_weight, _)) in scope_value_pairs.clone() {
            let rebalancing_config = self.rebalancer.get_config_by_scope(deps.storage, &scope)?;
            if let Some(rebalancing_config) = rebalancing_config {
                let nearest_ideal_weight = rebalancing_config.nearest_ideal_weight(prev_weight);

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

        let total_balance = SignedDecimal256::from_atomics(Int256::from(total_balance_before), 0)?;
        let total_incentive_required_for_rebalance =
            round_adjustment(total_adjustment_to_ideal_required.checked_mul(total_balance)?)?
                .abs_diff(Int256::zero()); // absolute value

        Ok(total_incentive_required_for_rebalance)
    }

    /// Compute total adjustment rate. It's sum of each asset's effective adjustment rate.
    ///
    /// Effective adjustment rate is the adjustment rate that is applied to the asset's balance shift.
    /// If incentive pool is unhealthy and it's a fee case, the adjustment rate is raised to the critical rate.
    /// Otherwise, the adjustment rate is the same as the effective adjustment rate.
    fn compute_total_adjustment_rate(
        &self,
        deps: Deps,
        scope_value_pairs: &Vec<(Scope, (Decimal, Decimal))>,
        is_incentive_pool_unhealthy: bool,
    ) -> Result<SignedDecimal256, ContractError> {
        let mut total_adjustment_rate = SignedDecimal256::zero();
        for (scope, (prev_weight, updated_weight)) in scope_value_pairs.clone() {
            let rebalancing_config = self.rebalancer.get_config_by_scope(deps.storage, &scope)?;
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

        Ok(total_adjustment_rate)
    }

    /// Compute total adjustment value. It's used to apply incentives or fees to the output.
    ///
    /// At its core, it is: total adjustment rate * total balance.
    ///
    /// If total balance is updated, it means that one of token in or out is alloyed.
    /// In this case, we need to use the balance before the operation to calculate the adjustment value.
    /// Otherwise, we use the balance after the operation.
    fn compute_total_adjustment_value(
        &self,
        total_adjustment_rate: SignedDecimal256,
        total_balance_before: Uint128,
        total_balance_after: Uint128,
    ) -> Result<Int256, ContractError> {
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

        Ok(round_adjustment(total_adjustment_value)?)
    }
}
