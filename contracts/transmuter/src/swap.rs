use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    ensure, to_binary, Addr, BankMsg, Coin, Deps, DepsMut, Env, Response, StdError, Uint128,
};
use osmosis_std::types::osmosis::tokenfactory::v1beta1::{MsgBurn, MsgMint};
use serde::Serialize;

use crate::{
    alloyed_asset::{swap_from_alloyed, swap_to_alloyed},
    contract::{BurnTarget, SwapFromAlloyedConstraint, SwapToAlloyedConstraint, Transmuter},
    transmuter_pool::TransmuterPool,
    ContractError,
};

impl Transmuter<'_> {
    /// Getting the [SwapVariant] of the swap operation
    /// assuming the swap token is not
    pub fn swap_varaint(
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

        pool.join_pool(&tokens_in)?;

        // check and update limiters only if pool assets are not zero
        if let Some(denom_weight_pairs) = pool.weights()? {
            self.limiters.check_limits_and_update(
                deps.storage,
                denom_weight_pairs,
                env.block.time,
            )?;
        }

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

            // no need to check shares sufficiency since it requires pre-sending shares to the contract
            BurnTarget::SentFunds => Ok(&env.contract.address),
        }?
        .to_string();

        pool.exit_pool(&tokens_out)?;

        // check and update limiters only if pool assets are not zero
        if let Some(denom_weight_pairs) = pool.weights()? {
            self.limiters.check_limits_and_update(
                deps.storage,
                denom_weight_pairs,
                env.block.time,
            )?;
        }

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
}

/// Possible variants of swap, depending on the input and output tokens
#[derive(Serialize, Clone, PartialEq, Debug)]
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
        Entrypoint::Sudo => response.set_data(to_binary(data)?),
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
