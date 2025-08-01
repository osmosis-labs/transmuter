mod rebalancer_pass;

use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    coin, ensure, ensure_eq, to_json_binary, Coin, Decimal, Deps, Response, StdError, Storage,
    Uint128, Uint256,
};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};

use crate::{
    alloyed_asset::{swap_from_alloyed, swap_to_alloyed},
    asset::{convert_amount, Rounding},
    contract::Transmuter,
    corruptable::Corruptable,
    scope::Scope,
    transmuter_pool::{AmountConstraint, TransmuterPool},
    ContractError,
};

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

    /// remove corrupted assets from the pool & remove all rebalancing configs for that denom
    /// when each corrupted asset is all redeemed
    pub fn clean_up_drained_corrupted_assets(
        &self,
        storage: &mut dyn Storage,
        pool: &mut TransmuterPool,
    ) -> Result<(), ContractError> {
        // remove corrupted assets
        for corrupted in pool.clone().corrupted_assets() {
            if corrupted.amount().is_zero() {
                pool.remove_asset(corrupted.denom())?;
                self.rebalancer
                    .unchecked_remove_config(storage, Scope::denom(corrupted.denom()))?;
            }
        }

        // remove assets from asset groups
        for (label, asset_group) in pool.clone().asset_groups {
            // if asset group is corrupted
            if asset_group.is_corrupted() {
                // remove asset from pool if amount is zero.
                // removing asset here will also remove it from the asset group
                for denom in asset_group.denoms() {
                    if pool.get_pool_asset_by_denom(denom)?.amount().is_zero() {
                        pool.remove_asset(denom)?;
                    }
                }

                // remove rebalancing configs for asset group as well
                if pool.asset_groups.get(&label).is_none() {
                    self.rebalancer
                        .unchecked_remove_config(storage, Scope::asset_group(&label))?;
                }
            }
        }

        Ok(())
    }

    fn total_normalized_incentive_pool_balance(
        &self,
        storage: &dyn Storage,
        pool: &TransmuterPool,
    ) -> Result<Uint256, ContractError> {
        let std_norm_factor = pool.std_norm_factor()?;
        let alloyed_denom = self.alloyed_asset.get_alloyed_denom(storage)?;
        let alloyed_normalization_factor = self.alloyed_asset.get_normalization_factor(storage)?;
        self.incentive_pool
            .get_all_pool_balances(storage)?
            .into_iter()
            .try_fold(
                Uint256::zero(),
                |acc, c| -> Result<Uint256, ContractError> {
                    let denom_factor = if c.denom == alloyed_denom {
                        alloyed_normalization_factor
                    } else {
                        pool.get_pool_asset_by_denom(&c.denom)?
                            .normalization_factor()
                    };

                    Ok(acc.checked_add(
                        convert_amount(
                            c.amount,
                            denom_factor,
                            std_norm_factor,
                            &Rounding::Down, // rounding down because we want to cap what's available
                        )?
                        .into(),
                    )?)
                },
            )
    }
}

pub fn construct_scope_value_pairs(
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

/// Adjustment to the output amount after swap
pub enum Adjustment {
    /// Deduct fee from the output amount
    DeductFee { fee: Coin },
    /// Credit incentive to the beneficiary in a normalized amount
    CreditIncentive { incentive: Uint128 },
    /// No adjustment
    None,
}
