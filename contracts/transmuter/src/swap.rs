use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    ensure, ensure_eq, to_binary, Addr, BankMsg, Coin, Decimal, Deps, DepsMut, Env, Response,
    StdError, Uint128,
};
use osmosis_std::types::osmosis::tokenfactory::v1beta1::{MsgBurn, MsgMint};
use serde::Serialize;

use crate::{
    alloyed_asset::{swap_from_alloyed, swap_to_alloyed},
    contract::Transmuter,
    transmuter_pool::{AmountConstraint, TransmuterPool},
    ContractError,
};

/// Swap fee is hardcoded to zero intentionally.
pub const SWAP_FEE: Decimal = Decimal::zero();

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

    pub fn swap_non_alloyed_exact_amount_in(
        &self,
        token_in: Coin,
        token_out_denom: &str,
        token_out_min_amount: Uint128,
        sender: Addr,
        deps: DepsMut,
        env: Env,
    ) -> Result<Response, ContractError> {
        let (pool, actual_token_out) =
            self.out_amt_given_in(deps.as_ref(), token_in, &token_out_denom)?;

        // ensure token_out amount is greater than or equal to token_out_min_amount
        ensure!(
            actual_token_out.amount >= token_out_min_amount,
            ContractError::InsufficientTokenOut {
                required: token_out_min_amount,
                available: actual_token_out.amount
            }
        );

        // check and update limiters only if pool assets are not zero
        if let Some(denom_weight_pairs) = pool.weights()? {
            self.limiters.check_limits_and_update(
                deps.storage,
                denom_weight_pairs,
                env.block.time,
            )?;
        }

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
            .set_data(to_binary(&swap_result)?))
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
        let (pool, actual_token_in) =
            self.in_amt_given_out(deps.as_ref(), token_out.clone(), token_in_denom.to_string())?;

        ensure!(
            actual_token_in.amount <= token_in_max_amount,
            ContractError::ExcessiveRequiredTokenIn {
                limit: token_in_max_amount,
                required: actual_token_in.amount,
            }
        );

        // check and update limiters only if pool assets are not zero
        if let Some(denom_weight_pairs) = pool.weights()? {
            self.limiters.check_limits_and_update(
                deps.storage,
                denom_weight_pairs,
                env.block.time,
            )?;
        }

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
            .set_data(to_binary(&swap_result)?))
    }

    pub fn in_amt_given_out(
        &self,
        deps: Deps,
        token_out: Coin,
        token_in_denom: String,
    ) -> Result<(TransmuterPool, Coin), ContractError> {
        // ensure token in denom and token out denom are not the same
        ensure!(
            token_out.denom != token_in_denom,
            StdError::generic_err("token_in_denom and token_out_denom cannot be the same")
        );

        let alloyed_denom = self.alloyed_asset.get_alloyed_denom(deps.storage)?;

        // TODO: normalize these stuffs
        let token_in = Coin::new(token_out.amount.u128(), token_in_denom);
        let mut pool = self.pool.load(deps.storage)?;

        // In case where token_in_denom or token_out_denom is alloyed_denom:
        // TODO: use swap variant instead of checking the denom
        if token_in.denom == alloyed_denom {
            // token_in_denom == alloyed_denom: is the same as exit pool
            // so we ensure that exit pool has no problem
            pool.exit_pool(&[token_out])?;
            return Ok((pool, token_in));
        }

        if token_out.denom == alloyed_denom {
            // token_out_denom == alloyed_denom: is the same as join pool
            // so we ensure that join pool has no problem
            pool.join_pool(&[token_in.clone()])?;
            return Ok((pool, token_in));
        }

        let (token_in, actual_token_out) = pool.transmute(
            AmountConstraint::exact_out(token_out.amount),
            &token_in.denom,
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

        Ok((pool, token_in))
    }

    pub fn out_amt_given_in(
        &self,
        deps: Deps,
        token_in: Coin,
        token_out_denom: &str,
    ) -> Result<(TransmuterPool, Coin), ContractError> {
        // ensure token in denom and token out denom are not the same
        ensure!(
            token_out_denom != token_in.denom,
            StdError::generic_err("token_in_denom and token_out_denom cannot be the same")
        );

        let alloyed_denom = self.alloyed_asset.get_alloyed_denom(deps.storage)?;

        // TODO: normalize these stuffs
        let token_out = Coin::new(token_in.amount.u128(), token_out_denom);
        let mut pool = self.pool.load(deps.storage)?;

        // In case where token_in_denom or token_out_denom is alloyed_denom:

        // TODO: use swap variant instead of checking the denom
        if token_in.denom == alloyed_denom {
            // token_in_denom == alloyed_denom: is the same as exit pool
            // so we ensure that exit pool has no problem
            pool.exit_pool(&[token_out.clone()])?;
            return Ok((pool, token_out));
        }

        if token_out.denom == alloyed_denom {
            // token_out_denom == alloyed_denom: is the same as join pool
            // so we ensure that join pool has no problem
            pool.join_pool(&[token_in])?;
            return Ok((pool, token_out));
        }

        let mut pool = self.pool.load(deps.storage)?;

        let (_, token_out) = pool.transmute(
            AmountConstraint::exact_in(token_in.amount),
            &token_in.denom,
            &token_out.denom,
        )?;

        Ok((pool, token_out))
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
