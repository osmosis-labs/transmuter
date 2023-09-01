use crate::{
    admin::Admin,
    ensure_admin_authority,
    error::ContractError,
    limiter::{Limiter, LimiterParams, Limiters, WindowConfig},
    shares::Shares,
    transmuter_pool::TransmuterPool,
};
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    ensure, ensure_eq, Addr, BankMsg, Coin, Decimal, Deps, DepsMut, Env, MessageInfo, Reply,
    Response, StdError, SubMsg, Uint128,
};

use cw_storage_plus::Item;
use osmosis_std::types::{
    cosmos::bank::v1beta1::Metadata,
    osmosis::tokenfactory::v1beta1::{
        MsgBurn, MsgCreateDenom, MsgCreateDenomResponse, MsgMint, MsgSetDenomMetadata,
    },
};
use sylvia::contract;

/// version info for migration
const CONTRACT_NAME: &str = "crates.io:transmuter";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Swap fee is hardcoded to zero intentionally.
const SWAP_FEE: Decimal = Decimal::zero();

const CREATE_LP_DENOM_REPLY_ID: u64 = 1;

/// Prefix for alloyed asset denom
const ALLOYED_PREFIX: &str = "alloyed";

pub struct Transmuter<'a> {
    pub(crate) active_status: Item<'a, bool>,
    pub(crate) pool: Item<'a, TransmuterPool>,
    pub(crate) shares: Shares<'a>,
    pub(crate) admin: Admin<'a>,
    pub(crate) limiters: Limiters<'a>,
}

#[contract]
#[error(ContractError)]
impl Transmuter<'_> {
    /// Create a Transmuter instance.
    pub const fn new() -> Self {
        Self {
            active_status: Item::new("active_status"),
            pool: Item::new("pool"),
            shares: Shares::new("share_denom"),
            admin: Admin::new("admin"),
            limiters: Limiters::new("limiters"),
        }
    }

    /// Instantiate the contract.
    #[msg(instantiate)]
    pub fn instantiate(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        pool_asset_denoms: Vec<String>,
        alloyed_asset_subdenom: String,
        admin: Option<String>,
    ) -> Result<Response, ContractError> {
        let (deps, env, _info) = ctx;

        // store contract version for migration info
        cw2::set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

        // set admin if exists
        if let Some(admin) = admin {
            self.admin
                .init(deps.storage, deps.api.addr_validate(&admin)?)?;
        }

        // store pool
        self.pool
            .save(deps.storage, &TransmuterPool::new(&pool_asset_denoms))?;

        // set active status to true
        self.active_status.save(deps.storage, &true)?;

        // create lp denom
        let msg_create_lp_denom = SubMsg::reply_on_success(
            MsgCreateDenom {
                sender: env.contract.address.to_string(),
                subdenom: format!("{}/{}", ALLOYED_PREFIX, alloyed_asset_subdenom),
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

                Ok(Response::new().add_attribute("pool_share_denom", new_token_denom))
            }
            _ => Err(StdError::not_found(format!("No reply handler found for: {:?}", msg)).into()),
        }
    }

    // === executes ===

    #[msg(exec)]
    fn register_limiter(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        denom: String,
        label: String,
        limiter_params: LimiterParams,
    ) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;

        // only admin can register limiter
        ensure_admin_authority!(info.sender, self.admin, deps.as_ref());

        let base_attrs = vec![
            ("method", "register_limiter"),
            ("denom", &denom),
            ("label", &label),
        ];
        let limiter_attrs = match &limiter_params {
            LimiterParams::ChangeLimiter {
                window_config,
                boundary_offset,
            } => {
                let window_size = window_config.window_size.to_string();
                let division_count = window_config.division_count.to_string();
                let boundary_offset_string = boundary_offset.to_string();

                vec![
                    (String::from("limiter_type"), String::from("change_limiter")),
                    (String::from("window_size"), window_size),
                    (String::from("division_count"), division_count),
                    (String::from("boundary_offset"), boundary_offset_string),
                ]
            }
            LimiterParams::StaticLimiter { upper_limit } => vec![
                (String::from("limiter_type"), String::from("static_limiter")),
                (String::from("upper_limit"), upper_limit.to_string()),
            ],
        };

        // register limiter
        self.limiters
            .register(deps.storage, &denom, &label, limiter_params)?;

        Ok(Response::new()
            .add_attributes(base_attrs)
            .add_attributes(limiter_attrs))
    }

    #[msg(exec)]
    fn deregister_limiter(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        denom: String,
        label: String,
    ) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;

        // only admin can deregister limiter
        ensure_admin_authority!(info.sender, self.admin, deps.as_ref());

        let attrs = vec![
            ("method", "deregister_limiter"),
            ("denom", &denom),
            ("label", &label),
        ];

        // deregister limiter
        self.limiters.deregister(deps.storage, &denom, &label);

        Ok(Response::new().add_attributes(attrs))
    }

    #[msg(exec)]
    fn set_boundary_offset(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        denom: String,
        label: String,
        boundary_offset: Decimal,
    ) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;

        // only admin can set boundary offset
        ensure_admin_authority!(info.sender, self.admin, deps.as_ref());

        let boundary_offset_string = boundary_offset.to_string();
        let attrs = vec![
            ("method", "set_boundary_offset"),
            ("denom", &denom),
            ("label", &label),
            ("boundary_offset", boundary_offset_string.as_str()),
        ];

        // set boundary offset
        self.limiters
            .set_boundary_offset(deps.storage, &denom, &label, boundary_offset)?;

        Ok(Response::new().add_attributes(attrs))
    }

    #[msg(exec)]
    pub fn set_lp_denom_metadata(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        metadata: Metadata,
    ) -> Result<Response, ContractError> {
        let (deps, env, info) = ctx;

        // only admin can set denom metadata
        ensure_admin_authority!(info.sender, self.admin, deps.as_ref());

        let msg_set_denom_metadata = MsgSetDenomMetadata {
            sender: env.contract.address.to_string(),
            metadata: Some(metadata),
        };

        Ok(Response::new()
            .add_attribute("method", "set_lp_denom_metadata")
            .add_message(msg_set_denom_metadata))
    }

    #[msg(exec)]
    fn set_active_status(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        active: bool,
    ) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;

        // only admin can set active status
        ensure_admin_authority!(info.sender, self.admin, deps.as_ref());

        // set active status
        self.active_status.save(deps.storage, &active)?;

        Ok(Response::new()
            .add_attribute("method", "set_active_status")
            .add_attribute("active", active.to_string()))
    }

    /// Join pool with tokens that exist in the pool.
    /// Token used to join pool is sent to the contract via `funds` in `MsgExecuteContract`.
    #[msg(exec)]
    pub fn join_pool(&self, ctx: (DepsMut, Env, MessageInfo)) -> Result<Response, ContractError> {
        self.swap_tokens_for_shares("join_pool", ctx)
    }

    pub fn swap_tokens_for_shares(
        &self,
        method: &str,
        ctx: (DepsMut, Env, MessageInfo),
    ) -> Result<Response, ContractError> {
        let (deps, env, info) = ctx;

        // ensure funds not empty
        ensure!(
            !info.funds.is_empty(),
            ContractError::AtLeastSingleTokenExpected {}
        );

        // join pool
        let mut pool = self.pool.load(deps.storage)?;
        pool.join_pool(&info.funds)?;

        // check and update limiters only if pool assets are not zero
        if let Some(denom_weight_pairs) = pool.weights()? {
            self.limiters.check_limits_and_update(
                deps.storage,
                denom_weight_pairs,
                env.block.time,
            )?;
        }

        self.pool.save(deps.storage, &pool)?;

        // mint lp tokens
        let share_denom = self.shares.get_share_denom(deps.storage)?;
        let new_shares = Shares::calc_shares(&info.funds)?;
        let mint_msg = MsgMint {
            sender: env.contract.address.to_string(),
            amount: Some(Coin::new(new_shares.u128(), share_denom).into()),
            mint_to_address: info.sender.to_string(),
        };

        Ok(Response::new()
            .add_attribute("method", method)
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
        self.swap_shares_for_tokens("exit_pool", ctx, tokens_out)
    }

    pub fn swap_shares_for_tokens(
        &self,
        method: &str,
        ctx: (DepsMut, Env, MessageInfo),
        tokens_out: Vec<Coin>,
    ) -> Result<Response, ContractError> {
        let (deps, env, info) = ctx;

        let share_denom = self.shares.get_share_denom(deps.storage)?;

        // if funds contains one coin and is share token, use that as sender's share
        let (sender_shares, burn_from_address) = if info.funds.is_empty() {
            let sender_shares = self.shares.get_share(deps.as_ref(), &info.sender)?;
            let burn_from_address = info.sender.to_string();
            (sender_shares, burn_from_address)
        } else {
            ensure!(info.funds.len() == 1, ContractError::SingleTokenExpected {});
            ensure!(
                info.funds[0].denom == share_denom,
                ContractError::UnexpectedDenom {
                    expected: info.funds[0].clone().denom,
                    actual: share_denom
                }
            );

            let sender_shares = info.funds[0].amount;
            let burn_from_address = env.contract.address.to_string();
            (sender_shares, burn_from_address)
        };

        // check if sender's shares is enough
        let required_shares = Shares::calc_shares(&tokens_out)?;

        ensure!(
            sender_shares >= required_shares,
            ContractError::InsufficientShares {
                required: required_shares,
                available: sender_shares
            }
        );

        // exit pool
        let mut pool = self.pool.load(deps.storage)?;
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
            to_address: info.sender.to_string(),
            amount: tokens_out,
        };

        // burn lp tokens
        let share_denom = self.shares.get_share_denom(deps.storage)?;
        let burn_msg = MsgBurn {
            sender: env.contract.address.to_string(),
            amount: Some(Coin::new(required_shares.u128(), share_denom).into()),
            burn_from_address,
        };

        Ok(Response::new()
            .add_attribute("method", method)
            .add_message(burn_msg)
            .add_message(bank_send_msg))
    }

    // === queries ===

    #[msg(query)]
    fn list_limiters(&self, ctx: (Deps, Env)) -> Result<ListLimitersResponse, ContractError> {
        let (deps, _env) = ctx;

        let limiters = self.limiters.list_limiters(deps.storage)?;

        Ok(ListLimitersResponse { limiters })
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
        // TODO: handling this more thoughtfully
        let share_denom = self.get_share_denom(ctx.clone())?.share_denom;

        let token_in = Coin::new(token_out.amount.u128(), token_in_denom.clone());

        if token_in_denom == share_denom || token_out.denom == share_denom {
            return Ok(CalcInAmtGivenOutResponse { token_in });
        }

        let (_pool, token_in) =
            self._calc_in_amt_given_out(ctx, token_out, token_in_denom, swap_fee)?;

        let token_in = Coin::new(token_in.amount.u128(), share_denom);

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

    // --- admin ---

    #[msg(exec)]
    pub fn transfer_admin(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        candidate: String,
    ) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;

        let candidate_addr = deps.api.addr_validate(&candidate)?;
        self.admin.transfer(deps, info.sender, candidate_addr)?;

        Ok(Response::new()
            .add_attribute("method", "transfer_admin")
            .add_attribute("andidate", candidate))
    }

    #[msg(exec)]
    pub fn claim_admin(&self, ctx: (DepsMut, Env, MessageInfo)) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;
        let sender_string = info.sender.to_string();
        self.admin.claim(deps, info.sender)?;

        Ok(Response::new()
            .add_attribute("method", "claim_admin")
            .add_attribute("new_admin", sender_string))
    }

    #[msg(query)]
    fn get_admin(&self, ctx: (Deps, Env)) -> Result<GetAdminResponse, ContractError> {
        let (deps, _env) = ctx;

        Ok(GetAdminResponse {
            admin: self.admin.current(deps)?,
        })
    }

    #[msg(query)]
    fn get_admin_candidate(
        &self,
        ctx: (Deps, Env),
    ) -> Result<GetAdminCandidateResponse, ContractError> {
        let (deps, _env) = ctx;

        Ok(GetAdminCandidateResponse {
            admin_candidate: self.admin.candidate(deps)?,
        })
    }
}

#[cw_serde]
pub struct ListLimitersResponse {
    pub limiters: Vec<((String, String), Limiter)>,
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

#[cw_serde]
pub struct GetAdminResponse {
    pub admin: Addr,
}

#[cw_serde]
pub struct GetAdminCandidateResponse {
    pub admin_candidate: Option<Addr>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limiter::ChangeLimiter;
    use crate::sudo::SudoMsg;
    use crate::*;
    use cosmwasm_std::testing::{mock_dependencies, mock_env, mock_info};
    use cosmwasm_std::{attr, from_binary, Uint64};

    #[test]
    fn test_set_active_status() {
        let mut deps = mock_dependencies();

        let admin = "admin";
        let init_msg = InstantiateMsg {
            pool_asset_denoms: vec!["uosmo".to_string(), "uion".to_string()],
            alloyed_asset_subdenom: "uosmouion".to_string(),
            admin: Some(admin.to_string()),
        };
        let env = mock_env();
        let info = mock_info(admin, &[]);

        // Instantiate the contract.
        instantiate(deps.as_mut(), env.clone(), info.clone(), init_msg).unwrap();

        // Manually set share denom
        let share_denom = "uosmo".to_string();

        let transmuter = Transmuter::new();
        transmuter
            .shares
            .set_share_denom(&mut deps.storage, &share_denom)
            .unwrap();

        // Check the initial active status.
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::IsActive {}),
        )
        .unwrap();
        let active_status: IsActiveResponse = from_binary(&res).unwrap();
        assert!(active_status.is_active);

        // Attempt to set the active status by a non-admin user.
        let non_admin_info = mock_info("non_admin", &[]);
        let non_admin_msg = ContractExecMsg::Transmuter(ExecMsg::SetActiveStatus { active: false });
        let err = execute(deps.as_mut(), env.clone(), non_admin_info, non_admin_msg).unwrap_err();

        assert_eq!(err, ContractError::Unauthorized {});

        // Set the active status to false.
        let msg = ContractExecMsg::Transmuter(ExecMsg::SetActiveStatus { active: false });
        execute(deps.as_mut(), env.clone(), info, msg).unwrap();

        // Check the active status again.
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::IsActive {}),
        )
        .unwrap();
        let active_status: IsActiveResponse = from_binary(&res).unwrap();
        assert!(!active_status.is_active);

        // Test that JoinPool is blocked when active status is false
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        let err = execute(
            deps.as_mut(),
            env.clone(),
            mock_info("user", &[Coin::new(1000, "uion"), Coin::new(1000, "uosmo")]),
            join_pool_msg,
        )
        .unwrap_err();
        assert_eq!(err, ContractError::InactivePool {});

        // Test that SwapExactAmountIn is blocked when active status is false
        let swap_exact_amount_in_msg = SudoMsg::SwapExactAmountIn {
            token_in: Coin::new(1000, "uion"),
            swap_fee: Decimal::zero(),
            sender: "mock_sender".to_string(),
            token_out_denom: "uosmo".to_string(),
            token_out_min_amount: Uint128::new(500),
        };
        let err = sudo(deps.as_mut(), env.clone(), swap_exact_amount_in_msg).unwrap_err();
        assert_eq!(err, ContractError::InactivePool {});

        // Test that SwapExactAmountOut is blocked when active status is false
        let swap_exact_amount_out_msg = SudoMsg::SwapExactAmountOut {
            sender: "mock_sender".to_string(),
            token_out: Coin::new(500, "uosmo"),
            swap_fee: Decimal::zero(),
            token_in_denom: "uion".to_string(),
            token_in_max_amount: Uint128::new(1000),
        };
        let err = sudo(deps.as_mut(), env.clone(), swap_exact_amount_out_msg).unwrap_err();
        assert_eq!(err, ContractError::InactivePool {});

        // Test that ExitPool is blocked when active status is false
        let exit_pool_msg = ContractExecMsg::Transmuter(ExecMsg::ExitPool {
            tokens_out: vec![Coin::new(1000, "uion"), Coin::new(1000, "uosmo")],
        });
        let err = execute(
            deps.as_mut(),
            env.clone(),
            mock_info("user", &[Coin::new(1000, "uion"), Coin::new(1000, "uosmo")]),
            exit_pool_msg,
        )
        .unwrap_err();
        assert_eq!(err, ContractError::InactivePool {});

        // Set the active status back to true
        let msg = ContractExecMsg::Transmuter(ExecMsg::SetActiveStatus { active: true });
        execute(deps.as_mut(), env.clone(), mock_info(admin, &[]), msg).unwrap();

        // Check the active status again.
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::IsActive {}),
        )
        .unwrap();
        let active_status: IsActiveResponse = from_binary(&res).unwrap();
        assert!(active_status.is_active);

        // Test that JoinPool is active when active status is true
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        let res = execute(
            deps.as_mut(),
            env.clone(),
            mock_info("user", &[Coin::new(1000, "uion"), Coin::new(1000, "uosmo")]),
            join_pool_msg,
        );
        assert!(res.is_ok());

        // Test that SwapExactAmountIn is active when active status is true
        let swap_exact_amount_in_msg = SudoMsg::SwapExactAmountIn {
            token_in: Coin::new(100, "uion"),
            swap_fee: Decimal::zero(),
            sender: "mock_sender".to_string(),
            token_out_denom: "uosmo".to_string(),
            token_out_min_amount: Uint128::new(100),
        };
        let res = sudo(deps.as_mut(), env.clone(), swap_exact_amount_in_msg);
        assert!(res.is_ok());

        // Test that SwapExactAmountOut is active when active status is true
        let swap_exact_amount_out_msg = SudoMsg::SwapExactAmountOut {
            sender: "mock_sender".to_string(),
            token_out: Coin::new(100, "uosmo"),
            swap_fee: Decimal::zero(),
            token_in_denom: "uion".to_string(),
            token_in_max_amount: Uint128::new(100),
        };
        let res = sudo(deps.as_mut(), env, swap_exact_amount_out_msg);

        assert!(res.is_ok());
    }

    #[test]
    fn test_transfer_and_claim_admin() {
        let mut deps = mock_dependencies();

        let admin = "admin";
        let candidate = "candidate";
        let init_msg = InstantiateMsg {
            pool_asset_denoms: vec!["uosmo".to_string(), "uion".to_string()],
            admin: Some(admin.to_string()),
            alloyed_asset_subdenom: "usomoion".to_string(),
        };
        let env = mock_env();
        let info = mock_info(admin, &[]);

        // Instantiate the contract.
        instantiate(deps.as_mut(), env.clone(), info.clone(), init_msg).unwrap();

        // Transfer admin rights to the candidate
        let transfer_admin_msg = ContractExecMsg::Transmuter(ExecMsg::TransferAdmin {
            candidate: candidate.to_string(),
        });
        execute(deps.as_mut(), env.clone(), info, transfer_admin_msg).unwrap();

        // Check the admin candidate
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::GetAdminCandidate {}),
        )
        .unwrap();
        let admin_candidate: GetAdminCandidateResponse = from_binary(&res).unwrap();
        assert_eq!(admin_candidate.admin_candidate.unwrap().as_str(), candidate);

        // Claim admin rights by the candidate
        let claim_admin_msg = ContractExecMsg::Transmuter(ExecMsg::ClaimAdmin {});
        execute(
            deps.as_mut(),
            env.clone(),
            mock_info(candidate, &[]),
            claim_admin_msg,
        )
        .unwrap();

        // Check the current admin
        let res = query(
            deps.as_ref(),
            env,
            ContractQueryMsg::Transmuter(QueryMsg::GetAdmin {}),
        )
        .unwrap();
        let admin: GetAdminResponse = from_binary(&res).unwrap();
        assert_eq!(admin.admin.as_str(), candidate);
    }

    #[test]
    fn test_limiter_registration_and_config() {
        // register limiter
        let mut deps = mock_dependencies();

        let admin = "admin";
        let user = "user";
        let init_msg = InstantiateMsg {
            pool_asset_denoms: vec!["uosmo".to_string(), "uion".to_string()],
            admin: Some(admin.to_string()),
            alloyed_asset_subdenom: "usomoion".to_string(),
        };

        instantiate(deps.as_mut(), mock_env(), mock_info(admin, &[]), init_msg).unwrap();

        // normal user can't register limiter
        let err = execute(
            deps.as_mut(),
            mock_env(),
            mock_info(user, &[]),
            ContractExecMsg::Transmuter(ExecMsg::RegisterLimiter {
                denom: "uosmo".to_string(),
                label: "1h".to_string(),
                limiter_params: LimiterParams::ChangeLimiter {
                    window_config: WindowConfig {
                        window_size: Uint64::from(604_800_000_000u64),
                        division_count: Uint64::from(5u64),
                    },
                    boundary_offset: Decimal::zero(),
                },
            }),
        )
        .unwrap_err();

        assert_eq!(err, ContractError::Unauthorized {});

        // admin can register limiter
        let window_config_1h = WindowConfig {
            window_size: Uint64::from(3_600_000_000_000u64),
            division_count: Uint64::from(5u64),
        };
        let res = execute(
            deps.as_mut(),
            mock_env(),
            mock_info(admin, &[]),
            ContractExecMsg::Transmuter(ExecMsg::RegisterLimiter {
                denom: "uosmo".to_string(),
                label: "1h".to_string(),
                limiter_params: LimiterParams::ChangeLimiter {
                    window_config: window_config_1h.clone(),
                    boundary_offset: Decimal::zero(),
                },
            }),
        )
        .unwrap();

        let attrs = vec![
            attr("method", "register_limiter"),
            attr("denom", "uosmo"),
            attr("label", "1h"),
            attr("limiter_type", "change_limiter"),
            attr("window_size", "3600000000000"),
            attr("division_count", "5"),
            attr("boundary_offset", "0"),
        ];

        assert_eq!(res.attributes, attrs);

        // Query the list of limiters
        let query_msg = ContractQueryMsg::Transmuter(QueryMsg::ListLimiters {});
        let res = query(deps.as_ref(), mock_env(), query_msg).unwrap();
        let limiters: ListLimitersResponse = from_binary(&res).unwrap();

        assert_eq!(
            limiters.limiters,
            vec![(
                (String::from("uosmo"), String::from("1h")),
                Limiter::ChangeLimiter(
                    ChangeLimiter::new(window_config_1h.clone(), Decimal::zero()).unwrap()
                )
            )]
        );

        let window_config_1w = WindowConfig {
            window_size: Uint64::from(604_800_000_000u64),
            division_count: Uint64::from(5u64),
        };
        let res = execute(
            deps.as_mut(),
            mock_env(),
            mock_info(admin, &[]),
            ContractExecMsg::Transmuter(ExecMsg::RegisterLimiter {
                denom: "osmo".to_string(),
                label: "1w".to_string(),
                limiter_params: LimiterParams::ChangeLimiter {
                    window_config: window_config_1w.clone(),
                    boundary_offset: Decimal::zero(),
                },
            }),
        )
        .unwrap();

        let attrs_1w = vec![
            attr("method", "register_limiter"),
            attr("denom", "osmo"),
            attr("label", "1w"),
            attr("limiter_type", "change_limiter"),
            attr("window_size", "604800000000"),
            attr("division_count", "5"),
            attr("boundary_offset", "0"),
        ];

        assert_eq!(res.attributes, attrs_1w);

        // Query the list of limiters
        let query_msg = ContractQueryMsg::Transmuter(QueryMsg::ListLimiters {});
        let res = query(deps.as_ref(), mock_env(), query_msg).unwrap();
        let limiters: ListLimitersResponse = from_binary(&res).unwrap();

        assert_eq!(
            limiters.limiters,
            vec![
                (
                    (String::from("osmo"), String::from("1w")),
                    Limiter::ChangeLimiter(
                        ChangeLimiter::new(window_config_1w.clone(), Decimal::zero()).unwrap()
                    )
                ),
                (
                    (String::from("uosmo"), String::from("1h")),
                    Limiter::ChangeLimiter(
                        ChangeLimiter::new(window_config_1h, Decimal::zero()).unwrap()
                    )
                )
            ]
        );

        // deregister limiter by user is unauthorized
        let err = execute(
            deps.as_mut(),
            mock_env(),
            mock_info(user, &[]),
            ContractExecMsg::Transmuter(ExecMsg::DeregisterLimiter {
                denom: "uosmo".to_string(),
                label: "1h".to_string(),
            }),
        )
        .unwrap_err();

        assert_eq!(err, ContractError::Unauthorized {});

        // deregister limiter by admin should work
        let res = execute(
            deps.as_mut(),
            mock_env(),
            mock_info(admin, &[]),
            ContractExecMsg::Transmuter(ExecMsg::DeregisterLimiter {
                denom: "uosmo".to_string(),
                label: "1h".to_string(),
            }),
        )
        .unwrap();

        let attrs = vec![
            attr("method", "deregister_limiter"),
            attr("denom", "uosmo"),
            attr("label", "1h"),
        ];

        assert_eq!(res.attributes, attrs);

        // Query the list of limiters
        let query_msg = ContractQueryMsg::Transmuter(QueryMsg::ListLimiters {});
        let res = query(deps.as_ref(), mock_env(), query_msg).unwrap();
        let limiters: ListLimitersResponse = from_binary(&res).unwrap();

        assert_eq!(
            limiters.limiters,
            vec![(
                (String::from("osmo"), String::from("1w")),
                Limiter::ChangeLimiter(
                    ChangeLimiter::new(window_config_1w.clone(), Decimal::zero()).unwrap()
                )
            )]
        );

        // set boundary offset by user is unauthorized
        let err = execute(
            deps.as_mut(),
            mock_env(),
            mock_info(user, &[]),
            ContractExecMsg::Transmuter(ExecMsg::SetBoundaryOffset {
                denom: "osmo".to_string(),
                label: "1w".to_string(),
                boundary_offset: Decimal::zero(),
            }),
        )
        .unwrap_err();

        assert_eq!(err, ContractError::Unauthorized {});

        // set boundary offset by admin but for osmo 1h should fail
        let err = execute(
            deps.as_mut(),
            mock_env(),
            mock_info(admin, &[]),
            ContractExecMsg::Transmuter(ExecMsg::SetBoundaryOffset {
                denom: "osmo".to_string(),
                label: "1h".to_string(),
                boundary_offset: Decimal::zero(),
            }),
        )
        .unwrap_err();

        assert_eq!(
            err,
            ContractError::LimiterDoesNotExist {
                denom: "osmo".to_string(),
                label: "1h".to_string()
            }
        );

        // set boundary offset by admin for existing limiter should work
        let res = execute(
            deps.as_mut(),
            mock_env(),
            mock_info(admin, &[]),
            ContractExecMsg::Transmuter(ExecMsg::SetBoundaryOffset {
                denom: "osmo".to_string(),
                label: "1w".to_string(),
                boundary_offset: Decimal::percent(10),
            }),
        )
        .unwrap();

        let attrs = vec![
            attr("method", "set_boundary_offset"),
            attr("denom", "osmo"),
            attr("label", "1w"),
            attr("boundary_offset", "0.1"),
        ];

        assert_eq!(res.attributes, attrs);

        // Query the list of limiters
        let query_msg = ContractQueryMsg::Transmuter(QueryMsg::ListLimiters {});
        let res = query(deps.as_ref(), mock_env(), query_msg).unwrap();
        let limiters: ListLimitersResponse = from_binary(&res).unwrap();

        assert_eq!(
            limiters.limiters,
            vec![(
                (String::from("osmo"), String::from("1w")),
                Limiter::ChangeLimiter(
                    ChangeLimiter::new(window_config_1w, Decimal::percent(10)).unwrap()
                )
            )]
        );
    }
}
