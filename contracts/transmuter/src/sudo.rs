use cosmwasm_schema::cw_serde;
use cosmwasm_std::{ensure, to_binary, Coin, Decimal, DepsMut, Env, Response, Uint128};

use crate::{contract::Transmuter, ContractError};

#[cw_serde]
pub enum SudoMsg {
    /// SwapExactAmountIn swaps an exact amount of tokens in for as many tokens out as possible.
    /// The amount of tokens out is determined by the current exchange rate and the swap fee.
    /// The user specifies a minimum amount of tokens out, and the transaction will revert if that amount of tokens
    /// is not received.
    SwapExactAmountIn {
        sender: String,
        token_in: Coin,
        token_out_denom: String,
        token_out_min_amount: Uint128,
        swap_fee: Decimal,
    },
    /// SwapExactAmountOut swaps as many tokens in as possible for an exact amount of tokens out.
    /// The amount of tokens in is determined by the current exchange rate and the swap fee.
    /// The user specifies a maximum amount of tokens in, and the transaction will revert if that amount of tokens
    /// is exceeded.
    SwapExactAmountOut {
        sender: String,
        token_in_denom: String,
        token_in_max_amount: Uint128,
        token_out: Coin,
        swap_fee: Decimal,
    },
}

impl SudoMsg {
    pub fn dispatch(
        self,
        transmuter: &Transmuter,
        ctx: (DepsMut, Env),
    ) -> Result<Response, ContractError> {
        match self {
            SudoMsg::SwapExactAmountIn {
                sender: _,
                token_in,
                token_out_denom,
                token_out_min_amount,
                swap_fee,
            } => {
                let (deps, env) = ctx;
                let (pool, token_out) = transmuter._calc_out_amt_given_in(
                    (deps.as_ref(), env),
                    token_in,
                    token_out_denom,
                    swap_fee,
                )?;

                // ensure token_out amount is greater than or equal to token_out_min_amount
                ensure!(
                    token_out.amount >= token_out_min_amount,
                    ContractError::InsufficientTokenOut {
                        required: token_out_min_amount,
                        available: token_out.amount
                    }
                );

                // save pool
                transmuter.pool.save(deps.storage, &pool)?;

                let swap_result = SwapExactAmountInResponseData {
                    token_out_amount: token_out.amount,
                };

                Ok(Response::new()
                    .add_attribute("method", "swap_exact_amount_in")
                    .set_data(to_binary(&swap_result)?))
            }
            SudoMsg::SwapExactAmountOut {
                sender: _,
                token_in_denom,
                token_in_max_amount,
                token_out,
                swap_fee,
            } => {
                let (deps, env) = ctx;

                let (pool, token_in) = transmuter._calc_in_amt_given_out(
                    (deps.as_ref(), env),
                    token_out,
                    token_in_denom,
                    swap_fee,
                )?;

                ensure!(
                    token_in.amount <= token_in_max_amount,
                    ContractError::ExceedingTokenIn {
                        limit: token_in_max_amount,
                        required: token_in.amount,
                    }
                );

                // save pool
                transmuter.pool.save(deps.storage, &pool)?;

                let swap_result = SwapExactAmountOutResponseData {
                    token_in_amount: token_in.amount,
                };

                Ok(Response::new()
                    .add_attribute("method", "swap_exact_amount_out")
                    .set_data(to_binary(&swap_result)?))
            }
        }
    }
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
