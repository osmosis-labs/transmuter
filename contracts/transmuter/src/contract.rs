use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    ensure, ensure_eq, to_binary, BankMsg, Coin, Decimal, Deps, DepsMut, Env, MessageInfo,
    Response, Uint128,
};
use cw_storage_plus::Item;
use sylvia::contract;

use crate::{
    error::ContractError,
    shares::Shares,
    sudo::{SwapExactAmountInResponseData, SwapExactAmountOutResponseData},
    transmuter_pool::TransmuterPool,
};

// version info for migration info
const CONTRACT_NAME: &str = "crates.io:transmuter";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

const SWAP_FEE: Decimal = Decimal::zero();

pub struct Transmuter<'a> {
    pub(crate) active_status: Item<'a, bool>,
    pub(crate) pool: Item<'a, TransmuterPool>,
    pub(crate) shares: Shares<'a>,
}

#[contract]
impl Transmuter<'_> {
    /// Create a Transmuter instance.
    pub const fn new() -> Self {
        Self {
            active_status: Item::new("active_status"),
            pool: Item::new("pool"),
            shares: Shares::new(),
        }
    }

    /// Instantiate the contract.
    #[msg(instantiate)]
    pub fn instantiate(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        pool_asset_denoms: Vec<String>,
    ) -> Result<Response, ContractError> {
        let (deps, _env, _info) = ctx;

        // store contract version for migration info
        cw2::set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

        // store pool
        self.pool
            .save(deps.storage, &TransmuterPool::new(&pool_asset_denoms))?;

        // set active status to true
        self.active_status.save(deps.storage, &true)?;

        Ok(Response::new()
            .add_attribute("method", "instantiate")
            .add_attribute("contract_name", CONTRACT_NAME)
            .add_attribute("contract_version", CONTRACT_VERSION))
    }

    /// Join pool with tokens that exist in the pool.
    /// Token used to join pool is sent to the contract via `funds` in `MsgExecuteContract`.
    #[msg(exec)]
    pub fn join_pool(&self, ctx: (DepsMut, Env, MessageInfo)) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;

        // ensure funds not empty
        ensure!(
            !info.funds.is_empty(),
            ContractError::AtLeastSingleTokenExpected {}
        );

        let new_shares = Shares::calc_shares(&info.funds)?;

        // update shares
        self.shares
            .add_share(deps.storage, &info.sender, new_shares)?;

        // update pool
        self.pool
            .update(deps.storage, |mut pool| -> Result<_, ContractError> {
                pool.join_pool(&info.funds)?;
                Ok(pool)
            })?;

        Ok(Response::new().add_attribute("method", "join_pool"))
    }

    /// Exit pool with `tokens_out` amount of tokens.
    /// As long as the sender has enough shares, the contract will send `tokens_out` amount of tokens to the sender.
    /// The amount of shares will be deducted from the sender's shares.
    #[msg(exec)]
    pub fn exit_pool(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        tokens_out: Vec<Coin>,
    ) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;

        // check if sender's shares is enough
        let sender_shares = self.shares.get_share(deps.as_ref().storage, &info.sender)?;

        let required_shares = Shares::calc_shares(&tokens_out)?;

        ensure!(
            sender_shares >= required_shares,
            ContractError::InsufficientShares {
                required: required_shares,
                available: sender_shares
            }
        );

        // update shares
        self.shares
            .sub_share(deps.storage, &info.sender, required_shares)?;

        // exit pool
        self.pool
            .update(deps.storage, |mut pool| -> Result<_, ContractError> {
                pool.exit_pool(&tokens_out)?;
                Ok(pool)
            })?;

        let bank_send_msg = BankMsg::Send {
            to_address: info.sender.to_string(),
            amount: tokens_out,
        };

        Ok(Response::new()
            .add_attribute("method", "exit_pool")
            .add_message(bank_send_msg))
    }

    /// SwapExactAmountIn swaps an exact amount of tokens in for as many tokens out as possible.
    /// The amount of tokens out is determined by the current exchange rate and the swap fee.
    /// The user specifies a minimum amount of tokens out, and the transaction will revert if that amount of tokens
    /// is not received.
    #[msg(exec)]
    pub fn swap_exact_amount_in(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        token_in: Coin,
        token_out_denom: String,
        token_out_min_amount: Uint128,
    ) -> Result<Response, ContractError> {
        self._swap_exact_amount_in(
            ctx,
            token_in,
            token_out_denom,
            token_out_min_amount,
            SWAP_FEE,
        )
    }

    /// This is a helper function for `swap_exact_amount_in`. As it will also be used by sudo endpoint.
    pub(crate) fn _swap_exact_amount_in(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        token_in: Coin,
        token_out_denom: String,
        token_out_min_amount: Uint128,
        swap_fee: Decimal,
    ) -> Result<Response, ContractError> {
        let (deps, env, info) = ctx;

        // ensure funds match token_in
        ensure!(
            info.funds.len() == 1 && info.funds[0] == token_in,
            ContractError::FundsMismatchTokenIn {
                funds: info.funds,
                token_in
            }
        );

        let (pool, token_out) =
            self._calc_out_amt_given_in((deps.as_ref(), env), token_in, token_out_denom, swap_fee)?;

        // ensure token_out amount is greater than or equal to token_out_min_amount
        ensure!(
            token_out.amount >= token_out_min_amount,
            ContractError::InsufficientTokenOut {
                required: token_out_min_amount,
                available: token_out.amount
            }
        );

        // save pool
        self.pool.save(deps.storage, &pool)?;

        let send_token_out_to_sender_msg = BankMsg::Send {
            to_address: info.sender.to_string(),
            amount: vec![token_out.clone()],
        };

        let swap_result = SwapExactAmountInResponseData {
            token_out_amount: token_out.amount,
        };

        Ok(Response::new()
            .add_attribute("method", "swap_exact_amount_in")
            .add_message(send_token_out_to_sender_msg)
            .set_data(to_binary(&swap_result)?))
    }

    /// SwapExactAmountOut swaps as many tokens in as possible for an exact amount of tokens out.
    /// The amount of tokens in is determined by the current exchange rate and the swap fee.
    /// The user specifies a maximum amount of tokens in, and the transaction will revert if that amount of tokens
    /// is exceeded.
    #[msg(exec)]
    pub fn swap_exact_amount_out(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        token_in_denom: String,
        token_in_max_amount: Uint128,
        token_out: Coin,
    ) -> Result<Response, ContractError> {
        self._swap_exact_amount_out(
            ctx,
            token_in_denom,
            token_in_max_amount,
            token_out,
            SWAP_FEE,
        )
    }

    fn _swap_exact_amount_out(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        token_in_denom: String,
        token_in_max_amount: Uint128,
        token_out: Coin,
        swap_fee: Decimal,
    ) -> Result<Response, ContractError> {
        let (deps, env, info) = ctx;

        let (pool, token_in) = self._calc_in_amt_given_out(
            (deps.as_ref(), env),
            token_out.clone(),
            token_in_denom,
            swap_fee,
        )?;

        // ensure funds match token_in
        ensure!(
            info.funds.len() == 1 && info.funds[0] == token_in,
            ContractError::FundsMismatchTokenIn {
                funds: info.funds,
                token_in
            }
        );

        // ensure token_in amount is less than or equal to token_in_max_amount
        ensure!(
            token_in.amount <= token_in_max_amount,
            ContractError::ExceedingTokenIn {
                limit: token_in_max_amount,
                required: token_in.amount,
            }
        );

        // save pool
        self.pool.save(deps.storage, &pool)?;

        let send_token_out_to_sender_msg = BankMsg::Send {
            to_address: info.sender.to_string(),
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

    #[msg(query)]
    pub fn get_shares(
        &self,
        ctx: (Deps, Env),
        address: String,
    ) -> Result<SharesResponse, ContractError> {
        let (deps, _env) = ctx;
        Ok(SharesResponse {
            shares: self
                .shares
                .get_share(deps.storage, &deps.api.addr_validate(&address)?)?,
        })
    }

    #[msg(query)]
    pub(crate) fn get_swap_fee(
        &self,
        _ctx: (Deps, Env),
    ) -> Result<GetSwapFeeResponse, ContractError> {
        Ok(GetSwapFeeResponse { swap_fee: SWAP_FEE })
    }

    #[msg(query)]
    pub(crate) fn is_active(&self, ctx: (Deps, Env)) -> Result<IsActiveResponse, ContractError> {
        let (deps, _env) = ctx;
        Ok(IsActiveResponse {
            is_active: self.active_status.load(deps.storage)?,
        })
    }

    #[msg(query)]
    pub(crate) fn get_total_shares(
        &self,
        ctx: (Deps, Env),
    ) -> Result<TotalSharesResponse, ContractError> {
        let (deps, _env) = ctx;
        let total_shares = self.shares.get_total_shares(deps.storage)?;
        Ok(TotalSharesResponse { total_shares })
    }

    #[msg(query)]
    pub(crate) fn get_total_pool_liquidity(
        &self,
        ctx: (Deps, Env),
    ) -> Result<TotalPoolLiquidityResponse, ContractError> {
        let (deps, _env) = ctx;
        let pool = self.pool.load(deps.storage)?;

        Ok(TotalPoolLiquidityResponse {
            total_pool_liquidity: pool.pool_assets,
        })
    }

    #[msg(query)]
    pub(crate) fn spot_price(
        &self,
        ctx: (Deps, Env),
        quote_asset_denom: String,
        base_asset_denom: String,
    ) -> Result<SpotPriceResponse, ContractError> {
        let (deps, _env) = ctx;

        // ensure that it's not the same denom
        ensure!(
            quote_asset_denom != base_asset_denom,
            ContractError::SpotPriceQueryFailed {
                reason: "quote_asset_denom and base_asset_denom cannot be the same".to_string()
            }
        );

        // ensure that qoute asset denom are in pool asset
        let pool = self.pool.load(deps.storage)?;
        ensure!(
            pool.pool_assets
                .iter()
                .any(|c| c.denom == quote_asset_denom),
            ContractError::SpotPriceQueryFailed {
                reason: format!(
                    "quote_asset_denom is not in pool assets: must be one of {:?} but got {}",
                    pool.pool_assets, quote_asset_denom
                )
            }
        );

        // ensure that base asset denom are in pool asset
        ensure!(
            pool.pool_assets.iter().any(|c| c.denom == base_asset_denom),
            ContractError::SpotPriceQueryFailed {
                reason: format!(
                    "base_asset_denom is not in pool assets: must be one of {:?} but got {}",
                    pool.pool_assets, base_asset_denom
                )
            }
        );

        // spot price is always one for both side
        Ok(SpotPriceResponse {
            spot_price: Decimal::one(),
        })
    }

    #[msg(query)]
    pub(crate) fn calc_out_amt_given_in(
        &self,
        ctx: (Deps, Env),
        token_in: Coin,
        token_out_denom: String,
        swap_fee: Decimal,
    ) -> Result<CalcOutAmtGivenInResponse, ContractError> {
        let (_pool, token_out) =
            self._calc_out_amt_given_in(ctx, token_in, token_out_denom, swap_fee)?;

        Ok(CalcOutAmtGivenInResponse { token_out })
    }

    pub(crate) fn _calc_out_amt_given_in(
        &self,
        ctx: (Deps, Env),
        token_in: Coin,
        token_out_denom: String,
        swap_fee: Decimal,
    ) -> Result<(TransmuterPool, Coin), ContractError> {
        let (deps, env) = ctx;

        // ensure swap fee is the same as one from get_swap_fee which essentially is always 0
        // in case where the swap fee mismatch, it can cause the pool to be imbalanced
        let contract_swap_fee = self.get_swap_fee((deps, env))?.swap_fee;
        ensure_eq!(
            swap_fee,
            contract_swap_fee,
            ContractError::InvalidSwapFee {
                expected: contract_swap_fee,
                actual: swap_fee
            }
        );

        let mut pool = self.pool.load(deps.storage)?;
        let token_out = pool.transmute(&token_in, &token_out_denom)?;

        Ok((pool, token_out))
    }

    #[msg(query)]
    pub(crate) fn calc_in_amt_given_out(
        &self,
        ctx: (Deps, Env),
        token_out: Coin,
        token_in_denom: String,
        swap_fee: Decimal,
    ) -> Result<CalcInAmtGivenOutResponse, ContractError> {
        let (deps, env) = ctx;

        // ensure swap fee is the same as one from get_swap_fee which essentially is always 0
        // in case where the swap fee mismatch, it can cause the pool to be imbalanced
        let contract_swap_fee = self.get_swap_fee((deps, env))?.swap_fee;
        ensure_eq!(
            swap_fee,
            contract_swap_fee,
            ContractError::InvalidSwapFee {
                expected: contract_swap_fee,
                actual: swap_fee
            }
        );

        let token_in = Coin::new(token_out.amount.into(), token_in_denom);

        let mut pool = self.pool.load(deps.storage)?;
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

        Ok(CalcInAmtGivenOutResponse { token_in })
    }

    pub(crate) fn _calc_in_amt_given_out(
        &self,
        ctx: (Deps, Env),
        token_out: Coin,
        token_in_denom: String,
        swap_fee: Decimal,
    ) -> Result<(TransmuterPool, Coin), ContractError> {
        let (deps, env) = ctx;

        // ensure swap fee is the same as one from get_swap_fee which essentially is always 0
        // in case where the swap fee mismatch, it can cause the pool to be imbalanced
        let contract_swap_fee = self.get_swap_fee((deps, env))?.swap_fee;
        ensure_eq!(
            swap_fee,
            contract_swap_fee,
            ContractError::InvalidSwapFee {
                expected: contract_swap_fee,
                actual: swap_fee
            }
        );

        let token_in = Coin::new(token_out.amount.into(), token_in_denom);

        let mut pool = self.pool.load(deps.storage)?;
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

        Ok((pool, token_in))
    }
}

#[cw_serde]
pub struct SharesResponse {
    pub shares: Uint128,
}

#[cw_serde]
pub struct GetSwapFeeResponse {
    pub swap_fee: Decimal,
}

#[cw_serde]
pub struct IsActiveResponse {
    pub is_active: bool,
}

#[cw_serde]
pub struct TotalSharesResponse {
    pub total_shares: Uint128,
}

#[cw_serde]
pub struct TotalPoolLiquidityResponse {
    pub total_pool_liquidity: Vec<Coin>,
}

#[cw_serde]
pub struct SpotPriceResponse {
    pub spot_price: Decimal,
}

#[cw_serde]
pub struct CalcOutAmtGivenInResponse {
    pub token_out: Coin,
}

#[cw_serde]
pub struct CalcInAmtGivenOutResponse {
    pub token_in: Coin,
}
