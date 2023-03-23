use cosmwasm_schema::cw_serde;
use cosmwasm_std::{ensure, ensure_eq, to_binary, Coin, Decimal, DepsMut, Env, Response, Uint128};

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

                // ensure swap fee is the same as one from get_swap_fee which essentially is always 0
                // in case where the swap fee mismatch, it can cause the pool to be imbalanced
                let contract_swap_fee = transmuter.get_swap_fee((deps.as_ref(), env))?.swap_fee;
                ensure_eq!(
                    swap_fee,
                    contract_swap_fee,
                    ContractError::InvalidSwapFee {
                        expected: contract_swap_fee,
                        actual: swap_fee
                    }
                );

                let mut pool = transmuter.pool.load(deps.storage)?;
                let token_out = pool.transmute(&token_in, &token_out_denom)?;

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

                let swap_result = SwapResult {
                    token_in,
                    token_out,
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

                // ensure swap fee is the same as one from get_swap_fee which essentially is always 0
                // in case where the swap fee mismatch, it can cause the pool to be imbalanced
                let contract_swap_fee = transmuter.get_swap_fee((deps.as_ref(), env))?.swap_fee;
                ensure_eq!(
                    swap_fee,
                    contract_swap_fee,
                    ContractError::InvalidSwapFee {
                        expected: contract_swap_fee,
                        actual: swap_fee
                    }
                );

                let token_in = Coin::new(token_in_max_amount.into(), token_in_denom);

                let mut pool = transmuter.pool.load(deps.storage)?;
                let actual_token_out = pool.transmute(&token_in, &token_out.denom)?;

                // ensure that actual_token_out is equal to token_out
                ensure_eq!(
                    token_out,
                    actual_token_out,
                    ContractError::InvalidTokenOutAmount {
                        expected: token_out.amount,
                        actual: actual_token_out.amount
                    }
                );

                // save pool
                transmuter.pool.save(deps.storage, &pool)?;

                let swap_result = SwapResult {
                    token_in,
                    token_out: actual_token_out,
                };

                Ok(Response::new()
                    .add_attribute("method", "swap_exact_amount_out")
                    .set_data(to_binary(&swap_result)?))
            }
        }
    }
}

#[cw_serde]
/// Result of a swap operation.
/// This will dictate how cosmwasm pool module route bank send msgs.
struct SwapResult {
    /// The amount of tokens that swap-er will actaully send to the pool.
    pub token_in: Coin,
    /// The amount of tokens that swap-er will actaully received from the pool.
    pub token_out: Coin,
}

#[cfg(test)]
mod tests {}
