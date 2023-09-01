use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    ensure, to_binary, BankMsg, Coin, Decimal, DepsMut, Env, MessageInfo, Response, Uint128,
};

use crate::{contract::Transmuter, ContractError};

#[cw_serde]
pub enum SudoMsg {
    SetActive {
        is_active: bool,
    },
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
            SudoMsg::SetActive { is_active } => {
                let (deps, _env) = ctx;
                transmuter.active_status.save(deps.storage, &is_active)?;

                Ok(Response::new().add_attribute("method", "set_active"))
            }
            SudoMsg::SwapExactAmountIn {
                sender,
                token_in,
                token_out_denom,
                token_out_min_amount,
                swap_fee,
            } => {
                let method = "swap_exact_amount_in";

                let (deps, env) = ctx;
                let sender = deps.api.addr_validate(&sender)?;

                let alloyed_denom = transmuter.alloyed_asset.get_alloyed_denom(deps.storage)?;

                // if token in is share denom, swap alloyed asset for tokens
                if token_in.denom == alloyed_denom {
                    let token_out = Coin::new(token_in.amount.u128(), token_out_denom);
                    let swap_result = to_binary(&SwapExactAmountInResponseData {
                        token_out_amount: token_out.amount,
                    })?;

                    return transmuter
                        .swap_alloyed_asset_for_tokens(
                            method,
                            (
                                deps,
                                env,
                                MessageInfo {
                                    sender,
                                    funds: vec![token_in],
                                },
                            ),
                            vec![token_out],
                        )
                        .map(|res| res.set_data(swap_result));
                }

                // if token out is share denom, swap token for shares
                if token_out_denom == alloyed_denom {
                    let token_out = Coin::new(token_in.amount.u128(), token_out_denom);
                    let swap_result = to_binary(&SwapExactAmountInResponseData {
                        token_out_amount: token_out.amount,
                    })?;

                    return transmuter
                        .swap_tokens_for_alloyed_asset(
                            method,
                            (
                                deps,
                                env,
                                MessageInfo {
                                    sender,
                                    funds: vec![token_in],
                                },
                            ),
                        )
                        .map(|res| res.set_data(swap_result));
                }

                let (pool, token_out) = transmuter._calc_out_amt_given_in(
                    (deps.as_ref(), env.clone()),
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

                // check and update limiters only if pool assets are not zero
                if let Some(denom_weight_pairs) = pool.weights()? {
                    transmuter.limiters.check_limits_and_update(
                        deps.storage,
                        denom_weight_pairs,
                        env.block.time,
                    )?;
                }

                // save pool
                transmuter.pool.save(deps.storage, &pool)?;

                let send_token_out_to_sender_msg = BankMsg::Send {
                    to_address: sender.to_string(),
                    amount: vec![token_out.clone()],
                };

                let swap_result = SwapExactAmountInResponseData {
                    token_out_amount: token_out.amount,
                };

                Ok(Response::new()
                    .add_attribute("method", method)
                    .add_message(send_token_out_to_sender_msg)
                    .set_data(to_binary(&swap_result)?))
            }
            SudoMsg::SwapExactAmountOut {
                sender,
                token_in_denom,
                token_in_max_amount,
                token_out,
                swap_fee,
            } => {
                let method = "swap_exact_amount_out";
                let (deps, env) = ctx;

                let sender = deps.api.addr_validate(&sender)?;

                let alloyed_denom = transmuter.alloyed_asset.get_alloyed_denom(deps.storage)?;

                // if token in is share denom, swap shares for tokens
                if token_in_denom == alloyed_denom {
                    let token_in = Coin::new(token_out.amount.u128(), token_in_denom);
                    let swap_result = to_binary(&SwapExactAmountOutResponseData {
                        token_in_amount: token_in.amount,
                    })?;

                    return transmuter
                        .swap_alloyed_asset_for_tokens(
                            method,
                            (
                                deps,
                                env,
                                MessageInfo {
                                    sender,
                                    funds: vec![token_in],
                                },
                            ),
                            vec![token_out],
                        )
                        .map(|res| res.set_data(swap_result));
                }

                // if token out is share denom, swap token for shares
                if token_out.denom == alloyed_denom {
                    let token_in = Coin::new(token_out.amount.u128(), token_in_denom);
                    let swap_result = to_binary(&SwapExactAmountOutResponseData {
                        token_in_amount: token_in.amount,
                    })?;

                    return transmuter
                        .swap_tokens_for_alloyed_asset(
                            method,
                            (
                                deps,
                                env,
                                MessageInfo {
                                    sender,
                                    funds: vec![token_in],
                                },
                            ),
                        )
                        .map(|res| res.set_data(swap_result));
                }

                let (pool, token_in) = transmuter._calc_in_amt_given_out(
                    (deps.as_ref(), env.clone()),
                    token_out.clone(),
                    token_in_denom,
                    swap_fee,
                )?;

                ensure!(
                    token_in.amount <= token_in_max_amount,
                    ContractError::ExcessiveRequiredTokenIn {
                        limit: token_in_max_amount,
                        required: token_in.amount,
                    }
                );

                // check and update limiters only if pool assets are not zero
                if let Some(denom_weight_pairs) = pool.weights()? {
                    transmuter.limiters.check_limits_and_update(
                        deps.storage,
                        denom_weight_pairs,
                        env.block.time,
                    )?;
                }

                // save pool
                transmuter.pool.save(deps.storage, &pool)?;

                let send_token_out_to_sender_msg = BankMsg::Send {
                    to_address: sender.to_string(),
                    amount: vec![token_out],
                };

                let swap_result = SwapExactAmountOutResponseData {
                    token_in_amount: token_in.amount,
                };

                Ok(Response::new()
                    .add_attribute("method", "swap_exact_amount_out")
                    .add_message(send_token_out_to_sender_msg)
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
