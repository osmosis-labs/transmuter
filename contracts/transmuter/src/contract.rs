use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    ensure, ensure_eq, BankMsg, Coin, Decimal, Deps, DepsMut, Env, MessageInfo, Reply, Response,
    StdError, SubMsg, Uint128,
};
use cw_storage_plus::Item;
use osmosis_std::types::osmosis::tokenfactory::v1beta1::{
    MsgBurn, MsgCreateDenom, MsgCreateDenomResponse, MsgMint,
};
use sylvia::contract;

use crate::{error::ContractError, shares::Shares, transmuter_pool::TransmuterPool};

// version info for migration info
const CONTRACT_NAME: &str = "crates.io:transmuter";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

const SWAP_FEE: Decimal = Decimal::zero();

const CREATE_LP_DENOM_REPLY_ID: u64 = 1;

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
        let (deps, env, _info) = ctx;

        // store contract version for migration info
        cw2::set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

        // store pool
        self.pool
            .save(deps.storage, &TransmuterPool::new(&pool_asset_denoms))?;

        // set active status to true
        self.active_status.save(deps.storage, &true)?;

        // create lp denom
        let msg_create_lp_denom = SubMsg::reply_on_success(
            MsgCreateDenom {
                sender: env.contract.address.to_string(),
                subdenom: "transmuter/poolshare".to_owned(),
            },
            CREATE_LP_DENOM_REPLY_ID,
        );

        Ok(Response::new()
            .add_attribute("method", "instantiate")
            .add_attribute("contract_name", CONTRACT_NAME)
            .add_attribute("contract_version", CONTRACT_VERSION)
            .add_submessage(msg_create_lp_denom))
    }

    pub fn reply(&self, ctx: (DepsMut, Env), msg: Reply) -> Result<Response, ContractError> {
        let (deps, _env) = ctx;

        match msg.id {
            CREATE_LP_DENOM_REPLY_ID => {
                // register created token denom
                let MsgCreateDenomResponse { new_token_denom } = msg.result.try_into()?;
                self.shares
                    .set_share_denom(deps.storage, &new_token_denom)?;

                Ok(Response::new().add_attribute("lp_denom", new_token_denom))
            }
            _ => Err(StdError::not_found(format!("No reply handler found for: {:?}", msg)).into()),
        }
    }

    /// Join pool with tokens that exist in the pool.
    /// Token used to join pool is sent to the contract via `funds` in `MsgExecuteContract`.
    #[msg(exec)]
    pub fn join_pool(&self, ctx: (DepsMut, Env, MessageInfo)) -> Result<Response, ContractError> {
        let (deps, env, info) = ctx;

        // ensure funds not empty
        ensure!(
            !info.funds.is_empty(),
            ContractError::AtLeastSingleTokenExpected {}
        );

        // update pool
        self.pool
            .update(deps.storage, |mut pool| -> Result<_, ContractError> {
                pool.join_pool(&info.funds)?;
                Ok(pool)
            })?;

        // mint lp tokens
        let share_denom = self.shares.get_share_denom(deps.storage)?;
        let new_shares = Shares::calc_shares(&info.funds)?;
        let mint_msg = MsgMint {
            sender: env.contract.address.to_string(),
            amount: Some(Coin::new(new_shares.u128(), share_denom).into()),
            mint_to_address: info.sender.to_string(),
        };

        Ok(Response::new()
            .add_attribute("method", "join_pool")
            .add_message(mint_msg))
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
        let (deps, env, info) = ctx;

        // check if sender's shares is enough
        let sender_shares = self.shares.get_share(deps.as_ref(), &info.sender)?;

        let required_shares = Shares::calc_shares(&tokens_out)?;

        ensure!(
            sender_shares >= required_shares,
            ContractError::InsufficientShares {
                required: required_shares,
                available: sender_shares
            }
        );

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

        // burn lp tokens
        let share_denom = self.shares.get_share_denom(deps.storage)?;
        let burn_msg = MsgBurn {
            sender: env.contract.address.to_string(),
            amount: Some(Coin::new(required_shares.u128(), share_denom).into()),
            burn_from_address: info.sender.to_string(),
        };

        Ok(Response::new()
            .add_attribute("method", "exit_pool")
            .add_message(burn_msg)
            .add_message(bank_send_msg))
    }

    #[msg(query)]
    pub fn get_shares(
        &self,
        ctx: (Deps, Env),
        address: String,
    ) -> Result<GetSharesResponse, ContractError> {
        let (deps, _env) = ctx;
        Ok(GetSharesResponse {
            shares: self
                .shares
                .get_share(deps, &deps.api.addr_validate(&address)?)?,
        })
    }

    #[msg(query)]
    pub(crate) fn get_share_denom(
        &self,
        ctx: (Deps, Env),
    ) -> Result<GetShareDenomResponse, ContractError> {
        let (deps, _env) = ctx;
        Ok(GetShareDenomResponse {
            share_denom: self.shares.get_share_denom(deps.storage)?,
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
    ) -> Result<GetTotalSharesResponse, ContractError> {
        let (deps, _env) = ctx;
        let total_shares = self.shares.get_total_shares(deps)?;
        Ok(GetTotalSharesResponse { total_shares })
    }

    #[msg(query)]
    pub(crate) fn get_total_pool_liquidity(
        &self,
        ctx: (Deps, Env),
    ) -> Result<GetTotalPoolLiquidityResponse, ContractError> {
        let (deps, _env) = ctx;
        let pool = self.pool.load(deps.storage)?;

        Ok(GetTotalPoolLiquidityResponse {
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
                    pool.pool_assets
                        .iter()
                        .map(|coin| coin.denom.clone())
                        .collect::<Vec<_>>(),
                    quote_asset_denom
                )
            }
        );

        // ensure that base asset denom are in pool asset
        ensure!(
            pool.pool_assets.iter().any(|c| c.denom == base_asset_denom),
            ContractError::SpotPriceQueryFailed {
                reason: format!(
                    "base_asset_denom is not in pool assets: must be one of {:?} but got {}",
                    pool.pool_assets
                        .iter()
                        .map(|coin| coin.denom.clone())
                        .collect::<Vec<_>>(),
                    base_asset_denom
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
pub struct GetSharesResponse {
    pub shares: Uint128,
}

#[cw_serde]
pub struct GetShareDenomResponse {
    pub share_denom: String,
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
pub struct GetTotalSharesResponse {
    pub total_shares: Uint128,
}

#[cw_serde]
pub struct GetTotalPoolLiquidityResponse {
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
