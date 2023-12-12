use crate::{
    alloyed_asset::AlloyedAsset,
    asset::{Asset, AssetConfig},
    ensure_admin_authority, ensure_moderator_authority,
    error::ContractError,
    limiter::{Limiter, LimiterParams, Limiters},
    role::Role,
    swap::Entrypoint,
    transmuter_pool::{AmountConstraint, TransmuterPool},
};
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    ensure, ensure_eq, Addr, Coin, Decimal, Deps, DepsMut, Env, MessageInfo, Reply, Response,
    StdError, SubMsg, Uint128,
};

use cw_storage_plus::Item;
use osmosis_std::types::{
    cosmos::bank::v1beta1::Metadata,
    osmosis::tokenfactory::v1beta1::{MsgCreateDenom, MsgCreateDenomResponse, MsgSetDenomMetadata},
};

use sylvia::contract;

/// version info for migration
pub const CONTRACT_NAME: &str = "crates.io:transmuter";
pub const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Swap fee is hardcoded to zero intentionally.
const SWAP_FEE: Decimal = Decimal::zero();

const CREATE_ALLOYED_DENOM_REPLY_ID: u64 = 1;

/// Prefix for alloyed asset denom
const ALLOYED_PREFIX: &str = "alloyed";

pub struct Transmuter<'a> {
    pub(crate) active_status: Item<'a, bool>,
    pub(crate) pool: Item<'a, TransmuterPool>,
    pub(crate) alloyed_asset: AlloyedAsset<'a>,
    pub(crate) role: Role<'a>,
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
            alloyed_asset: AlloyedAsset::new("alloyed_denom"),
            role: Role::new("admin", "moderator"),
            limiters: Limiters::new("limiters"),
        }
    }

    /// Instantiate the contract.
    #[msg(instantiate)]
    pub fn instantiate(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        pool_asset_configs: Vec<AssetConfig>,
        alloyed_asset_subdenom: String,
        admin: Option<String>,
        moderator: Option<String>,
    ) -> Result<Response, ContractError> {
        let (deps, env, _info) = ctx;

        // store contract version for migration info
        cw2::set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

        // set admin if exists
        if let Some(admin) = admin {
            self.role
                .admin
                .init(deps.storage, deps.api.addr_validate(&admin)?)?;
        }

        // set moderator if exists
        if let Some(moderator) = moderator {
            self.role
                .moderator
                .init(deps.storage, deps.api.addr_validate(&moderator)?)?;
        }

        let pool_assets = pool_asset_configs
            .into_iter()
            .map(|config| AssetConfig::checked_init_asset(config, deps.as_ref()))
            .collect::<Result<Vec<_>, ContractError>>()?;

        // store pool
        self.pool
            .save(deps.storage, &TransmuterPool::new(pool_assets)?)?;

        // set active status to true
        self.active_status.save(deps.storage, &true)?;

        // create alloyed denom
        let msg_create_alloyed_denom = SubMsg::reply_on_success(
            MsgCreateDenom {
                sender: env.contract.address.to_string(),
                subdenom: format!("{}/{}", ALLOYED_PREFIX, alloyed_asset_subdenom),
            },
            CREATE_ALLOYED_DENOM_REPLY_ID,
        );

        Ok(Response::new()
            .add_attribute("method", "instantiate")
            .add_attribute("contract_name", CONTRACT_NAME)
            .add_attribute("contract_version", CONTRACT_VERSION)
            .add_submessage(msg_create_alloyed_denom))
    }

    pub fn reply(&self, ctx: (DepsMut, Env), msg: Reply) -> Result<Response, ContractError> {
        let (deps, _env) = ctx;

        match msg.id {
            CREATE_ALLOYED_DENOM_REPLY_ID => {
                // register created token denom
                let MsgCreateDenomResponse { new_token_denom } = msg.result.try_into()?;
                self.alloyed_asset
                    .set_alloyed_denom(deps.storage, &new_token_denom)?;

                Ok(Response::new().add_attribute("alloyed_denom", new_token_denom))
            }
            _ => Err(StdError::not_found(format!("No reply handler found for: {:?}", msg)).into()),
        }
    }

    // === executes ===

    #[msg(exec)]
    fn add_new_assets(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        asset_configs: Vec<AssetConfig>,
    ) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;

        // only admin can add new assets
        ensure_admin_authority!(info.sender, self.role.admin, deps.as_ref());

        // ensure that new denoms are not alloyed denom
        let share_denom = self.alloyed_asset.get_alloyed_denom(deps.storage)?;
        for cfg in &asset_configs {
            ensure!(
                cfg.denom != share_denom,
                ContractError::ShareDenomNotAllowedAsPoolAsset {}
            );
        }

        // convert denoms to Denom type
        let assets = asset_configs
            .into_iter()
            .map(|cfg| cfg.checked_init_asset(deps.as_ref()))
            .collect::<Result<Vec<_>, ContractError>>()?;

        // add new assets to the pool
        let mut pool = self.pool.load(deps.storage)?;
        pool.add_new_assets(assets)?;
        self.pool.save(deps.storage, &pool)?;

        Ok(Response::new().add_attribute("method", "add_new_assets"))
    }

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
        ensure_admin_authority!(info.sender, self.role.admin, deps.as_ref());

        // ensure pool has the specified denom
        let pool = self.pool.load(deps.storage)?;
        ensure!(
            pool.has_denom(&denom),
            ContractError::InvalidPoolAssetDenom { denom }
        );

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
        ensure_admin_authority!(info.sender, self.role.admin, deps.as_ref());

        let attrs = vec![
            ("method", "deregister_limiter"),
            ("denom", &denom),
            ("label", &label),
        ];

        // deregister limiter
        self.limiters.deregister(deps.storage, &denom, &label)?;

        Ok(Response::new().add_attributes(attrs))
    }

    #[msg(exec)]
    fn set_change_limiter_boundary_offset(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        denom: String,
        label: String,
        boundary_offset: Decimal,
    ) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;

        // only admin can set boundary offset
        ensure_admin_authority!(info.sender, self.role.admin, deps.as_ref());

        let boundary_offset_string = boundary_offset.to_string();
        let attrs = vec![
            ("method", "set_change_limiter_boundary_offset"),
            ("denom", &denom),
            ("label", &label),
            ("boundary_offset", boundary_offset_string.as_str()),
        ];

        // set boundary offset
        self.limiters.set_change_limiter_boundary_offset(
            deps.storage,
            &denom,
            &label,
            boundary_offset,
        )?;

        Ok(Response::new().add_attributes(attrs))
    }

    #[msg(exec)]
    fn set_static_limiter_upper_limit(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        denom: String,
        label: String,
        upper_limit: Decimal,
    ) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;

        // only admin can set upper limit
        ensure_admin_authority!(info.sender, self.role.admin, deps.as_ref());

        let upper_limit_string = upper_limit.to_string();
        let attrs = vec![
            ("method", "set_static_limiter_upper_limit"),
            ("denom", &denom),
            ("label", &label),
            ("upper_limit", upper_limit_string.as_str()),
        ];

        // set upper limit
        self.limiters
            .set_static_limiter_upper_limit(deps.storage, &denom, &label, upper_limit)?;

        Ok(Response::new().add_attributes(attrs))
    }

    #[msg(exec)]
    pub fn set_alloyed_denom_metadata(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        metadata: Metadata,
    ) -> Result<Response, ContractError> {
        let (deps, env, info) = ctx;

        // only admin can set denom metadata
        ensure_admin_authority!(info.sender, self.role.admin, deps.as_ref());

        let msg_set_denom_metadata = MsgSetDenomMetadata {
            sender: env.contract.address.to_string(),
            metadata: Some(metadata),
        };

        Ok(Response::new()
            .add_attribute("method", "set_alloyed_denom_metadata")
            .add_message(msg_set_denom_metadata))
    }

    #[msg(exec)]
    fn set_active_status(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        active: bool,
    ) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;

        // only moderator can set active status
        ensure_moderator_authority!(info.sender, self.role.moderator, deps.as_ref());

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
        let (deps, env, info) = ctx;
        self.swap_tokens_to_alloyed_asset(
            Entrypoint::Exec,
            SwapToAlloyedConstraint::ExactIn {
                tokens_in: &info.funds,
                token_out_min_amount: Uint128::zero(),
            },
            info.sender,
            deps,
            env,
        )
        .map(|res| res.add_attribute("method", "join_pool"))
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
        self.swap_alloyed_asset_to_tokens(
            Entrypoint::Exec,
            SwapFromAlloyedConstraint::ExactOut {
                tokens_out: &tokens_out,
                token_in_max_amount: Uint128::MAX,
            },
            BurnTarget::SenderAccount,
            info.sender,
            deps,
            env,
        )
        .map(|res| res.add_attribute("method", "exit_pool"))
    }

    // === queries ===

    #[msg(query)]
    fn list_asset_configs(
        &self,
        ctx: (Deps, Env),
    ) -> Result<ListAssetConfigsResponse, ContractError> {
        let (deps, _env) = ctx;

        let pool = self.pool.load(deps.storage)?;

        Ok(ListAssetConfigsResponse {
            asset_configs: pool
                .pool_assets
                .iter()
                .map(|asset| asset.config())
                .collect(),
        })
    }

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
                .alloyed_asset
                .get_balance(deps, &deps.api.addr_validate(&address)?)?,
        })
    }

    #[msg(query)]
    pub(crate) fn get_share_denom(
        &self,
        ctx: (Deps, Env),
    ) -> Result<GetShareDenomResponse, ContractError> {
        let (deps, _env) = ctx;
        Ok(GetShareDenomResponse {
            share_denom: self.alloyed_asset.get_alloyed_denom(deps.storage)?,
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
        let total_shares = self.alloyed_asset.get_total_supply(deps)?;
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
            total_pool_liquidity: pool.pool_assets.iter().map(Asset::to_coin).collect(),
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

        // ensure that qoute asset denom are in swappable assets
        let pool = self.pool.load(deps.storage)?;
        let alloyed_denom = self.alloyed_asset.get_alloyed_denom(deps.storage)?;
        let swappable_assets = pool
            .pool_assets
            .iter()
            .map(|c| c.denom().to_string())
            .chain(vec![alloyed_denom])
            .collect::<Vec<_>>();

        ensure!(
            swappable_assets
                .iter()
                .any(|denom| denom == quote_asset_denom.as_str()),
            ContractError::SpotPriceQueryFailed {
                reason: format!(
                    "quote_asset_denom is not in swappable assets: must be one of {:?} but got {}",
                    swappable_assets, quote_asset_denom
                )
            }
        );

        // ensure that base asset denom are in swappable assets
        ensure!(
            swappable_assets
                .iter()
                .any(|denom| denom == base_asset_denom.as_str()),
            ContractError::SpotPriceQueryFailed {
                reason: format!(
                    "base_asset_denom is not in swappable assets: must be one of {:?} but got {}",
                    swappable_assets, base_asset_denom
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
            self.do_calc_out_amt_given_in(ctx, token_in, &token_out_denom, swap_fee)?;

        Ok(CalcOutAmtGivenInResponse { token_out })
    }

    pub(crate) fn do_calc_out_amt_given_in(
        &self,
        ctx: (Deps, Env),
        token_in: Coin,
        token_out_denom: &str,
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

        // ensure token in denom and token out denom are not the same
        ensure!(
            token_out_denom != token_in.denom,
            StdError::generic_err("token_in_denom and token_out_denom cannot be the same")
        );

        let alloyed_denom = self.alloyed_asset.get_alloyed_denom(ctx.0.storage)?;

        let token_out = Coin::new(token_in.amount.u128(), token_out_denom);
        let mut pool = self.pool.load(ctx.0.storage)?;

        // In case where token_in_denom or token_out_denom is alloyed_denom:

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

    #[msg(query)]
    pub(crate) fn calc_in_amt_given_out(
        &self,
        ctx: (Deps, Env),
        token_out: Coin,
        token_in_denom: String,
        swap_fee: Decimal,
    ) -> Result<CalcInAmtGivenOutResponse, ContractError> {
        // else we call do_calc_in_amt_given_out to ensure constraint on the pool
        let (_pool, token_in) =
            self.do_calc_in_amt_given_out(ctx, token_out, token_in_denom, swap_fee)?;

        Ok(CalcInAmtGivenOutResponse { token_in })
    }

    pub(crate) fn do_calc_in_amt_given_out(
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

        // ensure token in denom and token out denom are not the same
        ensure!(
            token_out.denom != token_in_denom,
            StdError::generic_err("token_in_denom and token_out_denom cannot be the same")
        );

        let alloyed_denom = self.alloyed_asset.get_alloyed_denom(ctx.0.storage)?;

        let token_in = Coin::new(token_out.amount.u128(), token_in_denom);
        let mut pool = self.pool.load(deps.storage)?;

        // In case where token_in_denom or token_out_denom is alloyed_denom:

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

    // --- admin ---

    #[msg(exec)]
    pub fn transfer_admin(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        candidate: String,
    ) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;

        let candidate_addr = deps.api.addr_validate(&candidate)?;
        self.role
            .admin
            .transfer(deps, info.sender, candidate_addr)?;

        Ok(Response::new()
            .add_attribute("method", "transfer_admin")
            .add_attribute("candidate", candidate))
    }

    #[msg(exec)]
    pub fn cancel_admin_transfer(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
    ) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;
        self.role.admin.cancel_transfer(deps, info.sender)?;

        Ok(Response::new().add_attribute("method", "cancel_admin_transfer"))
    }

    #[msg(exec)]
    pub fn reject_admin_transfer(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
    ) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;
        self.role.admin.reject_transfer(deps, info.sender)?;

        Ok(Response::new().add_attribute("method", "reject_admin_transfer"))
    }

    #[msg(exec)]
    pub fn claim_admin(&self, ctx: (DepsMut, Env, MessageInfo)) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;
        let sender_string = info.sender.to_string();
        self.role.admin.claim(deps, info.sender)?;

        Ok(Response::new()
            .add_attribute("method", "claim_admin")
            .add_attribute("new_admin", sender_string))
    }

    #[msg(exec)]
    pub fn renounce_adminship(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
    ) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;
        self.role.admin.renounce(deps, info.sender)?;

        Ok(Response::new().add_attribute("method", "renounce_adminship"))
    }

    #[msg(query)]
    fn get_admin(&self, ctx: (Deps, Env)) -> Result<GetAdminResponse, ContractError> {
        let (deps, _env) = ctx;

        Ok(GetAdminResponse {
            admin: self.role.admin.current(deps)?,
        })
    }

    #[msg(query)]
    fn get_admin_candidate(
        &self,
        ctx: (Deps, Env),
    ) -> Result<GetAdminCandidateResponse, ContractError> {
        let (deps, _env) = ctx;

        Ok(GetAdminCandidateResponse {
            admin_candidate: self.role.admin.candidate(deps)?,
        })
    }

    // -- moderator --
    #[msg(exec)]
    pub fn assign_moderator(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        address: String,
    ) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;
        let moderator_address = deps.api.addr_validate(&address)?;

        self.role
            .assign_moderator(info.sender, deps, moderator_address)?;

        Ok(Response::new()
            .add_attribute("method", "assign_moderator")
            .add_attribute("moderator", address))
    }

    #[msg(exec)]
    pub fn remove_moderator(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
    ) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;

        self.role.remove_moderator(info.sender, deps)?;

        Ok(Response::new().add_attribute("method", "remove_moderator"))
    }

    #[msg(query)]
    fn get_moderator(&self, ctx: (Deps, Env)) -> Result<GetModeratorResponse, ContractError> {
        let (deps, _env) = ctx;

        Ok(GetModeratorResponse {
            moderator: self.role.moderator.get(deps)?,
        })
    }
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

#[cw_serde]
pub struct ListAssetConfigsResponse {
    pub asset_configs: Vec<AssetConfig>,
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

#[cw_serde]
pub struct GetModeratorResponse {
    pub moderator: Addr,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limiter::{ChangeLimiter, StaticLimiter, WindowConfig};
    use crate::sudo::SudoMsg;
    use crate::*;

    use cosmwasm_std::testing::{mock_dependencies, mock_env, mock_info};
    use cosmwasm_std::{attr, from_binary, BankMsg, SubMsgResponse, SubMsgResult, Uint64};
    use osmosis_std::types::osmosis::tokenfactory::v1beta1::MsgBurn;

    #[test]
    fn test_add_new_assets() {
        let mut deps = mock_dependencies();

        // make denom has non-zero total supply
        deps.querier.update_balance(
            "someone",
            vec![
                Coin::new(1, "uosmo"),
                Coin::new(1, "uion"),
                Coin::new(1, "new_asset1"),
                Coin::new(1, "new_asset2"),
            ],
        );

        let admin = "admin";
        let init_msg = InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("uosmo"),
                AssetConfig::from_denom_str("uion"),
            ],
            alloyed_asset_subdenom: "uosmouion".to_string(),
            admin: Some(admin.to_string()),
            moderator: None,
        };
        let env = mock_env();
        let info = mock_info(admin, &[]);

        // Instantiate the contract.
        instantiate(deps.as_mut(), env.clone(), info.clone(), init_msg).unwrap();

        // Manually reply
        let alloyed_denom = "usomoion";

        reply(
            deps.as_mut(),
            env.clone(),
            Reply {
                id: 1,
                result: SubMsgResult::Ok(SubMsgResponse {
                    events: vec![],
                    data: Some(
                        MsgCreateDenomResponse {
                            new_token_denom: alloyed_denom.to_string(),
                        }
                        .into(),
                    ),
                }),
            },
        )
        .unwrap();

        // Add new assets

        // Attempt to add assets with invalid denom
        let invalid_denoms = vec!["invalid_asset1".to_string(), "invalid_asset2".to_string()];
        let add_invalid_assets_msg = ContractExecMsg::Transmuter(ExecMsg::AddNewAssets {
            asset_configs: invalid_denoms
                .into_iter()
                .map(|denom| AssetConfig::from_denom_str(denom.as_str()))
                .collect(),
        });

        let res = execute(
            deps.as_mut(),
            env.clone(),
            info.clone(),
            add_invalid_assets_msg,
        );

        // Check if the attempt resulted in DenomHasNoSupply error
        assert_eq!(
            res.unwrap_err(),
            ContractError::DenomHasNoSupply {
                denom: "invalid_asset1".to_string()
            }
        );

        let new_assets = vec!["new_asset1".to_string(), "new_asset2".to_string()];
        let add_assets_msg = ContractExecMsg::Transmuter(ExecMsg::AddNewAssets {
            asset_configs: new_assets
                .into_iter()
                .map(|denom| AssetConfig::from_denom_str(denom.as_str()))
                .collect(),
        });

        // Attempt to add assets by non-admin
        let non_admin_info = mock_info("non_admin", &[]);
        let res = execute(
            deps.as_mut(),
            env.clone(),
            non_admin_info,
            add_assets_msg.clone(),
        );

        // Check if the attempt was unauthorized
        assert_eq!(
            res.unwrap_err(),
            ContractError::Unauthorized {},
            "Adding assets by non-admin should be unauthorized"
        );

        execute(deps.as_mut(), env.clone(), info, add_assets_msg).unwrap();

        // Check if the new assets were added
        let res = query(
            deps.as_ref(),
            env,
            ContractQueryMsg::Transmuter(QueryMsg::GetTotalPoolLiquidity {}),
        )
        .unwrap();
        let GetTotalPoolLiquidityResponse {
            total_pool_liquidity,
        } = from_binary(&res).unwrap();

        assert_eq!(
            total_pool_liquidity,
            vec![
                Coin::new(0, "uosmo"),
                Coin::new(0, "uion"),
                Coin::new(0, "new_asset1"),
                Coin::new(0, "new_asset2"),
            ]
        );
    }

    #[test]
    fn test_set_active_status() {
        let mut deps = mock_dependencies();

        // make denom has non-zero total supply
        deps.querier
            .update_balance("someone", vec![Coin::new(1, "uosmo"), Coin::new(1, "uion")]);

        let admin = "admin";
        let moderator = "moderator";
        let init_msg = InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("uosmo"),
                AssetConfig::from_denom_str("uion"),
            ],
            alloyed_asset_subdenom: "uosmouion".to_string(),
            admin: Some(admin.to_string()),
            moderator: Some(moderator.to_string()),
        };
        let env = mock_env();
        let info = mock_info(admin, &[]);

        // Instantiate the contract.
        instantiate(deps.as_mut(), env.clone(), info, init_msg).unwrap();

        // Manually set alloyed denom
        let alloyed_denom = "uosmo".to_string();

        let transmuter = Transmuter::new();
        transmuter
            .alloyed_asset
            .set_alloyed_denom(&mut deps.storage, &alloyed_denom)
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
        let non_admin_info = mock_info("non_moderator", &[]);
        let non_admin_msg = ContractExecMsg::Transmuter(ExecMsg::SetActiveStatus { active: false });
        let err = execute(deps.as_mut(), env.clone(), non_admin_info, non_admin_msg).unwrap_err();

        assert_eq!(err, ContractError::Unauthorized {});

        // Set the active status to false.
        let msg = ContractExecMsg::Transmuter(ExecMsg::SetActiveStatus { active: false });
        execute(deps.as_mut(), env.clone(), mock_info(moderator, &[]), msg).unwrap();

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
        execute(deps.as_mut(), env.clone(), mock_info(moderator, &[]), msg).unwrap();

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
        let res = sudo(deps.as_mut(), env.clone(), swap_exact_amount_out_msg);

        assert!(res.is_ok());

        // Test setting active status through sudo
        let set_active_status_msg = SudoMsg::SetActive { is_active: false };
        let res = sudo(deps.as_mut(), env.clone(), set_active_status_msg);
        assert!(res.is_ok());

        // Check the active status again.
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::IsActive {}),
        )
        .unwrap();
        let active_status: IsActiveResponse = from_binary(&res).unwrap();
        assert!(!active_status.is_active);

        // Set the active status back to true through sudo
        let set_active_status_msg = SudoMsg::SetActive { is_active: true };
        let res = sudo(deps.as_mut(), env.clone(), set_active_status_msg);
        assert!(res.is_ok());

        // Check the active status again.
        let res = query(
            deps.as_ref(),
            env,
            ContractQueryMsg::Transmuter(QueryMsg::IsActive {}),
        )
        .unwrap();
        let active_status: IsActiveResponse = from_binary(&res).unwrap();
        assert!(active_status.is_active);
    }

    #[test]
    fn test_transfer_and_claim_admin() {
        let mut deps = mock_dependencies();

        // make denom has non-zero total supply
        deps.querier
            .update_balance("someone", vec![Coin::new(1, "uosmo"), Coin::new(1, "uion")]);

        let admin = "admin";
        let canceling_candidate = "canceling_candidate";
        let rejecting_candidate = "rejecting_candidate";
        let candidate = "candidate";
        let init_msg = InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("uosmo"),
                AssetConfig::from_denom_str("uion"),
            ],
            admin: Some(admin.to_string()),
            alloyed_asset_subdenom: "usomoion".to_string(),
            moderator: None,
        };
        let env = mock_env();
        let info = mock_info(admin, &[]);

        // Instantiate the contract.
        instantiate(deps.as_mut(), env.clone(), info.clone(), init_msg).unwrap();

        // Transfer admin rights to the canceling candidate
        let transfer_admin_msg = ContractExecMsg::Transmuter(ExecMsg::TransferAdmin {
            candidate: canceling_candidate.to_string(),
        });
        execute(deps.as_mut(), env.clone(), info.clone(), transfer_admin_msg).unwrap();

        // Check the admin candidate
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::GetAdminCandidate {}),
        )
        .unwrap();
        let admin_candidate: GetAdminCandidateResponse = from_binary(&res).unwrap();
        assert_eq!(
            admin_candidate.admin_candidate.unwrap().as_str(),
            canceling_candidate
        );

        // Cancel admin rights transfer
        let cancel_admin_transfer_msg =
            ContractExecMsg::Transmuter(ExecMsg::CancelAdminTransfer {});

        execute(
            deps.as_mut(),
            env.clone(),
            mock_info(admin, &[]),
            cancel_admin_transfer_msg,
        )
        .unwrap();

        // Check the admin candidate
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::GetAdminCandidate {}),
        )
        .unwrap();
        let admin_candidate: GetAdminCandidateResponse = from_binary(&res).unwrap();
        assert_eq!(admin_candidate.admin_candidate, None);

        // Transfer admin rights to the rejecting candidate
        let transfer_admin_msg = ContractExecMsg::Transmuter(ExecMsg::TransferAdmin {
            candidate: rejecting_candidate.to_string(),
        });
        execute(deps.as_mut(), env.clone(), info.clone(), transfer_admin_msg).unwrap();

        // Check the admin candidate
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::GetAdminCandidate {}),
        )
        .unwrap();
        let admin_candidate: GetAdminCandidateResponse = from_binary(&res).unwrap();
        assert_eq!(
            admin_candidate.admin_candidate.unwrap().as_str(),
            rejecting_candidate
        );

        // Reject admin rights transfer
        let reject_admin_transfer_msg =
            ContractExecMsg::Transmuter(ExecMsg::RejectAdminTransfer {});

        execute(
            deps.as_mut(),
            env.clone(),
            mock_info(rejecting_candidate, &[]),
            reject_admin_transfer_msg,
        )
        .unwrap();

        // Check the admin candidate
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::GetAdminCandidate {}),
        )
        .unwrap();
        let admin_candidate: GetAdminCandidateResponse = from_binary(&res).unwrap();
        assert_eq!(admin_candidate.admin_candidate, None);

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
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::GetAdmin {}),
        )
        .unwrap();
        let admin: GetAdminResponse = from_binary(&res).unwrap();
        assert_eq!(admin.admin.as_str(), candidate);

        // Renounce admin rights
        let renounce_admin_msg = ContractExecMsg::Transmuter(ExecMsg::RenounceAdminship {});
        execute(
            deps.as_mut(),
            env.clone(),
            mock_info(candidate, &[]),
            renounce_admin_msg,
        )
        .unwrap();

        // Check the current admin
        let err = query(
            deps.as_ref(),
            env,
            ContractQueryMsg::Transmuter(QueryMsg::GetAdmin {}),
        )
        .unwrap_err();

        assert_eq!(err, ContractError::Std(StdError::not_found("admin")));
    }

    #[test]
    fn test_assign_and_remove_moderator() {
        let admin = "admin";
        let moderator = "moderator";

        let mut deps = mock_dependencies();

        // make denom has non-zero total supply
        deps.querier
            .update_balance("someone", vec![Coin::new(1, "uosmo"), Coin::new(1, "uion")]);

        // Instantiate the contract.
        let init_msg = InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("uosmo"),
                AssetConfig::from_denom_str("uion"),
            ],
            admin: Some(admin.to_string()),
            alloyed_asset_subdenom: "usomoion".to_string(),
            moderator: None,
        };
        instantiate(deps.as_mut(), mock_env(), mock_info(admin, &[]), init_msg).unwrap();

        // Check the current moderator
        let err = query(
            deps.as_ref(),
            mock_env(),
            ContractQueryMsg::Transmuter(QueryMsg::GetModerator {}),
        )
        .unwrap_err();

        assert_eq!(err, ContractError::Std(StdError::not_found("moderator")));

        let mut deps = mock_dependencies();

        // make denom has non-zero total supply
        deps.querier
            .update_balance("someone", vec![Coin::new(1, "uosmo"), Coin::new(1, "uion")]);

        // Instantiate the contract.
        let init_msg = InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("uosmo"),
                AssetConfig::from_denom_str("uion"),
            ],
            admin: Some(admin.to_string()),
            alloyed_asset_subdenom: "usomoion".to_string(),
            moderator: Some(moderator.to_string()),
        };
        instantiate(deps.as_mut(), mock_env(), mock_info(admin, &[]), init_msg).unwrap();

        // Check the current moderator
        let res = query(
            deps.as_ref(),
            mock_env(),
            ContractQueryMsg::Transmuter(QueryMsg::GetModerator {}),
        )
        .unwrap();
        let moderator_response: GetModeratorResponse = from_binary(&res).unwrap();
        assert_eq!(moderator_response.moderator, moderator);

        let new_moderator = "new_moderator";

        // Try to assign new moderator by non admin
        let err = execute(
            deps.as_mut(),
            mock_env(),
            mock_info("non_admin", &[]),
            ContractExecMsg::Transmuter(ExecMsg::AssignModerator {
                address: new_moderator.to_string(),
            }),
        )
        .unwrap_err();

        assert_eq!(err, ContractError::Unauthorized {});

        // Assign new moderator by admin
        execute(
            deps.as_mut(),
            mock_env(),
            mock_info(admin, &[]),
            ContractExecMsg::Transmuter(ExecMsg::AssignModerator {
                address: new_moderator.to_string(),
            }),
        )
        .unwrap();

        // Check the current moderator
        let res = query(
            deps.as_ref(),
            mock_env(),
            ContractQueryMsg::Transmuter(QueryMsg::GetModerator {}),
        )
        .unwrap();
        let moderator_response: GetModeratorResponse = from_binary(&res).unwrap();
        assert_eq!(moderator_response.moderator, new_moderator);

        // Try to remove moderator by non admin
        let err = execute(
            deps.as_mut(),
            mock_env(),
            mock_info("non_admin", &[]),
            ContractExecMsg::Transmuter(ExecMsg::RemoveModerator {}),
        )
        .unwrap_err();

        assert_eq!(err, ContractError::Unauthorized {});

        // Remove moderator by admin
        execute(
            deps.as_mut(),
            mock_env(),
            mock_info(admin, &[]),
            ContractExecMsg::Transmuter(ExecMsg::RemoveModerator {}),
        )
        .unwrap();

        // Check the current moderator
        let err = query(
            deps.as_ref(),
            mock_env(),
            ContractQueryMsg::Transmuter(QueryMsg::GetModerator {}),
        )
        .unwrap_err();

        assert_eq!(err, ContractError::Std(StdError::not_found("moderator")));
    }

    #[test]
    fn test_limiter_registration_and_config() {
        // register limiter
        let mut deps = mock_dependencies();

        // make denom has non-zero total supply
        deps.querier
            .update_balance("someone", vec![Coin::new(1, "uosmo"), Coin::new(1, "uion")]);

        let admin = "admin";
        let user = "user";
        let init_msg = InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("uosmo"),
                AssetConfig::from_denom_str("uion"),
            ],
            admin: Some(admin.to_string()),
            moderator: None,
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
                    boundary_offset: Decimal::one(),
                },
            }),
        )
        .unwrap_err();

        assert_eq!(err, ContractError::Unauthorized {});

        let err = execute(
            deps.as_mut(),
            mock_env(),
            mock_info(user, &[]),
            ContractExecMsg::Transmuter(ExecMsg::RegisterLimiter {
                denom: "uosmo".to_string(),
                label: "1h".to_string(),
                limiter_params: LimiterParams::StaticLimiter {
                    upper_limit: Decimal::percent(60),
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
                    boundary_offset: Decimal::percent(1),
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
            attr("boundary_offset", "0.01"),
        ];

        assert_eq!(res.attributes, attrs);

        // denom that is not in the pool can't be registered
        let err = execute(
            deps.as_mut(),
            mock_env(),
            mock_info(admin, &[]),
            ContractExecMsg::Transmuter(ExecMsg::RegisterLimiter {
                denom: "invalid_denom".to_string(),
                label: "1h".to_string(),
                limiter_params: LimiterParams::ChangeLimiter {
                    window_config: window_config_1h.clone(),
                    boundary_offset: Decimal::percent(1),
                },
            }),
        )
        .unwrap_err();

        assert_eq!(
            err,
            ContractError::InvalidPoolAssetDenom {
                denom: "invalid_denom".to_string(),
            }
        );

        // Query the list of limiters
        let query_msg = ContractQueryMsg::Transmuter(QueryMsg::ListLimiters {});
        let res = query(deps.as_ref(), mock_env(), query_msg).unwrap();
        let limiters: ListLimitersResponse = from_binary(&res).unwrap();

        assert_eq!(
            limiters.limiters,
            vec![(
                (String::from("uosmo"), String::from("1h")),
                Limiter::ChangeLimiter(
                    ChangeLimiter::new(window_config_1h.clone(), Decimal::percent(1)).unwrap()
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
                denom: "uosmo".to_string(),
                label: "1w".to_string(),
                limiter_params: LimiterParams::ChangeLimiter {
                    window_config: window_config_1w.clone(),
                    boundary_offset: Decimal::percent(1),
                },
            }),
        )
        .unwrap();

        let attrs_1w = vec![
            attr("method", "register_limiter"),
            attr("denom", "uosmo"),
            attr("label", "1w"),
            attr("limiter_type", "change_limiter"),
            attr("window_size", "604800000000"),
            attr("division_count", "5"),
            attr("boundary_offset", "0.01"),
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
                    (String::from("uosmo"), String::from("1h")),
                    Limiter::ChangeLimiter(
                        ChangeLimiter::new(window_config_1h, Decimal::percent(1)).unwrap()
                    )
                ),
                (
                    (String::from("uosmo"), String::from("1w")),
                    Limiter::ChangeLimiter(
                        ChangeLimiter::new(window_config_1w.clone(), Decimal::percent(1)).unwrap()
                    )
                ),
            ]
        );

        // register static limiter
        let res = execute(
            deps.as_mut(),
            mock_env(),
            mock_info(admin, &[]),
            ContractExecMsg::Transmuter(ExecMsg::RegisterLimiter {
                denom: "uosmo".to_string(),
                label: "static".to_string(),
                limiter_params: LimiterParams::StaticLimiter {
                    upper_limit: Decimal::percent(60),
                },
            }),
        )
        .unwrap();

        let attrs = vec![
            attr("method", "register_limiter"),
            attr("denom", "uosmo"),
            attr("label", "static"),
            attr("limiter_type", "static_limiter"),
            attr("upper_limit", "0.6"),
        ];

        assert_eq!(res.attributes, attrs);

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
            vec![
                (
                    (String::from("uosmo"), String::from("1w")),
                    Limiter::ChangeLimiter(
                        ChangeLimiter::new(window_config_1w.clone(), Decimal::percent(1)).unwrap()
                    )
                ),
                (
                    (String::from("uosmo"), String::from("static")),
                    Limiter::StaticLimiter(StaticLimiter::new(Decimal::percent(60)).unwrap())
                )
            ]
        );

        // set boundary offset by user is unauthorized
        let err = execute(
            deps.as_mut(),
            mock_env(),
            mock_info(user, &[]),
            ContractExecMsg::Transmuter(ExecMsg::SetChangeLimiterBoundaryOffset {
                denom: "uosmo".to_string(),
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
            ContractExecMsg::Transmuter(ExecMsg::SetChangeLimiterBoundaryOffset {
                denom: "uosmo".to_string(),
                label: "1h".to_string(),
                boundary_offset: Decimal::zero(),
            }),
        )
        .unwrap_err();

        assert_eq!(
            err,
            ContractError::LimiterDoesNotExist {
                denom: "uosmo".to_string(),
                label: "1h".to_string()
            }
        );

        // set boundary offset by admin for existing limiter should work
        let res = execute(
            deps.as_mut(),
            mock_env(),
            mock_info(admin, &[]),
            ContractExecMsg::Transmuter(ExecMsg::SetChangeLimiterBoundaryOffset {
                denom: "uosmo".to_string(),
                label: "1w".to_string(),
                boundary_offset: Decimal::percent(10),
            }),
        )
        .unwrap();

        let attrs = vec![
            attr("method", "set_change_limiter_boundary_offset"),
            attr("denom", "uosmo"),
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
            vec![
                (
                    (String::from("uosmo"), String::from("1w")),
                    Limiter::ChangeLimiter(
                        ChangeLimiter::new(window_config_1w.clone(), Decimal::percent(10)).unwrap()
                    )
                ),
                (
                    (String::from("uosmo"), String::from("static")),
                    Limiter::StaticLimiter(StaticLimiter::new(Decimal::percent(60)).unwrap())
                )
            ]
        );

        // set upper limit by user is unauthorized
        let err = execute(
            deps.as_mut(),
            mock_env(),
            mock_info(user, &[]),
            ContractExecMsg::Transmuter(ExecMsg::SetStaticLimiterUpperLimit {
                denom: "uosmo".to_string(),
                label: "static".to_string(),
                upper_limit: Decimal::percent(50),
            }),
        )
        .unwrap_err();

        assert_eq!(err, ContractError::Unauthorized {});

        // set upper limit by admin but for uosmo 1h should fail
        let err = execute(
            deps.as_mut(),
            mock_env(),
            mock_info(admin, &[]),
            ContractExecMsg::Transmuter(ExecMsg::SetStaticLimiterUpperLimit {
                denom: "uosmo".to_string(),
                label: "1h".to_string(),
                upper_limit: Decimal::percent(50),
            }),
        )
        .unwrap_err();

        assert_eq!(
            err,
            ContractError::LimiterDoesNotExist {
                denom: "uosmo".to_string(),
                label: "1h".to_string()
            }
        );

        // set upper limit by admin for change limiter should fail
        let err = execute(
            deps.as_mut(),
            mock_env(),
            mock_info(admin, &[]),
            ContractExecMsg::Transmuter(ExecMsg::SetStaticLimiterUpperLimit {
                denom: "uosmo".to_string(),
                label: "1w".to_string(),
                upper_limit: Decimal::percent(50),
            }),
        )
        .unwrap_err();

        assert_eq!(
            err,
            ContractError::WrongLimiterType {
                expected: "static_limiter".to_string(),
                actual: "change_limiter".to_string()
            }
        );

        // set upper limit by admin for static limiter should work
        let res = execute(
            deps.as_mut(),
            mock_env(),
            mock_info(admin, &[]),
            ContractExecMsg::Transmuter(ExecMsg::SetStaticLimiterUpperLimit {
                denom: "uosmo".to_string(),
                label: "static".to_string(),
                upper_limit: Decimal::percent(50),
            }),
        )
        .unwrap();

        let attrs = vec![
            attr("method", "set_static_limiter_upper_limit"),
            attr("denom", "uosmo"),
            attr("label", "static"),
            attr("upper_limit", "0.5"),
        ];

        assert_eq!(res.attributes, attrs);

        // Query the list of limiters
        let query_msg = ContractQueryMsg::Transmuter(QueryMsg::ListLimiters {});
        let res = query(deps.as_ref(), mock_env(), query_msg).unwrap();
        let limiters: ListLimitersResponse = from_binary(&res).unwrap();

        assert_eq!(
            limiters.limiters,
            vec![
                (
                    (String::from("uosmo"), String::from("1w")),
                    Limiter::ChangeLimiter(
                        ChangeLimiter::new(window_config_1w, Decimal::percent(10)).unwrap()
                    )
                ),
                (
                    (String::from("uosmo"), String::from("static")),
                    Limiter::StaticLimiter(StaticLimiter::new(Decimal::percent(50)).unwrap())
                )
            ]
        );
    }

    #[test]
    fn test_set_alloyed_denom_metadata() {
        let mut deps = mock_dependencies();

        // make denom has non-zero total supply
        deps.querier
            .update_balance("someone", vec![Coin::new(1, "uosmo"), Coin::new(1, "uion")]);

        let admin = "admin";
        let non_admin = "non_admin";
        let init_msg = InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("uosmo"),
                AssetConfig::from_denom_str("uion"),
            ],
            alloyed_asset_subdenom: "uosmouion".to_string(),
            admin: Some(admin.to_string()),
            moderator: None,
        };
        let env = mock_env();
        let info = mock_info(admin, &[]);

        // Instantiate the contract.
        instantiate(deps.as_mut(), env.clone(), info.clone(), init_msg).unwrap();

        let metadata = Metadata {
            description: "description".to_string(),
            base: "base".to_string(),
            display: "display".to_string(),
            name: "name".to_string(),
            symbol: "symbol".to_string(),
            denom_units: vec![],
        };

        // Attempt to set alloyed denom metadata by a non-admin user.
        let non_admin_info = mock_info(non_admin, &[]);
        let non_admin_msg = ContractExecMsg::Transmuter(ExecMsg::SetAlloyedDenomMetadata {
            metadata: metadata.clone(),
        });
        let err = execute(deps.as_mut(), env.clone(), non_admin_info, non_admin_msg).unwrap_err();

        assert_eq!(err, ContractError::Unauthorized {});

        // Set the alloyed denom metadata by admin.
        let msg = ContractExecMsg::Transmuter(ExecMsg::SetAlloyedDenomMetadata {
            metadata: metadata.clone(),
        });
        let res = execute(deps.as_mut(), env.clone(), info, msg).unwrap();

        assert_eq!(
            res.attributes,
            vec![attr("method", "set_alloyed_denom_metadata")]
        );

        assert_eq!(
            res.messages,
            vec![SubMsg::new(MsgSetDenomMetadata {
                sender: env.contract.address.to_string(),
                metadata: Some(metadata)
            })]
        )
    }

    #[test]
    fn test_join_pool() {
        let mut deps = mock_dependencies();

        // make denom has non-zero total supply
        deps.querier
            .update_balance("someone", vec![Coin::new(1, "uosmo"), Coin::new(1, "uion")]);

        let admin = "admin";
        let user = "user";
        let init_msg = InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("uosmo"),
                AssetConfig::from_denom_str("uion"),
            ],
            admin: Some(admin.to_string()),
            alloyed_asset_subdenom: "usomoion".to_string(),
            moderator: None,
        };
        let env = mock_env();
        let info = mock_info(admin, &[]);

        // Instantiate the contract.
        instantiate(deps.as_mut(), env.clone(), info, init_msg).unwrap();

        // Manually reply
        let alloyed_denom = "usomoion";

        reply(
            deps.as_mut(),
            env.clone(),
            Reply {
                id: 1,
                result: SubMsgResult::Ok(SubMsgResponse {
                    events: vec![],
                    data: Some(
                        MsgCreateDenomResponse {
                            new_token_denom: alloyed_denom.to_string(),
                        }
                        .into(),
                    ),
                }),
            },
        )
        .unwrap();

        // join pool with amount 0 coin should error
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        let info = mock_info(user, &[Coin::new(1000, "uion"), Coin::new(0, "uosmo")]);
        let err = execute(deps.as_mut(), env.clone(), info, join_pool_msg).unwrap_err();

        assert_eq!(err, ContractError::ZeroValueOperation {});

        // join pool properly works
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        let info = mock_info(user, &[Coin::new(1000, "uion"), Coin::new(1000, "uosmo")]);
        execute(deps.as_mut(), env.clone(), info, join_pool_msg).unwrap();

        // Check pool asset
        let GetTotalPoolLiquidityResponse {
            total_pool_liquidity,
        } = from_binary(
            &query(
                deps.as_ref(),
                env,
                ContractQueryMsg::Transmuter(QueryMsg::GetTotalPoolLiquidity {}),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            total_pool_liquidity,
            vec![Coin::new(1000, "uosmo"), Coin::new(1000, "uion")]
        );
    }

    #[test]
    fn test_exit_pool() {
        let mut deps = mock_dependencies();

        // make denom has non-zero total supply
        deps.querier
            .update_balance("someone", vec![Coin::new(1, "uosmo"), Coin::new(1, "uion")]);

        let admin = "admin";
        let user = "user";
        let init_msg = InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("uosmo"),
                AssetConfig::from_denom_str("uion"),
            ],
            admin: Some(admin.to_string()),
            alloyed_asset_subdenom: "usomoion".to_string(),
            moderator: None,
        };
        let env = mock_env();
        let info = mock_info(admin, &[]);

        // Instantiate the contract.
        instantiate(deps.as_mut(), env.clone(), info, init_msg).unwrap();

        // Manually reply
        let alloyed_denom = "usomoion";

        reply(
            deps.as_mut(),
            env.clone(),
            Reply {
                id: 1,
                result: SubMsgResult::Ok(SubMsgResponse {
                    events: vec![],
                    data: Some(
                        MsgCreateDenomResponse {
                            new_token_denom: alloyed_denom.to_string(),
                        }
                        .into(),
                    ),
                }),
            },
        )
        .unwrap();

        // join pool by others for sufficient amount
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        let info = mock_info(admin, &[Coin::new(1000, "uion"), Coin::new(1000, "uosmo")]);
        execute(deps.as_mut(), env.clone(), info, join_pool_msg).unwrap();

        // User tries to exit pool
        let exit_pool_msg = ContractExecMsg::Transmuter(ExecMsg::ExitPool {
            tokens_out: vec![Coin::new(1000, "uion"), Coin::new(1000, "uosmo")],
        });
        let err = execute(
            deps.as_mut(),
            env.clone(),
            mock_info(user, &[]),
            exit_pool_msg,
        )
        .unwrap_err();

        assert_eq!(
            err,
            ContractError::InsufficientShares {
                required: 2000u128.into(),
                available: Uint128::zero()
            }
        );
        // User tries to join pool
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        let res = execute(
            deps.as_mut(),
            env.clone(),
            mock_info(user, &[Coin::new(1000, "uion"), Coin::new(1000, "uosmo")]),
            join_pool_msg,
        );
        assert!(res.is_ok());

        deps.querier
            .update_balance(user, vec![Coin::new(2000, alloyed_denom)]);

        // User tries to exit pool with zero amount
        let exit_pool_msg = ContractExecMsg::Transmuter(ExecMsg::ExitPool {
            tokens_out: vec![Coin::new(0, "uion"), Coin::new(1, "uosmo")],
        });
        let err = execute(
            deps.as_mut(),
            env.clone(),
            mock_info(user, &[]),
            exit_pool_msg,
        )
        .unwrap_err();
        assert_eq!(err, ContractError::ZeroValueOperation {});

        // User tries to exit pool again
        let exit_pool_msg = ContractExecMsg::Transmuter(ExecMsg::ExitPool {
            tokens_out: vec![Coin::new(1000, "uion"), Coin::new(1000, "uosmo")],
        });
        let res = execute(
            deps.as_mut(),
            env.clone(),
            mock_info(user, &[]),
            exit_pool_msg,
        )
        .unwrap();

        let expected = Response::new()
            .add_attribute("method", "exit_pool")
            .add_message(MsgBurn {
                sender: env.contract.address.to_string(),
                amount: Some(Coin::new(2000u128, alloyed_denom).into()),
                burn_from_address: user.to_string(),
            })
            .add_message(BankMsg::Send {
                to_address: user.to_string(),
                amount: vec![Coin::new(1000, "uion"), Coin::new(1000, "uosmo")],
            });

        assert_eq!(res, expected);
    }

    #[test]
    fn test_shares_and_liquidity() {
        let mut deps = mock_dependencies();

        // make denom has non-zero total supply
        deps.querier
            .update_balance("someone", vec![Coin::new(1, "uosmo"), Coin::new(1, "uion")]);

        let admin = "admin";
        let user_1 = "user_1";
        let user_2 = "user_2";
        let init_msg = InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("uosmo"),
                AssetConfig::from_denom_str("uion"),
            ],
            admin: Some(admin.to_string()),
            alloyed_asset_subdenom: "usomoion".to_string(),
            moderator: None,
        };
        let env = mock_env();
        let info = mock_info(admin, &[]);

        // Instantiate the contract.
        instantiate(deps.as_mut(), env.clone(), info, init_msg).unwrap();

        // Manually reply
        let alloyed_denom = "usomoion";

        reply(
            deps.as_mut(),
            env.clone(),
            Reply {
                id: 1,
                result: SubMsgResult::Ok(SubMsgResponse {
                    events: vec![],
                    data: Some(
                        MsgCreateDenomResponse {
                            new_token_denom: alloyed_denom.to_string(),
                        }
                        .into(),
                    ),
                }),
            },
        )
        .unwrap();

        // Join pool
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        execute(
            deps.as_mut(),
            env.clone(),
            mock_info(user_1, &[Coin::new(1000, "uion"), Coin::new(1000, "uosmo")]),
            join_pool_msg,
        )
        .unwrap();

        // Update alloyed asset denom balance for user
        deps.querier
            .update_balance(user_1, vec![Coin::new(2000, "usomoion")]);

        // Query the shares of the user
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::GetShares {
                address: user_1.to_string(),
            }),
        )
        .unwrap();
        let shares: GetSharesResponse = from_binary(&res).unwrap();
        assert_eq!(shares.shares.u128(), 2000u128);

        // Query the total shares
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::GetTotalShares {}),
        )
        .unwrap();
        let total_shares: GetTotalSharesResponse = from_binary(&res).unwrap();
        assert_eq!(total_shares.total_shares.u128(), 2000u128);

        // Query the total pool liquidity
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::GetTotalPoolLiquidity {}),
        )
        .unwrap();
        let total_pool_liquidity: GetTotalPoolLiquidityResponse = from_binary(&res).unwrap();
        assert_eq!(
            total_pool_liquidity.total_pool_liquidity,
            vec![Coin::new(1000, "uosmo"), Coin::new(1000, "uion")]
        );

        // Join pool
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        execute(
            deps.as_mut(),
            env.clone(),
            mock_info(user_2, &[Coin::new(1000, "uion")]),
            join_pool_msg,
        )
        .unwrap();

        // Update balance for user 2
        deps.querier
            .update_balance(user_2, vec![Coin::new(1000, "usomoion")]);

        // Query the total shares
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::GetTotalShares {}),
        )
        .unwrap();

        let total_shares: GetTotalSharesResponse = from_binary(&res).unwrap();

        assert_eq!(total_shares.total_shares.u128(), 3000u128);

        // Query the total pool liquidity
        let res = query(
            deps.as_ref(),
            env,
            ContractQueryMsg::Transmuter(QueryMsg::GetTotalPoolLiquidity {}),
        )
        .unwrap();

        let total_pool_liquidity: GetTotalPoolLiquidityResponse = from_binary(&res).unwrap();

        assert_eq!(
            total_pool_liquidity.total_pool_liquidity,
            vec![Coin::new(1000, "uosmo"), Coin::new(2000, "uion")]
        );
    }

    #[test]
    fn test_denom() {
        let mut deps = mock_dependencies();

        // make denom has non-zero total supply
        deps.querier
            .update_balance("someone", vec![Coin::new(1, "uosmo"), Coin::new(1, "uion")]);

        let admin = "admin";
        let init_msg = InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("uosmo"),
                AssetConfig::from_denom_str("uion"),
            ],
            admin: Some(admin.to_string()),
            alloyed_asset_subdenom: "usomoion".to_string(),
            moderator: None,
        };
        let env = mock_env();
        let info = mock_info(admin, &[]);

        // Instantiate the contract.
        instantiate(deps.as_mut(), env.clone(), info, init_msg).unwrap();

        // Manually reply
        let alloyed_denom = "usomoion";

        reply(
            deps.as_mut(),
            env.clone(),
            Reply {
                id: 1,
                result: SubMsgResult::Ok(SubMsgResponse {
                    events: vec![],
                    data: Some(
                        MsgCreateDenomResponse {
                            new_token_denom: alloyed_denom.to_string(),
                        }
                        .into(),
                    ),
                }),
            },
        )
        .unwrap();

        // Query the share denom
        let res = query(
            deps.as_ref(),
            env,
            ContractQueryMsg::Transmuter(QueryMsg::GetShareDenom {}),
        )
        .unwrap();

        let share_denom: GetShareDenomResponse = from_binary(&res).unwrap();
        assert_eq!(share_denom.share_denom, "usomoion");
    }

    #[test]
    fn test_spot_price() {
        let mut deps = mock_dependencies();

        // make denom has non-zero total supply
        deps.querier
            .update_balance("someone", vec![Coin::new(1, "uosmo"), Coin::new(1, "uion")]);

        let admin = "admin";
        let init_msg = InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("uosmo"),
                AssetConfig::from_denom_str("uion"),
            ],
            admin: Some(admin.to_string()),
            alloyed_asset_subdenom: "usomoion".to_string(),
            moderator: None,
        };
        let env = mock_env();
        let info = mock_info(admin, &[]);

        // Instantiate the contract.
        instantiate(deps.as_mut(), env.clone(), info, init_msg).unwrap();

        // Manually reply
        let alloyed_denom = "usomoion";

        reply(
            deps.as_mut(),
            env.clone(),
            Reply {
                id: 1,
                result: SubMsgResult::Ok(SubMsgResponse {
                    events: vec![],
                    data: Some(
                        MsgCreateDenomResponse {
                            new_token_denom: alloyed_denom.to_string(),
                        }
                        .into(),
                    ),
                }),
            },
        )
        .unwrap();

        // Test spot price with same denom
        let err = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::SpotPrice {
                quote_asset_denom: "uosmo".to_string(),
                base_asset_denom: "uosmo".to_string(),
            }),
        )
        .unwrap_err();

        assert_eq!(
            err,
            ContractError::SpotPriceQueryFailed {
                reason: "quote_asset_denom and base_asset_denom cannot be the same".to_string()
            }
        );

        // Test spot price with denom not in swappable assets
        let err = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::SpotPrice {
                quote_asset_denom: "uatom".to_string(),
                base_asset_denom: "uosmo".to_string(),
            }),
        )
        .unwrap_err();

        assert_eq!(
            err,
            ContractError::SpotPriceQueryFailed {
                reason: "quote_asset_denom is not in swappable assets: must be one of [\"uosmo\", \"uion\", \"usomoion\"] but got uatom".to_string()
            }
        );

        let err = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::SpotPrice {
                quote_asset_denom: "uion".to_string(),
                base_asset_denom: "uatom".to_string(),
            }),
        )
        .unwrap_err();

        assert_eq!(
            err,
            ContractError::SpotPriceQueryFailed {
                reason: "base_asset_denom is not in swappable assets: must be one of [\"uosmo\", \"uion\", \"usomoion\"] but got uatom".to_string()
            }
        );

        // Test spot price with pool assets
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::SpotPrice {
                quote_asset_denom: "uosmo".to_string(),
                base_asset_denom: "uion".to_string(),
            }),
        )
        .unwrap();

        let spot_price: SpotPriceResponse = from_binary(&res).unwrap();
        assert_eq!(spot_price.spot_price, Decimal::one());

        // Test spot price with alloyed denom
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::SpotPrice {
                quote_asset_denom: "uosmo".to_string(),
                base_asset_denom: alloyed_denom.to_string(),
            }),
        )
        .unwrap();

        let spot_price: SpotPriceResponse = from_binary(&res).unwrap();
        assert_eq!(spot_price.spot_price, Decimal::one());

        let res = query(
            deps.as_ref(),
            env,
            ContractQueryMsg::Transmuter(QueryMsg::SpotPrice {
                quote_asset_denom: alloyed_denom.to_string(),
                base_asset_denom: "uion".to_string(),
            }),
        )
        .unwrap();

        let spot_price: SpotPriceResponse = from_binary(&res).unwrap();
        assert_eq!(spot_price.spot_price, Decimal::one());
    }

    #[test]
    fn test_calc_out_amt_given_in() {
        let mut deps = mock_dependencies();

        // make denom has non-zero total supply
        deps.querier.update_balance(
            "someone",
            vec![Coin::new(1, "axlusdc"), Coin::new(1, "whusdc")],
        );

        let admin = "admin";
        let init_msg = InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("axlusdc"),
                AssetConfig::from_denom_str("whusdc"),
            ],
            admin: Some(admin.to_string()),
            alloyed_asset_subdenom: "alloyedusdc".to_string(),
            moderator: None,
        };
        let env = mock_env();
        let info = mock_info(admin, &[]);

        // Instantiate the contract.
        instantiate(deps.as_mut(), env.clone(), info, init_msg).unwrap();

        // Manually reply
        let alloyed_denom = "alloyedusdc";

        reply(
            deps.as_mut(),
            env.clone(),
            Reply {
                id: 1,
                result: SubMsgResult::Ok(SubMsgResponse {
                    events: vec![],
                    data: Some(
                        MsgCreateDenomResponse {
                            new_token_denom: alloyed_denom.to_string(),
                        }
                        .into(),
                    ),
                }),
            },
        )
        .unwrap();

        // join pool
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        execute(
            deps.as_mut(),
            env.clone(),
            mock_info(
                admin,
                &[Coin::new(1000, "axlusdc"), Coin::new(2000, "whusdc")],
            ),
            join_pool_msg,
        )
        .unwrap();

        struct Case {
            name: String,
            token_in: Coin,
            token_out_denom: String,
            swap_fee: Decimal,
            expected: Result<CalcOutAmtGivenInResponse, ContractError>,
        }

        for Case {
            name,
            token_in,
            token_out_denom,
            swap_fee,
            expected,
        } in vec![
            Case {
                name: String::from("axlusdc to whusdc - ok"),
                token_in: Coin::new(1000, "axlusdc"),
                token_out_denom: "whusdc".to_string(),
                swap_fee: Decimal::zero(),
                expected: Ok(CalcOutAmtGivenInResponse {
                    token_out: Coin::new(1000, "whusdc"),
                }),
            },
            Case {
                name: String::from("whusdc to axlusdc - ok"),
                token_in: Coin::new(1000, "whusdc"),
                token_out_denom: "axlusdc".to_string(),
                swap_fee: Decimal::zero(),
                expected: Ok(CalcOutAmtGivenInResponse {
                    token_out: Coin::new(1000, "axlusdc"),
                }),
            },
            Case {
                name: String::from("whusdc to axlusdc - token out not enough"),
                token_in: Coin::new(1001, "whusdc"),
                token_out_denom: "axlusdc".to_string(),
                swap_fee: Decimal::zero(),
                expected: Err(ContractError::InsufficientPoolAsset {
                    required: Coin::new(1001, "axlusdc"),
                    available: Coin::new(1000, "axlusdc"),
                }),
            },
            Case {
                name: String::from("same denom error (pool asset)"),
                token_in: Coin::new(1000, "axlusdc"),
                token_out_denom: "axlusdc".to_string(),
                swap_fee: Decimal::zero(),
                expected: Err(StdError::generic_err(
                    "token_in_denom and token_out_denom cannot be the same",
                )
                .into()),
            },
            Case {
                name: String::from("same denom error (alloyed asset)"),
                token_in: Coin::new(1000, "alloyedusdc"),
                token_out_denom: "alloyedusdc".to_string(),
                swap_fee: Decimal::zero(),
                expected: Err(StdError::generic_err(
                    "token_in_denom and token_out_denom cannot be the same",
                )
                .into()),
            },
            Case {
                name: String::from("alloyedusdc to axlusdc - ok"),
                token_in: Coin::new(1000, "alloyedusdc"),
                token_out_denom: "axlusdc".to_string(),
                swap_fee: Decimal::zero(),
                expected: Ok(CalcOutAmtGivenInResponse {
                    token_out: Coin::new(1000, "axlusdc"),
                }),
            },
            Case {
                name: String::from("alloyedusdc to whusdc - ok"),
                token_in: Coin::new(1000, "alloyedusdc"),
                token_out_denom: "whusdc".to_string(),
                swap_fee: Decimal::zero(),
                expected: Ok(CalcOutAmtGivenInResponse {
                    token_out: Coin::new(1000, "whusdc"),
                }),
            },
            Case {
                name: String::from("alloyedusdc to axlusdc - token out not enough"),
                token_in: Coin::new(1001, "alloyedusdc"),
                token_out_denom: "axlusdc".to_string(),
                swap_fee: Decimal::zero(),
                expected: Err(ContractError::InsufficientPoolAsset {
                    required: Coin::new(1001, "axlusdc"),
                    available: Coin::new(1000, "axlusdc"),
                }),
            },
            Case {
                name: String::from("axlusdc to alloyedusdc - ok"),
                token_in: Coin::new(1000, "axlusdc"),
                token_out_denom: "alloyedusdc".to_string(),
                swap_fee: Decimal::zero(),
                expected: Ok(CalcOutAmtGivenInResponse {
                    token_out: Coin::new(1000, "alloyedusdc"),
                }),
            },
            Case {
                name: String::from("whusdc to alloyedusdc - ok"),
                token_in: Coin::new(1000, "whusdc"),
                token_out_denom: "alloyedusdc".to_string(),
                swap_fee: Decimal::zero(),
                expected: Ok(CalcOutAmtGivenInResponse {
                    token_out: Coin::new(1000, "alloyedusdc"),
                }),
            },
            Case {
                name: String::from("invalid swap fee"),
                token_in: Coin::new(1000, "axlusdc"),
                token_out_denom: "whusdc".to_string(),
                swap_fee: Decimal::percent(1),
                expected: Err(ContractError::InvalidSwapFee {
                    expected: Decimal::zero(),
                    actual: Decimal::percent(1),
                }),
            },
            Case {
                name: String::from("invalid swap fee (alloyed asset as token in)"),
                token_in: Coin::new(1000, "alloyedusdc"),
                token_out_denom: "whusdc".to_string(),
                swap_fee: Decimal::percent(1),
                expected: Err(ContractError::InvalidSwapFee {
                    expected: Decimal::zero(),
                    actual: Decimal::percent(1),
                }),
            },
            Case {
                name: String::from("invalid swap fee (alloyed asset as token out)"),
                token_in: Coin::new(1000, "axlusdc"),
                token_out_denom: "alloyedusdc".to_string(),
                swap_fee: Decimal::percent(2),
                expected: Err(ContractError::InvalidSwapFee {
                    expected: Decimal::zero(),
                    actual: Decimal::percent(2),
                }),
            },
        ] {
            let res = query(
                deps.as_ref(),
                env.clone(),
                ContractQueryMsg::Transmuter(QueryMsg::CalcOutAmtGivenIn {
                    token_in: token_in.clone(),
                    token_out_denom: token_out_denom.clone(),
                    swap_fee,
                }),
            )
            .map(|value| from_binary(&value).unwrap());

            assert_eq!(res, expected, "case: {}", name);
        }
    }

    #[test]
    fn test_calc_in_amt_given_out() {
        let mut deps = mock_dependencies();

        // make denom has non-zero total supply
        deps.querier.update_balance(
            "someone",
            vec![Coin::new(1, "axlusdc"), Coin::new(1, "whusdc")],
        );

        let admin = "admin";
        let init_msg = InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("axlusdc"),
                AssetConfig::from_denom_str("whusdc"),
            ],
            admin: Some(admin.to_string()),
            alloyed_asset_subdenom: "alloyedusdc".to_string(),
            moderator: None,
        };
        let env = mock_env();
        let info = mock_info(admin, &[]);

        // Instantiate the contract.
        instantiate(deps.as_mut(), env.clone(), info, init_msg).unwrap();

        // Manually reply
        let alloyed_denom = "alloyedusdc";

        reply(
            deps.as_mut(),
            env.clone(),
            Reply {
                id: 1,
                result: SubMsgResult::Ok(SubMsgResponse {
                    events: vec![],
                    data: Some(
                        MsgCreateDenomResponse {
                            new_token_denom: alloyed_denom.to_string(),
                        }
                        .into(),
                    ),
                }),
            },
        )
        .unwrap();

        // join pool
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        execute(
            deps.as_mut(),
            env.clone(),
            mock_info(
                admin,
                &[Coin::new(1000, "axlusdc"), Coin::new(2000, "whusdc")],
            ),
            join_pool_msg,
        )
        .unwrap();

        struct Case {
            name: String,
            token_in_denom: String,
            token_out: Coin,
            swap_fee: Decimal,
            expected: Result<CalcInAmtGivenOutResponse, ContractError>,
        }

        for Case {
            name,
            token_in_denom,
            token_out,
            swap_fee,
            expected,
        } in vec![
            Case {
                name: String::from("axlusdc to whusdc - ok"),
                token_in_denom: "axlusdc".to_string(),
                token_out: Coin::new(1000, "whusdc"),
                swap_fee: Decimal::zero(),
                expected: Ok(CalcInAmtGivenOutResponse {
                    token_in: Coin::new(1000, "axlusdc"),
                }),
            },
            Case {
                name: String::from("whusdc to axlusdc - ok"),
                token_in_denom: "whusdc".to_string(),
                token_out: Coin::new(1000, "axlusdc"),
                swap_fee: Decimal::zero(),
                expected: Ok(CalcInAmtGivenOutResponse {
                    token_in: Coin::new(1000, "whusdc"),
                }),
            },
            Case {
                name: String::from("whusdc to axlusdc - token out not enough"),
                token_in_denom: "whusdc".to_string(),
                token_out: Coin::new(1001, "axlusdc"),
                swap_fee: Decimal::zero(),
                expected: Err(ContractError::InsufficientPoolAsset {
                    required: Coin::new(1001, "axlusdc"),
                    available: Coin::new(1000, "axlusdc"),
                }),
            },
            Case {
                name: String::from("same denom error (pool asset)"),
                token_in_denom: "axlusdc".to_string(),
                token_out: Coin::new(1000, "axlusdc"),
                swap_fee: Decimal::zero(),
                expected: Err(StdError::generic_err(
                    "token_in_denom and token_out_denom cannot be the same",
                )
                .into()),
            },
            Case {
                name: String::from("same denom error (alloyed asset)"),
                token_in_denom: "alloyedusdc".to_string(),
                token_out: Coin::new(1000, "alloyedusdc"),
                swap_fee: Decimal::zero(),
                expected: Err(StdError::generic_err(
                    "token_in_denom and token_out_denom cannot be the same",
                )
                .into()),
            },
            Case {
                name: String::from("alloyedusdc to axlusdc - ok"),
                token_in_denom: "alloyedusdc".to_string(),
                token_out: Coin::new(1000, "axlusdc"),
                swap_fee: Decimal::zero(),
                expected: Ok(CalcInAmtGivenOutResponse {
                    token_in: Coin::new(1000, "alloyedusdc"),
                }),
            },
            Case {
                name: String::from("alloyedusdc to whusdc - ok"),
                token_in_denom: "alloyedusdc".to_string(),
                token_out: Coin::new(1000, "whusdc"),
                swap_fee: Decimal::zero(),
                expected: Ok(CalcInAmtGivenOutResponse {
                    token_in: Coin::new(1000, "alloyedusdc"),
                }),
            },
            Case {
                name: String::from("alloyedusdc to axlusdc - token out not enough"),
                token_in_denom: "alloyedusdc".to_string(),
                token_out: Coin::new(1001, "axlusdc"),
                swap_fee: Decimal::zero(),
                expected: Err(ContractError::InsufficientPoolAsset {
                    required: Coin::new(1001, "axlusdc"),
                    available: Coin::new(1000, "axlusdc"),
                }),
            },
            Case {
                name: String::from("pool asset to alloyed asset - ok"),
                token_in_denom: "axlusdc".to_string(),
                token_out: Coin::new(1000, "alloyedusdc"),
                swap_fee: Decimal::zero(),
                expected: Ok(CalcInAmtGivenOutResponse {
                    token_in: Coin::new(1000, "axlusdc"),
                }),
            },
            Case {
                name: String::from("invalid swap fee"),
                token_in_denom: "whusdc".to_string(),
                token_out: Coin::new(1000, "axlusdc"),
                swap_fee: Decimal::percent(1),
                expected: Err(ContractError::InvalidSwapFee {
                    expected: Decimal::zero(),
                    actual: Decimal::percent(1),
                }),
            },
            Case {
                name: String::from("invalid swap fee (alloyed asset as token in)"),
                token_in_denom: "alloyedusdc".to_string(),
                token_out: Coin::new(1000, "axlusdc"),
                swap_fee: Decimal::percent(1),
                expected: Err(ContractError::InvalidSwapFee {
                    expected: Decimal::zero(),
                    actual: Decimal::percent(1),
                }),
            },
            Case {
                name: String::from("invalid swap fee (alloyed asset as token out)"),
                token_in_denom: "whusdc".to_string(),
                token_out: Coin::new(1000, "alloyedusdc"),
                swap_fee: Decimal::percent(2),
                expected: Err(ContractError::InvalidSwapFee {
                    expected: Decimal::zero(),
                    actual: Decimal::percent(2),
                }),
            },
        ] {
            let res = query(
                deps.as_ref(),
                env.clone(),
                ContractQueryMsg::Transmuter(QueryMsg::CalcInAmtGivenOut {
                    token_in_denom: token_in_denom.clone(),
                    token_out: token_out.clone(),
                    swap_fee,
                }),
            )
            .map(|value| from_binary(&value).unwrap());

            assert_eq!(res, expected, "case: {}", name);
        }
    }
}
