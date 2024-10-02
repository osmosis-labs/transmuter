use std::collections::{BTreeMap, HashMap};

use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    ensure, ensure_eq, to_json_binary, Addr, BankMsg, Coin, Decimal, Deps, DepsMut, Env, Response,
    StdError, Storage, Uint128,
};
use osmosis_std::types::osmosis::tokenfactory::v1beta1::{MsgBurn, MsgMint};
use serde::Serialize;

use crate::{
    alloyed_asset::{swap_from_alloyed, swap_to_alloyed},
    contract::Transmuter,
    scope::Scope,
    transmuter_pool::{AmountConstraint, AssetGroup, TransmuterPool},
    ContractError,
};

/// Swap fee is hardcoded to zero intentionally.
pub const SWAP_FEE: Decimal = Decimal::zero();

impl Transmuter<'_> {
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
        deps: DepsMut,
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
                let tokens_in = vec![Coin::new(in_amount.u128(), token_in_denom)];

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

        let prev_weights = pool.weights_map()?;

        pool.join_pool(&tokens_in)?;

        // check and update limiters only if pool assets are not zero
        if let Some(updated_weights) = pool.asset_weights()? {
            let scope_value_pairs = construct_scope_value_pairs(
                prev_weights,
                updated_weights,
                pool.asset_groups.clone(),
            )?;

            self.limiters.check_limits_and_update(
                deps.storage,
                scope_value_pairs,
                env.block.time,
            )?;
        }

        // no need for cleaning up drained corrupted assets here
        // since this function will only adding more underlying assets
        // rather than removing any of them

        self.pool.save(deps.storage, &pool)?;

        let alloyed_asset_out = Coin::new(
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
        deps: DepsMut,
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

                let tokens_out = vec![Coin::new(out_amount.u128(), token_out_denom)];

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

        let is_force_exit_corrupted_assets = tokens_out.iter().all(|coin| {
            let total_liquidity = pool
                .get_pool_asset_by_denom(&coin.denom)
                .map(|asset| asset.amount())
                .unwrap_or_default();

            let is_redeeming_total_liquidity = coin.amount == total_liquidity;

            pool.is_corrupted_asset(&coin.denom) && is_redeeming_total_liquidity
        });

        // If all tokens out are corrupted assets and exit with all remaining liquidity
        // then ignore the limiters and remove the corrupted assets from the pool
        if is_force_exit_corrupted_assets {
            pool.unchecked_exit_pool(&tokens_out)?;

            // change limiter needs reset if force redemption since it gets by passed
            // the current state will not be accurate
            self.limiters.reset_change_limiter_states(
                deps.storage,
                env.block.time,
                pool.asset_weights()?
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(denom, weight)| (Scope::denom(&denom).key(), weight)) // TODO: handle asset group
                    .collect::<Vec<_>>(),
            )?;
        } else {
            let prev_weights = pool.weights_map()?;

            pool.exit_pool(&tokens_out)?;

            // check and update limiters only if pool assets are not zero
            if let Some(updated_weights) = pool.asset_weights()? {
                let scope_value_pairs = construct_scope_value_pairs(
                    prev_weights,
                    updated_weights,
                    pool.asset_groups.clone(),
                )?;

                self.limiters.check_limits_and_update(
                    deps.storage,
                    scope_value_pairs,
                    env.block.time,
                )?;
            }
        }

        self.clean_up_drained_corrupted_assets(deps.storage, &mut pool)?;

        self.pool.save(deps.storage, &pool)?;

        let bank_send_msg = BankMsg::Send {
            to_address: sender.to_string(),
            amount: tokens_out,
        };

        let alloyed_asset_to_burn = Coin::new(
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
        deps: DepsMut,
        env: Env,
    ) -> Result<Response, ContractError> {
        let pool = self.pool.load(deps.storage)?;
        let prev_weights = pool.weights_map()?;

        let (mut pool, actual_token_out) =
            self.out_amt_given_in(deps.as_ref(), pool, token_in, token_out_denom)?;

        // ensure token_out amount is greater than or equal to token_out_min_amount
        ensure!(
            actual_token_out.amount >= token_out_min_amount,
            ContractError::InsufficientTokenOut {
                min_required: token_out_min_amount,
                amount_out: actual_token_out.amount
            }
        );

        // check and update limiters only if pool assets are not zero
        if let Some(updated_weights) = pool.asset_weights()? {
            let scope_value_pairs = construct_scope_value_pairs(
                prev_weights,
                updated_weights,
                pool.asset_groups.clone(),
            )?;

            self.limiters.check_limits_and_update(
                deps.storage,
                scope_value_pairs,
                env.block.time,
            )?;
        }

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
        deps: DepsMut,
        env: Env,
    ) -> Result<Response, ContractError> {
        let pool = self.pool.load(deps.storage)?;
        let prev_weights = pool.weights_map()?;

        let (mut pool, actual_token_in) = self.in_amt_given_out(
            deps.as_ref(),
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

        // check and update limiters only if pool assets are not zero
        if let Some(updated_weights) = pool.asset_weights()? {
            let scope_value_pairs = construct_scope_value_pairs(
                prev_weights,
                updated_weights,
                pool.asset_groups.clone(),
            )?;
            self.limiters.check_limits_and_update(
                deps.storage,
                scope_value_pairs,
                env.block.time,
            )?;
        }

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
                let token_in = Coin::new(token_in_amount.u128(), token_in_denom);
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
                let token_in = Coin::new(token_in_amount.u128(), token_in_denom);
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
                let token_out = Coin::new(token_out_amount.u128(), token_out_denom);
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
                let token_out = Coin::new(token_out_amount.u128(), token_out_denom);
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

    /// remove corrupted assets from the pool & deregister all limiters for that denom
    /// when each corrupted asset is all redeemed
    fn clean_up_drained_corrupted_assets(
        &self,
        storage: &mut dyn Storage,
        pool: &mut TransmuterPool,
    ) -> Result<(), ContractError> {
        for corrupted in pool.clone().corrupted_assets() {
            if corrupted.amount().is_zero() {
                pool.remove_corrupted_asset(corrupted.denom())?;
                self.limiters.uncheck_deregister_all_for_scope(
                    storage,
                    Scope::denom(corrupted.denom()), // TODO: bubble this up
                )?;
            }
        }

        // TODO: remove limiters from asset group too
        // TODO: remove denom from asset group, if asset group is empty, remove it

        Ok(())
    }
}

fn construct_scope_value_pairs(
    prev_weights: BTreeMap<String, Decimal>,
    updated_weights: Vec<(String, Decimal)>,
    asset_group: BTreeMap<String, AssetGroup>,
) -> Result<Vec<(Scope, (Decimal, Decimal))>, StdError> {
    let mut denom_weight_pairs: HashMap<Scope, (Decimal, Decimal)> = HashMap::new();
    let mut asset_group_weight_pairs: HashMap<Scope, (Decimal, Decimal)> = HashMap::new();

    // Reverse index the asset groups
    // TODO: handle cases where asset group contains denom that does not exist
    let mut asset_groups_of_denom = HashMap::new();
    for (group, asset_group) in asset_group {
        for denom in asset_group.into_denoms() {
            asset_groups_of_denom
                .entry(denom)
                .or_insert_with(Vec::new)
                .push(group.clone());
        }
    }

    for (denom, weight) in &updated_weights {
        let prev_weight = prev_weights.get(denom.as_str()).unwrap_or(weight);
        denom_weight_pairs.insert(Scope::denom(denom), (*prev_weight, *weight));

        for group in asset_groups_of_denom.get(denom).unwrap_or(&vec![]) {
            match asset_group_weight_pairs.get_mut(&Scope::asset_group(group)) {
                Some((prev, curr)) => {
                    *prev = prev.checked_add(*prev_weight)?;
                    *curr = curr.checked_add(*weight)?;
                }
                None => {
                    asset_group_weight_pairs
                        .insert(Scope::asset_group(group), (*prev_weight, *weight));
                }
            }

            // TODO: check for invalid cases like total weight is not 1, proptest it
        }
    }

    Ok(denom_weight_pairs
        .into_iter()
        .chain(asset_group_weight_pairs.into_iter())
        .collect())
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
    use crate::{asset::Asset, limiter::LimiterParams};

    use super::*;
    use cosmwasm_std::{
        coin,
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
        let transmuter = Transmuter::default();
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
            tokens_in: &[Coin::new(100, "denom1")],
            token_out_min_amount: Uint128::one(),
        },
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgMint {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(Coin::new(10000u128, "alloyed").into()),
                mint_to_address: "addr1".to_string()
            })),
    )]
    #[case(
        Entrypoint::Sudo,
        SwapToAlloyedConstraint::ExactIn {
            tokens_in: &[Coin::new(100, "denom1")],
            token_out_min_amount: Uint128::one(),
        },
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .set_data(to_json_binary(&SwapExactAmountInResponseData {
                token_out_amount: Uint128::new(10000u128)
            }).unwrap())
            .add_message(MsgMint {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(Coin::new(10000u128, "alloyed").into()),
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
                amount: Some(Coin::new(10000u128, "alloyed").into()),
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
                amount: Some(Coin::new(10000u128, "alloyed").into()),
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
        let transmuter = Transmuter::default();
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
                amount: Some(Coin::new(100u128, "alloyed").into()),
                burn_from_address: "addr1".to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![Coin::new(1u128, "denom1")]
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
                amount: Some(Coin::new(100u128, "alloyed").into()),
                burn_from_address: MOCK_CONTRACT_ADDR.to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![Coin::new(1u128, "denom1")]
            })
            .set_data(to_json_binary(&SwapExactAmountInResponseData {
                token_out_amount: Uint128::from(1u128)
            }).unwrap()))
    )]
    #[case(
        Entrypoint::Exec,
        SwapFromAlloyedConstraint::ExactOut {
            tokens_out: &[Coin::new(1u128, "denom1")],
            token_in_max_amount: Uint128::from(100u128),
        },
        BurnTarget::SenderAccount,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgBurn {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(Coin::new(100u128, "alloyed").into()),
                burn_from_address: "addr1".to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![Coin::new(1u128, "denom1")]
            }))
    )]
    #[case(
        Entrypoint::Sudo,
        SwapFromAlloyedConstraint::ExactOut {
            tokens_out: &[Coin::new(1u128, "denom1")],
            token_in_max_amount: Uint128::from(100u128),
        },
        BurnTarget::SentFunds,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgBurn {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(Coin::new(100u128, "alloyed").into()),
                burn_from_address: MOCK_CONTRACT_ADDR.to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![Coin::new(1u128, "denom1")]
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
            &[Coin::new(110000000000000u128, "alloyed")],
        )]);

        let transmuter = Transmuter::default();
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
            tokens_out: &[Coin::new(1000000000000u128, "denom1")],
            token_in_max_amount: Uint128::from(100000000000000u128),
        },
        vec!["denom1"],
        vec!["denom1"],
        BurnTarget::SentFunds,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgBurn {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(Coin::new(100000000000000u128, "alloyed").into()),
                burn_from_address: MOCK_CONTRACT_ADDR.to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![Coin::new(1000000000000u128, "denom1")]
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
                amount: Some(Coin::new(100000000000000u128, "alloyed").into()),
                burn_from_address: MOCK_CONTRACT_ADDR.to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![Coin::new(1000000000000u128, "denom1")]
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
                amount: Some(Coin::new(100000000000000u128, "alloyed").into()),
                burn_from_address: "addr1".to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![Coin::new(1000000000000u128, "denom1")]
            }))
    )]
    #[case(
        Entrypoint::Exec,
        SwapFromAlloyedConstraint::ExactOut {
            tokens_out: &[Coin::new(1000000000000u128, "denom1")],
            token_in_max_amount: Uint128::from(100000000000000u128),
        },
        vec!["denom1"],
        vec!["denom1"],
        BurnTarget::SenderAccount,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgBurn {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(Coin::new(100000000000000u128, "alloyed").into()),
                burn_from_address: "addr1".to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![Coin::new(1000000000000u128, "denom1")]
            }))
    )]
    #[case(
        Entrypoint::Sudo,
        SwapFromAlloyedConstraint::ExactOut {
            tokens_out: &[Coin::new(1000000000000u128, "denom1"), Coin::new(1000000000000u128, "denom2")],
            token_in_max_amount: Uint128::from(110000000000000u128),
        },
        vec!["denom1", "denom2"],
        vec!["denom1", "denom2"],
        BurnTarget::SentFunds,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgBurn {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(Coin::new(110000000000000u128, "alloyed").into()),
                burn_from_address: MOCK_CONTRACT_ADDR.to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![Coin::new(1000000000000u128, "denom1"), Coin::new(1000000000000u128, "denom2")]
            })
            .set_data(to_json_binary(&SwapExactAmountOutResponseData {
                token_in_amount: Uint128::from(110000000000000u128),
            }).unwrap()))
    )]
    #[case(
        Entrypoint::Sudo,
        SwapFromAlloyedConstraint::ExactOut {
            tokens_out: &[Coin::new(1000000000000u128, "denom1"), Coin::new(500000000000u128, "denom2")],
            token_in_max_amount: Uint128::from(105000000000000u128),
        },
        vec!["denom1", "denom2"],
        vec!["denom1"],
        BurnTarget::SentFunds,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgBurn {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(Coin::new(105000000000000u128, "alloyed").into()),
                burn_from_address: MOCK_CONTRACT_ADDR.to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![Coin::new(1000000000000u128, "denom1"), Coin::new(500000000000u128, "denom2")],
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
            &[Coin::new(210000000000000u128, "alloyed")],
        )]);

        let transmuter = Transmuter::default();
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
        let transmuter = Transmuter::default();
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
        let transmuter = Transmuter::default();
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
        Coin::new(100u128, "denom1"),
        "denom2",
        1000u128,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![Coin::new(1000u128, "denom2")]
            })
            .set_data(to_json_binary(&SwapExactAmountInResponseData {
                token_out_amount: Uint128::from(1000u128)
            }).unwrap()))
    )]
    #[case(
        Coin::new(100u128, "denom2"),
        "denom1",
        10u128,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![Coin::new(10u128, "denom1")]
            })
            .set_data(to_json_binary(&SwapExactAmountInResponseData {
                token_out_amount: Uint128::from(10u128)
            }).unwrap()))
    )]
    #[case(
        Coin::new(100u128, "denom2"),
        "denom1",
        100u128,
        Addr::unchecked("addr1"),
        Err(ContractError::InsufficientTokenOut {
            min_required: 100u128.into(),
            amount_out: 10u128.into()
        })
    )]
    #[case(
        Coin::new(100000000001u128, "denom1"),
        "denom2",
        1000000000010u128,
        Addr::unchecked("addr1"),
        Err(ContractError::InsufficientPoolAsset {
            required: Coin::new(1000000000010u128, "denom2"),
            available: Coin::new(1000000000000u128, "denom2"),
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
            &[Coin::new(2000000000000u128, "alloyed")],
        )]);

        let transmuter = Transmuter::default();
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
        Coin::new(1000u128, "denom2"),
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![Coin::new(1000u128, "denom2")]
            })
            .set_data(to_json_binary(&SwapExactAmountOutResponseData {
                token_in_amount: 100u128.into()
            }).unwrap()))
    )]
    #[case(
        "denom2",
        100u128,
        Coin::new(10u128, "denom1"),
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![Coin::new(10u128, "denom1")]
            })
            .set_data(to_json_binary(&SwapExactAmountOutResponseData {
                token_in_amount: 100u128.into()
            }).unwrap()))
    )]
    #[case(
        "denom2",
        100u128,
        Coin::new(100u128, "denom1"),
        Addr::unchecked("addr1"),
        Err(ContractError::ExcessiveRequiredTokenIn {
            limit: 100u128.into(),
            required: 1000u128.into()
        })
    )]
    #[case(
        "denom1",
        100000000001u128,
        Coin::new(1000000000010u128, "denom2"),
        Addr::unchecked("addr1"),
        Err(ContractError::InsufficientPoolAsset {
            required: Coin::new(1000000000010u128, "denom2"),
            available: Coin::new(1000000000000u128, "denom2"),
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
            &[Coin::new(2000000000000u128, "alloyed")],
        )]);

        let transmuter = Transmuter::default();
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
        HashMap::from([]),
        vec![],
        vec![],
    )]
    #[case::no_asset_group(
        HashMap::from([]),
        vec![
            ("eth.axl", (Decimal::percent(20), Decimal::percent(40))),
            ("eth.wh", (Decimal::percent(60), Decimal::percent(40))),
            ("wsteth.axl", (Decimal::percent(20), Decimal::percent(20))),
        ],
        vec![],
    )]
    #[case(
        HashMap::from([
            ("axelar", vec!["eth.axl", "wsteth.axl"]),
            ("wormhole", vec!["eth.wh"]),
        ]),
        vec![
            ("eth.axl", (Decimal::percent(20), Decimal::percent(40))),
            ("wsteth.axl", (Decimal::percent(20), Decimal::percent(20))),
            ("eth.wh", (Decimal::percent(60), Decimal::percent(40))),
        ],
        vec![
            (Scope::asset_group("axelar"), (Decimal::percent(40), Decimal::percent(60))),
            (Scope::asset_group("wormhole"), (Decimal::percent(60), Decimal::percent(40))),
        ],
    )]
    fn test_construct_scope_value_pairs(
        #[case] asset_groups: HashMap<&str, Vec<&str>>,
        #[case] denom_weights: Vec<(&str, (Decimal, Decimal))>,
        #[case] expected_asset_group_scopes: Vec<(Scope, (Decimal, Decimal))>,
    ) {
        let asset_groups = asset_groups
            .into_iter()
            .map(|(label, asset_group)| {
                (
                    label.to_string(),
                    AssetGroup::new(
                        asset_group
                            .into_iter()
                            .map(|asset| asset.to_string())
                            .collect_vec(),
                    ),
                )
            })
            .collect::<BTreeMap<String, AssetGroup>>();

        let prev_weights = denom_weights
            .clone()
            .into_iter()
            .map(|(denom, (prev_weight, _))| (denom.to_string(), prev_weight))
            .collect();

        let updated_weights = denom_weights
            .clone()
            .into_iter()
            .map(|(denom, (_, updated_weight))| (denom.to_string(), updated_weight))
            .collect_vec();

        let mut scope_value_pairs =
            construct_scope_value_pairs(prev_weights, updated_weights, asset_groups).unwrap();

        let scope_denom_value_pairs = denom_weights
            .into_iter()
            .map(|(denom, weight_transition)| (Scope::denom(denom), weight_transition))
            .collect_vec();

        let mut expected_scope_value_pairs =
            vec![scope_denom_value_pairs, expected_asset_group_scopes].concat();

        // assert by disregrard order
        scope_value_pairs.sort_by_key(|(scope, _)| scope.key());
        expected_scope_value_pairs.sort_by_key(|(scope, _)| scope.key());

        assert_eq!(scope_value_pairs, expected_scope_value_pairs);
    }
}
