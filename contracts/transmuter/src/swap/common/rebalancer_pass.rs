use crate::{
    alloyed_asset::{swap_from_alloyed, swap_to_alloyed},
    contract::Transmuter,
    scope::Scope,
    swap::common::{construct_scope_value_pairs, Adjustment},
    transmuter_pool::TransmuterPool,
    ContractError,
};
use cosmwasm_std::{
    coin, Coin, Decimal, Deps, DepsMut, Int256, SignedDecimal256, Storage, Uint128, Uint256,
};
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
        RebalancingAdjustment: Fn(Coin, Int256) -> Result<(Coin, Adjustment), ContractError>,
    {
        let (first_order_pool, first_order_output, first_order_total_adjustment_value) =
            self.run_pool_and_compute_adjustment_value(deps.as_ref(), pool, run_pool)?;

        // Apply adjustment effect on the swap output and return the adjustment information
        let (first_order_adjusted_output, first_order_adjustment) = rebalancing_adjustment(
            first_order_output.clone(),
            first_order_total_adjustment_value,
        )?;

        match first_order_adjustment {
            Adjustment::Incentivize { incentive } => self.incentivize(
                deps,
                incentive,
                first_order_pool,
                first_order_output,
                first_order_total_adjustment_value,
                first_order_adjusted_output,
                rebalancing_adjustment,
            ),
            Adjustment::DeductFee { ref fee } => {
                self.incentive_pool.add_tokens(deps.storage, &fee)?;
                Ok((
                    first_order_pool,
                    first_order_adjusted_output,
                    first_order_adjustment,
                ))
            }
            Adjustment::None => Ok((
                first_order_pool,
                first_order_adjusted_output,
                first_order_adjustment,
            )),
        }
    }

    /// Incentivize is more complex than taking fee since there are cases where there is not enough incentive token
    /// to be given out. In such cases, we need to perform internal incentive swap to get additional incentive token.
    /// That process itself could result in change of the pool balance, which could affect the total adjustment value.
    ///
    /// The `first_order` prefix means the result from the first pass of incentive calculation,
    /// the `second_order` is the result from the internal incentive swap.
    ///
    /// This function handles the internal incentive swap and returns the updated pool, output and adjustment.
    fn incentivize<RebalancingAdjustment>(
        &self,
        deps: DepsMut,
        first_order_incentive: Coin,
        first_order_pool: TransmuterPool,
        first_order_output: Coin,
        first_order_total_adjustment_value: Int256,
        first_order_adjusted_output: Coin,
        rebalancing_adjustment: RebalancingAdjustment,
    ) -> Result<(TransmuterPool, Coin, Adjustment), ContractError>
    where
        RebalancingAdjustment: Fn(Coin, Int256) -> Result<(Coin, Adjustment), ContractError>,
    {
        // check if incentive pool has enough balance for the incentive
        let incentive_denom_balance = self
            .incentive_pool
            .get_pool_balance(deps.storage, &first_order_incentive.denom)?;

        // if not enought, proceed with internal incentive swap
        if incentive_denom_balance < first_order_incentive.amount {
            self.incentivize_with_internal_incentive_swap(
                deps,
                first_order_incentive,
                first_order_pool,
                first_order_output,
                first_order_total_adjustment_value,
                first_order_adjusted_output,
                incentive_denom_balance,
                rebalancing_adjustment,
            )
        } else {
            self.incentive_pool
                .remove_tokens(deps.storage, &first_order_incentive)?;

            Ok((
                first_order_pool,
                first_order_adjusted_output,
                Adjustment::Incentivize {
                    incentive: first_order_incentive,
                },
            ))
        }
    }

    /// Internal incentive swap is a process of swapping substitute token to incentive token so that
    /// the incentive pool has enough balance to distribute the incentive.
    ///
    /// The result of the internal incentive swap can be divided largely into 3 cases:
    /// - Second order total adjustment value is positive or zero:
    ///     This means internal swap is helpful or neutral, reward swapper with the original incentive.
    /// - Second order total adjustment value partially negated the original incentive:
    ///     This means internal swap is harmful, but not fully negated the original incentive.
    ///     Adjust the output with the updated total adjustment value.
    /// - Second order total adjustment value fully negated the original incentive:
    ///     This means internal swap is harmful and fully negated the original incentive.
    ///     Abort the internal incentive swap. Gives no incentive.
    ///
    /// When perform internal incentive swap, contract balance stays the same.
    /// The swap will create `second_order_pool` which is the updated pool state after swap
    /// with the following changes:
    /// - substitute_token will be recorded as increased in the liquidity pool,
    ///   but will later decreased from the incentive pool for the same amount.
    ///   Essentially record same amount of token in the contract balance into different incentive/liquidity pool.
    ///
    /// - incentive token will be recorded as decreased in the liquidity pool.
    ///   This creates free incentive token to be distributed as incentive.
    ///
    /// The free incentive token will become part of token out in case of exact in
    /// or part of subsidized token in in case of exact out.
    fn incentivize_with_internal_incentive_swap<RebalancingAdjustment>(
        &self,
        deps: DepsMut,
        first_order_incentive: Coin,
        first_order_pool: TransmuterPool,
        first_order_output: Coin,
        first_order_total_adjustment_value: Int256,
        first_order_adjusted_output: Coin,
        incentive_denom_balance: Uint128,
        rebalancing_adjustment: RebalancingAdjustment,
    ) -> Result<(TransmuterPool, Coin, Adjustment), ContractError>
    where
        RebalancingAdjustment: Fn(Coin, Int256) -> Result<(Coin, Adjustment), ContractError>,
    {
        let additional_incentive_token_needed = coin(
            first_order_incentive
                .amount
                .saturating_sub(incentive_denom_balance)
                .u128(),
            first_order_incentive.denom.clone(),
        );

        // use token that has highest balance as substitute token to perform internal incentive swap to
        // get additional incentive token needed
        let Some(substitute_token_denom) = self
            .incentive_pool
            .get_all_pool_balances(deps.storage)?
            .iter()
            .max_by_key(|c| c.amount)
            .map(|c| c.denom.clone())
        else {
            // if no substitute token available, abort the internal incentive swap. Gives no incentive.
            return Ok((first_order_pool, first_order_output, Adjustment::None));
        };

        // perform internal incentive swap
        let Ok((second_order_pool, substitute_token, second_order_total_adjustment_value)) = self
            .internal_incentive_swap(
                deps.as_ref(),
                &first_order_incentive,
                first_order_pool.clone(),
                additional_incentive_token_needed,
                substitute_token_denom,
            )
        else {
            // if internal incentive swap failed, adjust nothing
            return Ok((first_order_pool, first_order_output, Adjustment::None));
        };

        let updated_total_adjustment_value =
            first_order_total_adjustment_value.checked_add(second_order_total_adjustment_value)?;

        match updated_total_adjustment_value {
            // if internal incentive swap is helpful or neutral, return the original output and adjustment
            u if u >= first_order_total_adjustment_value => {
                self.incentive_pool.remove_tokens(
                    deps.storage,
                    &coin(
                        incentive_denom_balance.u128(),
                        first_order_incentive.denom.clone(),
                    ),
                )?;
                self.incentive_pool
                    .remove_tokens(deps.storage, &substitute_token)?;
                Ok((
                    first_order_pool,
                    first_order_adjusted_output,
                    Adjustment::Incentivize {
                        incentive: first_order_incentive,
                    },
                ))
            }
            // If internal incentive swap is harmful and fully negated the original incentive,
            // abort the internal incentive swap. Gives no incentive.
            u if u <= Int256::zero() => {
                Ok((first_order_pool, first_order_output, Adjustment::None))
            }
            // If internal incentive swap is harmful and partially negated the original incentive,
            // adjust first order output with updated total adjustment value.
            // u if u > Int256::zero()
            _ => {
                let (readjusted_output, second_order_adjustment) = rebalancing_adjustment(
                    first_order_output.clone(),
                    updated_total_adjustment_value,
                )?;

                self.incentive_pool.remove_tokens(
                    deps.storage,
                    &coin(
                        incentive_denom_balance.u128(),
                        first_order_incentive.denom.clone(),
                    ),
                )?;
                self.incentive_pool
                    .remove_tokens(deps.storage, &substitute_token)?;

                Ok((
                    second_order_pool,
                    readjusted_output,
                    second_order_adjustment,
                ))
            }
        }
    }

    /// Internal incentive swap is a process of swapping substitute token to incentive token so that
    /// the incentive pool has enough balance to distribute the incentive.
    ///
    /// It use exact out only because it has expected output amount.
    fn internal_incentive_swap(
        &self,
        deps: Deps,
        first_order_incentive: &Coin,
        first_order_pool: TransmuterPool,
        additional_incentive_token_needed: Coin,
        substitute_token_denom: String,
    ) -> Result<(TransmuterPool, Coin, Int256), ContractError> {
        let alloyed_denom = self.alloyed_asset.get_alloyed_denom(deps.storage)?;

        match (
            first_order_incentive.denom == alloyed_denom,
            substitute_token_denom == alloyed_denom,
        ) {
            // swap alloyed to token
            (true, _) => {
                let token_in_norm_factor =
                    self.alloyed_asset.get_normalization_factor(deps.storage)?;

                let tokens_out = vec![additional_incentive_token_needed.clone()];
                let tokens_out_with_norm_factor =
                    first_order_pool.pair_coins_with_normalization_factor(&tokens_out)?;

                let in_amount = swap_from_alloyed::in_amount_via_exact_out(
                    Uint128::MAX,
                    token_in_norm_factor,
                    tokens_out_with_norm_factor,
                )?;

                let token_in = coin(in_amount.u128(), substitute_token_denom);

                self.run_pool_and_compute_adjustment_value(
                    deps,
                    first_order_pool,
                    |_: Deps, mut pool: TransmuterPool| {
                        pool.exit_pool(&tokens_out)?;
                        Ok((pool, token_in.clone()))
                    },
                )
            }

            // swap token to alloyed
            (_, true) => {
                let token_in_norm_factor = first_order_pool
                    .get_pool_asset_by_denom(&substitute_token_denom)?
                    .normalization_factor();
                let in_amount = swap_to_alloyed::in_amount_via_exact_out(
                    token_in_norm_factor,
                    Uint128::MAX,
                    additional_incentive_token_needed.amount,
                    self.alloyed_asset.get_normalization_factor(deps.storage)?,
                )?;
                let token_in = coin(in_amount.u128(), substitute_token_denom);

                self.run_pool_and_compute_adjustment_value(
                    deps,
                    first_order_pool,
                    |_deps: Deps, mut pool: TransmuterPool| {
                        pool.join_pool(&[token_in.clone()])?;
                        Ok((pool, token_in))
                    },
                )
            }

            // swap token to token
            (_, _) => self.run_pool_and_compute_adjustment_value(
                deps,
                first_order_pool,
                |deps: Deps, pool: TransmuterPool| {
                    self.in_amt_given_out(
                        deps,
                        pool,
                        additional_incentive_token_needed.clone(),
                        substitute_token_denom,
                    )
                },
            ),
        }
    }

    fn run_pool_and_compute_adjustment_value<RunPool>(
        &self,
        deps: Deps,
        pool: TransmuterPool,
        run_pool: RunPool,
    ) -> Result<(TransmuterPool, Coin, Int256), ContractError>
    where
        RunPool: FnOnce(Deps, TransmuterPool) -> Result<(TransmuterPool, Coin), ContractError>,
    {
        let prev_asset_weights = pool.asset_weights()?.unwrap_or_default();
        let prev_asset_group_weights = pool.asset_group_weights()?.unwrap_or_default();

        // total normalized amount of the asset in the pool
        let total_balance_before = pool.normalized_total_balance()?;

        let (pool, output) = run_pool(deps, pool)?;

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
                        deps,
                        &scope_value_pairs,
                        total_balance_before,
                    )?;

                // incentive pool is unhealthy if total incentive required to rebalance
                // is greater than avaialable incentive pool
                let is_incentive_pool_unhealthy =
                    total_incentive_required_to_rebalance > total_normalized_incentive_pool_balance;

                // compute total adjustment rate based on the incentive pool health
                total_adjustment_rate = self.compute_total_adjustment_rate(
                    deps,
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

        Ok((pool, output, total_adjustment_value))
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
        let mut total_adjustment_to_ideal_required: SignedDecimal256 = SignedDecimal256::zero();
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

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use std::collections::BTreeMap;

    use crate::{asset::Asset, swap::rebalancing_adjustment_for_exact_in};

    use super::*;

    #[rstest]
    #[case::no_internal_swap(
        vec![coin(200_000_000u128, "denom2")],
        200_000_000u128,
        vec![],
        vec![
            coin(25_000_000_000u128, "denom1"),
            coin(390_000_000_000u128, "denom2"),
            coin(3_600_000_000_000u128, "denom3"),
        ]
    )]
    #[case::no_internal_swap(
        vec![coin(199_999_99u128, "denom2")],
        0u128,
        vec![coin(199_999_99u128, "denom2")],
        vec![
            coin(25_000_000_000u128, "denom1"),
            coin(390_000_000_000u128, "denom2"),
            coin(3_600_000_000_000u128, "denom3"),
        ]
    )]
    #[case::incentive_partially_negated(
        vec![coin(2_000_000_000u128, "denom3")],
        198_000_000u128,
        vec![],
        vec![
            coin(25_000_000_000u128, "denom1"),
            coin(389_800_000_000u128, "denom2"),
            coin(3_602_000_000_000u128, "denom3"),
        ]
    )]
    fn test_incentivize_exact_in(
        #[case] incentive_pool: Vec<Coin>,
        #[case] incentive: u128,
        #[case] resulted_incentive_pool: Vec<Coin>,
        #[case] resulted_pool_assets: Vec<Coin>,
    ) {
        let transmuter = Transmuter::new();
        let mut deps = cosmwasm_std::testing::mock_dependencies_with_balances(&[]);
        transmuter
            .alloyed_asset
            .set_alloyed_denom(&mut deps.storage, &"alloyed".to_string())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_normalization_factor(&mut deps.storage, 100u128.into())
            .unwrap();

        let norm_factors = BTreeMap::from([
            ("denom1".to_string(), 1u128),
            ("denom2".to_string(), 10u128),
            ("denom3".to_string(), 100u128),
        ]);

        transmuter
            .pool
            .save(
                &mut deps.storage,
                &TransmuterPool {
                    pool_assets: vec![
                        Asset::new(
                            Uint128::from(24_000_000_000u128),
                            "denom1",
                            *norm_factors.get("denom1").unwrap(),
                        )
                        .unwrap(),
                        Asset::new(
                            Uint128::from(400_000_000_000u128),
                            "denom2",
                            *norm_factors.get("denom2").unwrap(),
                        )
                        .unwrap(),
                        Asset::new(
                            Uint128::from(3_600_000_000_000u128),
                            "denom3",
                            *norm_factors.get("denom3").unwrap(),
                        )
                        .unwrap(),
                    ],
                    asset_groups: BTreeMap::new(),
                },
            )
            .unwrap();

        transmuter
            .rebalancer
            .add_config(
                &mut deps.storage,
                Scope::denom("denom1"),
                RebalancingConfig::new(
                    Decimal::percent(40),
                    Decimal::percent(25),
                    Decimal::percent(55),
                    Decimal::percent(10),
                    Decimal::percent(65),
                    Decimal::percent(1),
                    Decimal::percent(2),
                )
                .unwrap(),
            )
            .unwrap();

        transmuter
            .rebalancer
            .add_config(
                &mut deps.storage,
                Scope::denom("denom2"),
                RebalancingConfig::new(
                    Decimal::percent(39),
                    Decimal::percent(25),
                    Decimal::percent(55),
                    Decimal::percent(10),
                    Decimal::percent(65),
                    Decimal::percent(1),
                    Decimal::percent(2),
                )
                .unwrap(),
            )
            .unwrap();

        transmuter
            .rebalancer
            .add_config(
                &mut deps.storage,
                Scope::denom("denom3"),
                RebalancingConfig::new(
                    Decimal::bps(3601),
                    Decimal::percent(36),
                    Decimal::percent(55),
                    Decimal::percent(10),
                    Decimal::percent(65),
                    Decimal::percent(1),
                    Decimal::percent(2),
                )
                .unwrap(),
            )
            .unwrap();

        for coin in incentive_pool {
            transmuter
                .incentive_pool
                .add_tokens(&mut deps.storage, &coin)
                .unwrap();
        }

        let token_in = coin(1_000_000_000u128, "denom1");
        let token_out_denom = "denom2";
        let pool = transmuter.pool.load(&deps.storage).unwrap();

        let std_norm_factor = pool.std_norm_factor().unwrap();
        let token_out_norm_factor = pool
            .get_pool_asset_by_denom(token_out_denom)
            .unwrap()
            .normalization_factor();

        let run_pool = |deps: Deps, pool: TransmuterPool| {
            transmuter.out_amt_given_in(deps, pool, token_in.clone(), token_out_denom)
        };

        let expected_token_out_amount = 10_000_000_000u128 + incentive;

        let rebalancing_adjustment = rebalancing_adjustment_for_exact_in(
            expected_token_out_amount.into(),
            std_norm_factor,
            token_out_norm_factor,
        );

        let (pool, output, adjustment) = transmuter
            .rebalancer_pass(
                deps.as_mut(),
                pool.clone(),
                run_pool,
                rebalancing_adjustment,
            )
            .unwrap();

        assert_eq!(output, coin(expected_token_out_amount, "denom2"));

        if incentive > 0 {
            assert_eq!(
                adjustment,
                Adjustment::Incentivize {
                    incentive: coin(incentive, "denom2")
                }
            )
        } else {
            assert_eq!(adjustment, Adjustment::None);
        };

        assert_eq!(
            transmuter
                .incentive_pool
                .get_all_pool_balances(&deps.storage)
                .unwrap(),
            resulted_incentive_pool
        );

        assert_eq!(
            pool.pool_assets,
            resulted_pool_assets
                .into_iter()
                .map(|coin| {
                    Asset::new(
                        coin.amount,
                        &coin.denom,
                        *norm_factors.get(&coin.denom).unwrap(),
                    )
                    .unwrap()
                })
                .collect::<Vec<_>>(),
        );
    }

    fn no_config_update(
        _transmuter: &Transmuter,
        _storage: &mut dyn Storage,
    ) -> Result<(), ContractError> {
        Ok(())
    }

    #[rstest]
    #[case::incentive_fully_negated(
        vec![coin(2_000_000_000_000u128, "denom3")],
        0u128,
        vec![coin(2_000_000_000_000u128, "denom3")],
        vec![
            coin(25_000_000_000u128, "denom1"),
            coin(390_000_000_000u128, "denom2"),
            coin(3_600_000_000_000u128, "denom3"),
        ],
        no_config_update
    )]
    #[case::incentive_amplified(
        vec![coin(20_000_001u128, "denom1")],
        20_000_000u128,
        vec![coin(1u128, "denom1")],
        vec![
            coin(25_000_000_000u128, "denom1"),
            coin(390_000_000_000u128, "denom2"),
            coin(3_600_000_000_000u128, "denom3"),
        ],
        |transmuter: &Transmuter, storage: &mut dyn Storage| {
            transmuter
            .rebalancer
            .update_config(
                storage,
                Scope::denom("denom3"),
                &RebalancingConfig::new(
                    Decimal::percent(40),
                    Decimal::percent(37),
                    Decimal::percent(55),
                    Decimal::percent(10),
                    Decimal::percent(65),
                    Decimal::percent(2),
                    Decimal::percent(2),
                )
                .unwrap(),
            )
        }
    )]
    fn test_incentivize_exact_out(
        #[case] incentive_pool: Vec<Coin>,
        #[case] incentive: u128,
        #[case] resulted_incentive_pool: Vec<Coin>,
        #[case] resulted_pool_assets: Vec<Coin>,
        #[case] update_config: fn(&Transmuter, &mut dyn Storage) -> Result<(), ContractError>,
    ) {
        use crate::swap::rebalancing_adjustment_for_exact_out;

        let transmuter = Transmuter::new();
        let mut deps = cosmwasm_std::testing::mock_dependencies_with_balances(&[]);
        transmuter
            .alloyed_asset
            .set_alloyed_denom(&mut deps.storage, &"alloyed".to_string())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_normalization_factor(&mut deps.storage, 100u128.into())
            .unwrap();

        let norm_factors = BTreeMap::from([
            ("denom1".to_string(), 1u128),
            ("denom2".to_string(), 10u128),
            ("denom3".to_string(), 100u128),
        ]);

        transmuter
            .pool
            .save(
                &mut deps.storage,
                &TransmuterPool {
                    pool_assets: vec![
                        Asset::new(
                            Uint128::from(24_000_000_000u128),
                            "denom1",
                            *norm_factors.get("denom1").unwrap(),
                        )
                        .unwrap(),
                        Asset::new(
                            Uint128::from(400_000_000_000u128),
                            "denom2",
                            *norm_factors.get("denom2").unwrap(),
                        )
                        .unwrap(),
                        Asset::new(
                            Uint128::from(3_600_000_000_000u128),
                            "denom3",
                            *norm_factors.get("denom3").unwrap(),
                        )
                        .unwrap(),
                    ],
                    asset_groups: BTreeMap::new(),
                },
            )
            .unwrap();

        transmuter
            .rebalancer
            .add_config(
                &mut deps.storage,
                Scope::denom("denom1"),
                RebalancingConfig::new(
                    Decimal::percent(40),
                    Decimal::percent(25),
                    Decimal::percent(55),
                    Decimal::percent(10),
                    Decimal::percent(65),
                    Decimal::percent(1),
                    Decimal::percent(2),
                )
                .unwrap(),
            )
            .unwrap();

        transmuter
            .rebalancer
            .add_config(
                &mut deps.storage,
                Scope::denom("denom2"),
                RebalancingConfig::new(
                    Decimal::percent(39),
                    Decimal::percent(25),
                    Decimal::percent(55),
                    Decimal::percent(10),
                    Decimal::percent(65),
                    Decimal::percent(1),
                    Decimal::percent(2),
                )
                .unwrap(),
            )
            .unwrap();

        transmuter
            .rebalancer
            .add_config(
                &mut deps.storage,
                Scope::denom("denom3"),
                RebalancingConfig::new(
                    Decimal::percent(36),
                    Decimal::percent(25),
                    Decimal::percent(55),
                    Decimal::percent(10),
                    Decimal::percent(65),
                    Decimal::percent(100),
                    Decimal::percent(2),
                )
                .unwrap(),
            )
            .unwrap();

        update_config(&transmuter, &mut deps.storage).unwrap();

        for coin in incentive_pool {
            transmuter
                .incentive_pool
                .add_tokens(&mut deps.storage, &coin)
                .unwrap();
        }

        let token_in_denom = "denom1";
        let token_out = coin(10_000_000_000u128, "denom2");
        let pool = transmuter.pool.load(&deps.storage).unwrap();

        let std_norm_factor = pool.std_norm_factor().unwrap();
        let token_in_norm_factor = pool
            .get_pool_asset_by_denom(token_in_denom)
            .unwrap()
            .normalization_factor();

        let expected_token_in_amount = 1_000_000_000u128 - incentive;

        let run_pool = |deps: Deps, pool: TransmuterPool| {
            transmuter.in_amt_given_out(deps, pool, token_out.clone(), token_in_denom.to_string())
        };

        let rebalancing_adjustment = rebalancing_adjustment_for_exact_out(
            token_out.amount,
            std_norm_factor,
            token_in_norm_factor,
        );

        let (pool, output, adjustment) = transmuter
            .rebalancer_pass(
                deps.as_mut(),
                pool.clone(),
                run_pool,
                rebalancing_adjustment,
            )
            .unwrap();

        assert_eq!(output, coin(expected_token_in_amount, "denom1"));

        if incentive > 0 {
            assert_eq!(
                adjustment,
                Adjustment::Incentivize {
                    incentive: coin(incentive, "denom1")
                }
            )
        } else {
            assert_eq!(adjustment, Adjustment::None);
        };

        assert_eq!(
            transmuter
                .incentive_pool
                .get_all_pool_balances(&deps.storage)
                .unwrap(),
            resulted_incentive_pool
        );

        assert_eq!(
            pool.pool_assets,
            resulted_pool_assets
                .into_iter()
                .map(|coin| {
                    Asset::new(
                        coin.amount,
                        &coin.denom,
                        *norm_factors.get(&coin.denom).unwrap(),
                    )
                    .unwrap()
                })
                .collect::<Vec<_>>(),
        );
    }
}
