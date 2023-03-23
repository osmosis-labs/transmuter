use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    ensure, ensure_eq, Addr, BankMsg, Coin, Decimal, Deps, DepsMut, Env, MessageInfo, Response,
    StdError, Uint128,
};
use cw_storage_plus::{Item, Map};
use sylvia::contract;

use crate::{error::ContractError, transmuter_pool::TransmuterPool};

// version info for migration info
const CONTRACT_NAME: &str = "crates.io:transmuter";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

const SWAP_FEE: Decimal = Decimal::zero();
const EXIT_FEE: Decimal = Decimal::zero();

pub struct Transmuter<'a> {
    pub(crate) pool: Item<'a, TransmuterPool>,
    pub(crate) shares: Map<'a, &'a Addr, Uint128>,
    pub(crate) total_shares: Item<'a, Uint128>,
}

#[contract]
impl Transmuter<'_> {
    /// Create a Transmuter instance.
    pub const fn new() -> Self {
        Self {
            pool: Item::new("pool"),
            shares: Map::new("shares"),
            total_shares: Item::new("total_shares"),
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

        // init total_shares
        self.total_shares.save(deps.storage, &Uint128::zero())?;

        Ok(Response::new()
            .add_attribute("method", "instantiate")
            .add_attribute("contract_name", CONTRACT_NAME)
            .add_attribute("contract_version", CONTRACT_VERSION))
    }

    /// Join pool with tokens that exist in the pool.
    /// Token used to join pool is sent to the contract via `funds` in `MsgExecuteContract`.
    #[msg(exec)]
    fn join_pool(&self, ctx: (DepsMut, Env, MessageInfo)) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;

        // ensure funds not empty
        ensure!(
            !info.funds.is_empty(),
            ContractError::AtLeastSingleTokenExpected {}
        );

        let new_shares = info
            .funds
            .iter()
            .fold(Uint128::zero(), |acc, c| acc + c.amount);

        // update shares
        self.shares.update(
            deps.storage,
            &info.sender,
            |shares| -> Result<Uint128, StdError> {
                shares
                    .unwrap_or_default()
                    .checked_add(new_shares)
                    .map_err(StdError::overflow)
            },
        )?;

        // update total shares
        self.total_shares.update(deps.storage, |shares| {
            shares.checked_add(new_shares).map_err(StdError::overflow)
        })?;

        // update pool
        self.pool
            .update(deps.storage, |mut pool| -> Result<_, ContractError> {
                pool.join_pool(&info.funds)?;
                Ok(pool)
            })?;

        Ok(Response::new().add_attribute("method", "join_pool"))
        // TODO: Band::Send to module account
    }

    /// Transmute recived token_in from `MsgExecuteContract`'s funds to `token_out_denom`.
    /// Send `token_out` back to the msg sender with 1:1 ratio.
    #[msg(exec)]
    fn transmute(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        token_out_denom: String,
    ) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;

        // ensure funds length == 1
        ensure_eq!(info.funds.len(), 1, ContractError::SingleTokenExpected {});

        // transmute
        let mut pool = self.pool.load(deps.storage)?;
        let token_in = info.funds[0].clone();
        let token_out = pool.transmute(&token_in, &token_out_denom)?;

        // save pool
        self.pool.save(deps.storage, &pool)?;

        let bank_send_msg = BankMsg::Send {
            to_address: info.sender.to_string(),
            amount: vec![token_out],
        };

        Ok(Response::new()
            .add_attribute("method", "transmute")
            .add_message(bank_send_msg))
    }

    /// Exit pool with `tokens_out` amount of tokens.
    /// As long as the sender has enough shares, the contract will send `tokens_out` amount of tokens to the sender.
    /// The amount of shares will be deducted from the sender's shares.
    #[msg(exec)]
    fn exit_pool(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        tokens_out: Vec<Coin>,
    ) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;

        // check if sender's shares is enough
        let sender_shares = self
            .shares
            .may_load(deps.storage, &info.sender)?
            .unwrap_or_default();

        let required_shares = tokens_out
            .iter()
            .fold(Uint128::zero(), |acc, curr| acc + curr.amount);

        ensure!(
            sender_shares >= required_shares,
            ContractError::InsufficientShares {
                required: required_shares,
                available: sender_shares
            }
        );

        // update shares
        self.shares.update(
            deps.storage,
            &info.sender,
            |sender_shares| -> Result<Uint128, StdError> {
                sender_shares
                    .unwrap_or_default()
                    .checked_sub(required_shares)
                    .map_err(StdError::overflow)
            },
        )?;

        // update total shares
        self.total_shares.update(deps.storage, |shares| {
            shares
                .checked_sub(required_shares)
                .map_err(StdError::overflow)
        })?;

        // exit pool
        self.pool
            .update(deps.storage, |mut pool| -> Result<_, ContractError> {
                pool.exit_pool(&tokens_out)?;
                Ok(pool)
            })?;

        // TODO: authz::MsgExec this with grant for module account
        let bank_send_msg = BankMsg::Send {
            to_address: info.sender.to_string(),
            amount: tokens_out,
        };

        Ok(Response::new()
            .add_attribute("method", "exit_pool")
            .add_message(bank_send_msg))
    }

    /// Query the pool information of the contract.
    #[msg(query)]
    fn pool(&self, ctx: (Deps, Env)) -> Result<PoolResponse, ContractError> {
        let (deps, _env) = ctx;
        Ok(PoolResponse {
            pool: self.pool.load(deps.storage)?,
        })
    }

    #[msg(query)]
    fn shares(&self, ctx: (Deps, Env), address: String) -> Result<SharesResponse, ContractError> {
        let (deps, _env) = ctx;
        Ok(SharesResponse {
            shares: self
                .shares
                .may_load(deps.storage, &deps.api.addr_validate(&address)?)?
                .unwrap_or_default(),
        })
    }

    // // query msg:
    // // { "get_swap_fee": {} }
    // // response:
    // // { "swap_fee": <swap_fee:string> }
    // GetSwapFee(ctx sdk.Context) sdk.Dec
    #[msg(query)]
    pub(crate) fn get_swap_fee(&self, _ctx: (Deps, Env)) -> Result<SwapFeeResponse, ContractError> {
        Ok(SwapFeeResponse { swap_fee: SWAP_FEE })
    }

    // // query msg:
    // // { "get_exit_fee": {} }
    // // response:
    // // { "exit_fee": <exit_fee:string> }
    // GetExitFee(ctx sdk.Context) sdk.Dec
    #[msg(query)]
    pub(crate) fn get_exit_fee(&self, _ctx: (Deps, Env)) -> Result<ExitFeeResponse, ContractError> {
        Ok(ExitFeeResponse { exit_fee: EXIT_FEE })
    }

    // // query msg:
    // // { "is_active": {} }
    // // response:
    // // { "is_active": <is_active:boolean> }
    // IsActive(ctx sdk.Context) bool
    #[msg(query)]
    pub(crate) fn is_active(&self, _ctx: (Deps, Env)) -> Result<IsActiveResponse, ContractError> {
        Ok(IsActiveResponse { is_active: true })
    }

    // // query msg:
    // // { "get_total_shares": {} }
    // // response:
    // // { "total_shares": <total_shares:number> }
    // GetTotalShares() sdk.Int
    #[msg(query)]
    pub(crate) fn get_total_shares(
        &self,
        ctx: (Deps, Env),
    ) -> Result<TotalSharesResponse, ContractError> {
        let (deps, _env) = ctx;
        let total_shares = self.total_shares.load(deps.storage)?;
        Ok(TotalSharesResponse { total_shares })
    }

    // // query msg:
    // // { "get_total_pool_liquidity": {} }
    // // response:
    // // { "total_pool_liquidity": [{ "denom": <denom:string>, "amount": <amount:string> }>,..] }
    // GetTotalPoolLiquidity(ctx sdk.Context) sdk.Coins
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

    // // query msg:
    // // { "spot_price": { quote_asset_denom: <quote_asset_denom>, base_asset_denom: <base_asset_denom> } }
    // // response:
    // // { "spot_price": <spot_price:string> }
    // SpotPrice(ctx sdk.Context, quoteAssetDenom string, baseAssetDenom string) (sdk.Dec, error)
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

    // // query msg:
    // // {
    // //   "calc_out_given_in": {
    // //     "token_in": { "denom": <denom:string>, "amount": <amount:string> },
    // //     "token_out_denom": <token_out_denom:string>,
    // //     "swap_fee": <swap_fee:string>,
    // //   }
    // // }
    // // response data:
    // // { "token_out": { "denom": <denom:string>, "amount": <amount:string> } }
    // CalcOutAmtGivenIn(
    //     ctx sdk.Context,
    //     poolI PoolI,
    //     tokenIn sdk.Coin,
    //     tokenOutDenom string,
    //     swapFee sdk.Dec,
    //   ) (tokenOut sdk.Coin, err error)
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

    // // query msg:
    // // {
    // //   "calc_in_given_out": {
    // //     "token_in_denom": <token_in_denom:string>,
    // //     "token_out": { "denom": <denom:string>, "amount": <amount:string> },
    // //     "swap_fee": <swap_fee:string>,
    // //   }
    // // }
    // // response data:
    // // { "token_in": { "denom": <denom:string>, "amount": <amount:string> } }
    // CalcInAmtGivenOut(
    //     ctx sdk.Context,
    //     poolI PoolI,
    //     tokenOut sdk.Coin,
    //     tokenInDenom string,
    //     swapFee sdk.Dec,
    //   ) (tokenIn sdk.Coin, err error)
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
pub struct PoolResponse {
    pub pool: TransmuterPool,
}

#[cw_serde]
pub struct SwapFeeResponse {
    pub swap_fee: Decimal,
}

#[cw_serde]
pub struct ExitFeeResponse {
    pub exit_fee: Decimal,
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
