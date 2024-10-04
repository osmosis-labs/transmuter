use crate::{corruptable::Corruptable, scope::Scope};
use std::{collections::BTreeMap, iter};

use crate::{
    alloyed_asset::AlloyedAsset,
    asset::{Asset, AssetConfig},
    ensure_admin_authority, ensure_moderator_authority,
    error::{non_empty_input_required, nonpayable, ContractError},
    limiter::{Limiter, LimiterParams, Limiters},
    math::{self, rescale},
    role::Role,
    swap::{BurnTarget, Entrypoint, SwapFromAlloyedConstraint, SwapToAlloyedConstraint, SWAP_FEE},
    transmuter_pool::{AssetGroup, TransmuterPool},
};
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    ensure, ensure_ne, Addr, Coin, Decimal, DepsMut, Env, Reply, Response, StdError, Storage,
    SubMsg, Uint128,
};

use cw_storage_plus::Item;
use osmosis_std::types::{
    cosmos::bank::v1beta1::Metadata,
    osmosis::tokenfactory::v1beta1::{MsgCreateDenom, MsgCreateDenomResponse, MsgSetDenomMetadata},
};

use sylvia::{
    contract,
    types::{ExecCtx, InstantiateCtx, QueryCtx},
};

/// version info for migration
pub const CONTRACT_NAME: &str = "crates.io:transmuter";
pub const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

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

pub mod key {
    pub const ACTIVE_STATUS: &str = "active_status";
    pub const POOL: &str = "pool";
    pub const ALLOYED_ASSET_DENOM: &str = "alloyed_denom";
    pub const ALLOYED_ASSET_NORMALIZATION_FACTOR: &str = "alloyed_asset_normalization_factor";
    pub const ADMIN: &str = "admin";
    pub const MODERATOR: &str = "moderator";
    pub const LIMITERS: &str = "limiters";
    pub const ASSET_GROUP: &str = "asset_group";
}

#[contract]
#[sv::error(ContractError)]
impl Transmuter<'_> {
    /// Create a Transmuter instance.
    pub const fn default() -> Self {
        Self {
            active_status: Item::new(key::ACTIVE_STATUS),
            pool: Item::new(key::POOL),
            alloyed_asset: AlloyedAsset::new(
                key::ALLOYED_ASSET_DENOM,
                key::ALLOYED_ASSET_NORMALIZATION_FACTOR,
            ),
            role: Role::new(key::ADMIN, key::MODERATOR),
            limiters: Limiters::new(key::LIMITERS),
        }
    }

    /// Instantiate the contract.
    #[sv::msg(instantiate)]
    pub fn instantiate(
        &self,
        InstantiateCtx { deps, env, info }: InstantiateCtx,
        pool_asset_configs: Vec<AssetConfig>,
        alloyed_asset_subdenom: String,
        alloyed_asset_normalization_factor: Uint128,
        admin: Option<String>,
        moderator: String,
    ) -> Result<Response, ContractError> {
        nonpayable(&info.funds)?;

        // store contract version for migration info
        cw2::set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

        // set admin if exists
        if let Some(admin) = admin {
            self.role
                .admin
                .init(deps.storage, deps.api.addr_validate(&admin)?)?;
        }

        // set moderator
        self.role
            .moderator
            .init(deps.storage, deps.api.addr_validate(&moderator)?)?;

        let pool_assets = pool_asset_configs
            .into_iter()
            .map(|config| AssetConfig::checked_init_asset(config, deps.as_ref()))
            .collect::<Result<Vec<_>, ContractError>>()?;

        // store pool
        self.pool
            .save(deps.storage, &TransmuterPool::new(pool_assets)?)?;

        // set active status to true
        self.active_status.save(deps.storage, &true)?;

        // subdenom must not contain extra parts
        ensure!(
            !alloyed_asset_subdenom.contains('/'),
            ContractError::SubDenomExtraPartsNotAllowed {
                subdenom: alloyed_asset_subdenom
            }
        );

        // create alloyed denom
        let msg_create_alloyed_denom = SubMsg::reply_on_success(
            MsgCreateDenom {
                sender: env.contract.address.to_string(),
                subdenom: format!("{}/{}", ALLOYED_PREFIX, alloyed_asset_subdenom),
            },
            CREATE_ALLOYED_DENOM_REPLY_ID,
        );

        // set normalization factor for alloyed asset
        self.alloyed_asset
            .set_normalization_factor(deps.storage, alloyed_asset_normalization_factor)?;

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

    #[sv::msg(exec)]
    fn rescale_normalization_factor(
        &self,
        ExecCtx { deps, env: _, info }: ExecCtx,
        numerator: Uint128,
        denominator: Uint128,
    ) -> Result<Response, ContractError> {
        nonpayable(&info.funds)?;

        // only admin can rescale normalization factor
        ensure_admin_authority!(info.sender, self.role.admin, deps.as_ref());

        // rescale normalization factor for pool assets
        self.pool.update(deps.storage, |pool| {
            pool.update_normalization_factor(|factor| {
                rescale(factor, numerator, denominator).map_err(Into::into)
            })
        })?;

        // rescale normalization factor for alloyed asset
        let alloyed_asset_normalization_factor =
            self.alloyed_asset.get_normalization_factor(deps.storage)?;

        let updated_alloyed_asset_normalization_factor =
            rescale(alloyed_asset_normalization_factor, numerator, denominator)?;

        self.alloyed_asset
            .set_normalization_factor(deps.storage, updated_alloyed_asset_normalization_factor)?;

        Ok(Response::new()
            .add_attribute("method", "rescale_normalization_factor")
            .add_attribute("numerator", numerator)
            .add_attribute("denominator", denominator))
    }

    #[sv::msg(exec)]
    fn add_new_assets(
        &self,
        ExecCtx { deps, env, info }: ExecCtx,
        asset_configs: Vec<AssetConfig>,
    ) -> Result<Response, ContractError> {
        non_empty_input_required("asset_configs", &asset_configs)?;
        nonpayable(&info.funds)?;

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

        let asset_weights_iter = pool
            .asset_weights()?
            .unwrap_or_default()
            .into_iter()
            .map(|(denom, weight)| (Scope::denom(&denom).key(), weight));
        let asset_group_weights_iter = pool
            .asset_group_weights()?
            .into_iter()
            .map(|(label, weight)| (Scope::asset_group(&label).key(), weight));

        self.limiters.reset_change_limiter_states(
            deps.storage,
            env.block.time,
            asset_weights_iter.chain(asset_group_weights_iter),
        )?;

        Ok(Response::new().add_attribute("method", "add_new_assets"))
    }

    #[sv::msg(exec)]
    fn create_asset_group(
        &self,
        ExecCtx { deps, env: _, info }: ExecCtx,
        label: String,
        denoms: Vec<String>,
    ) -> Result<Response, ContractError> {
        nonpayable(&info.funds)?;

        // only admin can create asset group
        ensure_admin_authority!(info.sender, self.role.admin, deps.as_ref());

        // create asset group
        self.pool
            .update(deps.storage, |mut pool| -> Result<_, ContractError> {
                pool.create_asset_group(label.clone(), denoms)?;
                Ok(pool)
            })?;

        Ok(Response::new()
            .add_attribute("method", "create_asset_group")
            .add_attribute("label", label))
    }

    #[sv::msg(exec)]
    fn remove_asset_group(
        &self,
        ExecCtx { deps, env: _, info }: ExecCtx,
        label: String,
    ) -> Result<Response, ContractError> {
        nonpayable(&info.funds)?;

        // only admin can remove asset group
        ensure_admin_authority!(info.sender, self.role.admin, deps.as_ref());

        self.pool
            .update(deps.storage, |mut pool| -> Result<_, ContractError> {
                pool.remove_asset_group(&label)?;
                Ok(pool)
            })?;

        // remove all limiters for asset group
        let limiters = self
            .limiters
            .list_limiters_by_scope(deps.storage, &Scope::AssetGroup(label.clone()))?;

        for (limiter_label, _) in limiters {
            self.limiters.unchecked_deregister(
                deps.storage,
                Scope::AssetGroup(label.clone()),
                &limiter_label,
            )?;
        }

        Ok(Response::new()
            .add_attribute("method", "remove_asset_group")
            .add_attribute("label", label))
    }

    /// Mark designated scopes as corrupted scopes.
    /// As a result, the corrupted scopes will not allowed to be increased by any means,
    /// both in terms of amount and weight.
    /// The only way to redeem other pool asset outside of the corrupted scopes is
    /// to also redeem asset within the corrupted scopes
    /// with the same pool-defnined value.
    #[sv::msg(exec)]
    fn mark_corrupted_scopes(
        &self,
        ExecCtx { deps, env: _, info }: ExecCtx,
        scopes: Vec<Scope>,
    ) -> Result<Response, ContractError> {
        non_empty_input_required("scopes", &scopes)?;
        nonpayable(&info.funds)?;

        // only moderator can mark corrupted assets
        ensure_moderator_authority!(info.sender, self.role.moderator, deps.as_ref());

        let mut pool = self.pool.load(deps.storage)?;

        for scope in &scopes {
            match scope {
                Scope::Denom(denom) => {
                    pool.mark_corrupted_asset(denom)?;
                }
                Scope::AssetGroup(label) => {
                    pool.mark_corrupted_asset_group(label)?;
                }
            }
        }

        self.pool.save(deps.storage, &pool)?;

        Ok(Response::new().add_attribute("method", "mark_corrupted_scopes"))
    }

    #[sv::msg(exec)]
    fn unmark_corrupted_scopes(
        &self,
        ExecCtx { deps, env: _, info }: ExecCtx,
        scopes: Vec<Scope>,
    ) -> Result<Response, ContractError> {
        non_empty_input_required("scopes", &scopes)?;
        nonpayable(&info.funds)?;

        // only moderator can unmark corrupted assets
        ensure_moderator_authority!(info.sender, self.role.moderator, deps.as_ref());

        let mut pool = self.pool.load(deps.storage)?;

        for scope in &scopes {
            match scope {
                Scope::Denom(denom) => {
                    pool.unmark_corrupted_asset(denom)?;
                }
                Scope::AssetGroup(label) => {
                    pool.unmark_corrupted_asset_group(label)?;
                }
            }
        }

        self.pool.save(deps.storage, &pool)?;

        Ok(Response::new().add_attribute("method", "unmark_corrupted_scopes"))
    }

    #[sv::msg(exec)]
    fn register_limiter(
        &self,
        ExecCtx { deps, env: _, info }: ExecCtx,
        scope: Scope,
        label: String,
        limiter_params: LimiterParams,
    ) -> Result<Response, ContractError> {
        nonpayable(&info.funds)?;

        // only admin can register limiter
        ensure_admin_authority!(info.sender, self.role.admin, deps.as_ref());

        let pool = self.pool.load(deps.storage)?;

        match scope.clone() {
            Scope::Denom(denom) => {
                // ensure pool has the specified denom
                ensure!(
                    pool.has_denom(&denom),
                    ContractError::InvalidPoolAssetDenom { denom }
                );
            }
            Scope::AssetGroup(label) => {
                // check if asset group exists
                ensure!(
                    pool.has_asset_group(&label),
                    ContractError::AssetGroupNotFound { label }
                );
            }
        };

        let scope_key = scope.key();
        let base_attrs = vec![
            ("method", "register_limiter"),
            ("label", &label),
            ("scope", &scope_key),
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
            .register(deps.storage, scope, &label, limiter_params)?;

        Ok(Response::new()
            .add_attributes(base_attrs)
            .add_attributes(limiter_attrs))
    }

    #[sv::msg(exec)]
    fn deregister_limiter(
        &self,
        ExecCtx { deps, env: _, info }: ExecCtx,
        scope: Scope,
        label: String,
    ) -> Result<Response, ContractError> {
        nonpayable(&info.funds)?;

        // only admin can deregister limiter
        ensure_admin_authority!(info.sender, self.role.admin, deps.as_ref());

        let scope_key = scope.key();
        let attrs = vec![
            ("method", "deregister_limiter"),
            ("scope", &scope_key),
            ("label", &label),
        ];

        // deregister limiter
        self.limiters.deregister(deps.storage, scope, &label)?;

        Ok(Response::new().add_attributes(attrs))
    }

    #[sv::msg(exec)]
    fn set_change_limiter_boundary_offset(
        &self,
        ExecCtx { deps, env: _, info }: ExecCtx,
        scope: Scope,
        label: String,
        boundary_offset: Decimal,
    ) -> Result<Response, ContractError> {
        nonpayable(&info.funds)?;

        // only admin can set boundary offset
        ensure_admin_authority!(info.sender, self.role.admin, deps.as_ref());

        let boundary_offset_string = boundary_offset.to_string();
        let scope_key = scope.key();
        let attrs = vec![
            ("method", "set_change_limiter_boundary_offset"),
            ("scope", &scope_key),
            ("label", &label),
            ("boundary_offset", boundary_offset_string.as_str()),
        ];

        // set boundary offset
        self.limiters.set_change_limiter_boundary_offset(
            deps.storage,
            scope,
            &label,
            boundary_offset,
        )?;

        Ok(Response::new().add_attributes(attrs))
    }

    #[sv::msg(exec)]
    fn set_static_limiter_upper_limit(
        &self,
        ExecCtx { deps, env: _, info }: ExecCtx,
        scope: Scope,
        label: String,
        upper_limit: Decimal,
    ) -> Result<Response, ContractError> {
        nonpayable(&info.funds)?;

        // only admin can set upper limit
        ensure_admin_authority!(info.sender, self.role.admin, deps.as_ref());

        let upper_limit_string = upper_limit.to_string();
        let scope_key = scope.key();

        let attrs = vec![
            ("method", "set_static_limiter_upper_limit"),
            ("scope", &scope_key),
            ("label", &label),
            ("upper_limit", upper_limit_string.as_str()),
        ];

        // set upper limit
        self.limiters
            .set_static_limiter_upper_limit(deps.storage, scope, &label, upper_limit)?;

        Ok(Response::new().add_attributes(attrs))
    }

    #[sv::msg(exec)]
    pub fn set_alloyed_denom_metadata(
        &self,
        ExecCtx { deps, env, info }: ExecCtx,
        metadata: Metadata,
    ) -> Result<Response, ContractError> {
        nonpayable(&info.funds)?;

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

    #[sv::msg(exec)]
    fn set_active_status(
        &self,
        ExecCtx { deps, env: _, info }: ExecCtx,
        active: bool,
    ) -> Result<Response, ContractError> {
        nonpayable(&info.funds)?;

        // only moderator can set active status
        ensure_moderator_authority!(info.sender, self.role.moderator, deps.as_ref());

        // set active status
        self.checked_set_active_status(deps.storage, active)?;

        Ok(Response::new()
            .add_attribute("method", "set_active_status")
            .add_attribute("active", active.to_string()))
    }

    pub(crate) fn checked_set_active_status(
        &self,
        storage: &mut dyn Storage,
        active: bool,
    ) -> Result<bool, ContractError> {
        self.active_status
            .update(storage, |prev_active| -> Result<bool, ContractError> {
                ensure_ne!(
                    prev_active,
                    active,
                    ContractError::UnchangedActiveStatus { status: active }
                );

                Ok(active)
            })
    }

    /// Join pool with tokens that exist in the pool.
    /// Token used to join pool is sent to the contract via `funds` in `MsgExecuteContract`.
    #[sv::msg(exec)]
    pub fn join_pool(
        &self,
        ExecCtx { deps, env, info }: ExecCtx,
    ) -> Result<Response, ContractError> {
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
    #[sv::msg(exec)]
    pub fn exit_pool(
        &self,
        ExecCtx { deps, env, info }: ExecCtx,
        tokens_out: Vec<Coin>,
    ) -> Result<Response, ContractError> {
        // it will deduct shares directly from the sender's account
        nonpayable(&info.funds)?;

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

    #[sv::msg(query)]
    fn list_asset_configs(
        &self,
        QueryCtx { deps, env: _ }: QueryCtx,
    ) -> Result<ListAssetConfigsResponse, ContractError> {
        let pool = self.pool.load(deps.storage)?;
        let alloyed_asset_config = AssetConfig {
            denom: self.alloyed_asset.get_alloyed_denom(deps.storage)?,
            normalization_factor: self.alloyed_asset.get_normalization_factor(deps.storage)?,
        };

        Ok(ListAssetConfigsResponse {
            asset_configs: pool
                .pool_assets
                .iter()
                .map(|asset| asset.config())
                .chain(iter::once(alloyed_asset_config))
                .collect(),
        })
    }

    #[sv::msg(query)]
    fn list_limiters(
        &self,
        QueryCtx { deps, env: _ }: QueryCtx,
    ) -> Result<ListLimitersResponse, ContractError> {
        let limiters = self.limiters.list_limiters(deps.storage)?;

        Ok(ListLimitersResponse { limiters })
    }

    #[sv::msg(query)]
    fn list_asset_groups(
        &self,
        QueryCtx { deps, env: _ }: QueryCtx,
    ) -> Result<ListAssetGroupsResponse, ContractError> {
        let pool = self.pool.load(deps.storage)?;

        Ok(ListAssetGroupsResponse {
            asset_groups: pool.asset_groups,
        })
    }

    #[sv::msg(query)]
    pub fn get_shares(
        &self,
        QueryCtx { deps, env: _ }: QueryCtx,
        address: String,
    ) -> Result<GetSharesResponse, ContractError> {
        Ok(GetSharesResponse {
            shares: self
                .alloyed_asset
                .get_balance(deps, &deps.api.addr_validate(&address)?)?,
        })
    }

    #[sv::msg(query)]
    pub(crate) fn get_share_denom(
        &self,
        QueryCtx { deps, env: _ }: QueryCtx,
    ) -> Result<GetShareDenomResponse, ContractError> {
        Ok(GetShareDenomResponse {
            share_denom: self.alloyed_asset.get_alloyed_denom(deps.storage)?,
        })
    }

    #[sv::msg(query)]
    pub(crate) fn get_swap_fee(&self, _ctx: QueryCtx) -> Result<GetSwapFeeResponse, ContractError> {
        Ok(GetSwapFeeResponse { swap_fee: SWAP_FEE })
    }

    #[sv::msg(query)]
    pub(crate) fn is_active(
        &self,
        QueryCtx { deps, env: _ }: QueryCtx,
    ) -> Result<IsActiveResponse, ContractError> {
        Ok(IsActiveResponse {
            is_active: self.active_status.load(deps.storage)?,
        })
    }

    #[sv::msg(query)]
    pub(crate) fn get_total_shares(
        &self,
        QueryCtx { deps, env: _ }: QueryCtx,
    ) -> Result<GetTotalSharesResponse, ContractError> {
        let total_shares = self.alloyed_asset.get_total_supply(deps)?;
        Ok(GetTotalSharesResponse { total_shares })
    }

    #[sv::msg(query)]
    pub(crate) fn get_total_pool_liquidity(
        &self,
        QueryCtx { deps, env: _ }: QueryCtx,
    ) -> Result<GetTotalPoolLiquidityResponse, ContractError> {
        let pool = self.pool.load(deps.storage)?;

        Ok(GetTotalPoolLiquidityResponse {
            total_pool_liquidity: pool.pool_assets.iter().map(Asset::to_coin).collect(),
        })
    }

    #[sv::msg(query)]
    pub(crate) fn spot_price(
        &self,
        QueryCtx { deps, env: _ }: QueryCtx,
        base_asset_denom: String,
        quote_asset_denom: String,
    ) -> Result<SpotPriceResponse, ContractError> {
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
        let alloyed_normalization_factor =
            self.alloyed_asset.get_normalization_factor(deps.storage)?;
        let swappable_asset_norm_factors = pool
            .pool_assets
            .iter()
            .map(|c| (c.denom().to_string(), c.normalization_factor()))
            .chain(vec![(alloyed_denom, alloyed_normalization_factor)])
            .collect::<BTreeMap<String, Uint128>>();

        let base_asset_norm_factor = swappable_asset_norm_factors
            .get(&base_asset_denom)
            .cloned()
            .ok_or_else(|| ContractError::SpotPriceQueryFailed {
                reason: format!(
                    "base_asset_denom is not in swappable assets: must be one of {:?} but got {}",
                    swappable_asset_norm_factors.keys(),
                    base_asset_denom
                ),
            })?;

        let quote_asset_norm_factor = swappable_asset_norm_factors
            .get(&quote_asset_denom)
            .cloned()
            .ok_or_else(|| ContractError::SpotPriceQueryFailed {
                reason: format!(
                    "quote_asset_denom is not in swappable assets: must be one of {:?} but got {}",
                    swappable_asset_norm_factors.keys(),
                    quote_asset_denom
                ),
            })?;

        Ok(SpotPriceResponse {
            spot_price: math::price(base_asset_norm_factor, quote_asset_norm_factor)?,
        })
    }

    #[sv::msg(query)]
    pub(crate) fn calc_out_amt_given_in(
        &self,
        QueryCtx { deps, env: _ }: QueryCtx,
        token_in: Coin,
        token_out_denom: String,
        swap_fee: Decimal,
    ) -> Result<CalcOutAmtGivenInResponse, ContractError> {
        self.ensure_valid_swap_fee(swap_fee)?;
        let pool = self.pool.load(deps.storage)?;
        let (_pool, token_out) = self.out_amt_given_in(deps, pool, token_in, &token_out_denom)?;

        Ok(CalcOutAmtGivenInResponse { token_out })
    }

    #[sv::msg(query)]
    pub(crate) fn calc_in_amt_given_out(
        &self,
        QueryCtx { deps, env: _ }: QueryCtx,
        token_out: Coin,
        token_in_denom: String,
        swap_fee: Decimal,
    ) -> Result<CalcInAmtGivenOutResponse, ContractError> {
        self.ensure_valid_swap_fee(swap_fee)?;
        let pool = self.pool.load(deps.storage)?;
        let (_pool, token_in) = self.in_amt_given_out(deps, pool, token_out, token_in_denom)?;

        Ok(CalcInAmtGivenOutResponse { token_in })
    }

    #[sv::msg(query)]
    pub(crate) fn get_corrupted_scopes(
        &self,
        QueryCtx { deps, env: _ }: QueryCtx,
    ) -> Result<GetCorrruptedScopesResponse, ContractError> {
        let pool = self.pool.load(deps.storage)?;

        let corrupted_assets = pool
            .corrupted_assets()
            .into_iter()
            .map(|a| Scope::denom(a.denom()));

        let corrupted_asset_groups = pool
            .asset_groups
            .iter()
            .filter(|(_, asset_group)| asset_group.is_corrupted())
            .map(|(label, _)| Scope::asset_group(label));

        Ok(GetCorrruptedScopesResponse {
            corrupted_scopes: corrupted_assets.chain(corrupted_asset_groups).collect(),
        })
    }

    // --- admin ---

    #[sv::msg(exec)]
    pub fn transfer_admin(
        &self,
        ExecCtx { deps, env: _, info }: ExecCtx,
        candidate: String,
    ) -> Result<Response, ContractError> {
        let candidate_addr = deps.api.addr_validate(&candidate)?;
        self.role
            .admin
            .transfer(deps, info.sender, candidate_addr)?;

        Ok(Response::new()
            .add_attribute("method", "transfer_admin")
            .add_attribute("candidate", candidate))
    }

    #[sv::msg(exec)]
    pub fn cancel_admin_transfer(
        &self,
        ExecCtx { deps, env: _, info }: ExecCtx,
    ) -> Result<Response, ContractError> {
        self.role.admin.cancel_transfer(deps, info.sender)?;

        Ok(Response::new().add_attribute("method", "cancel_admin_transfer"))
    }

    #[sv::msg(exec)]
    pub fn reject_admin_transfer(
        &self,
        ExecCtx { deps, env: _, info }: ExecCtx,
    ) -> Result<Response, ContractError> {
        self.role.admin.reject_transfer(deps, info.sender)?;

        Ok(Response::new().add_attribute("method", "reject_admin_transfer"))
    }

    #[sv::msg(exec)]
    pub fn claim_admin(
        &self,
        ExecCtx { deps, env: _, info }: ExecCtx,
    ) -> Result<Response, ContractError> {
        let sender_string = info.sender.to_string();
        self.role.admin.claim(deps, info.sender)?;

        Ok(Response::new()
            .add_attribute("method", "claim_admin")
            .add_attribute("new_admin", sender_string))
    }

    #[sv::msg(query)]
    fn get_admin(
        &self,
        QueryCtx { deps, env: _ }: QueryCtx,
    ) -> Result<GetAdminResponse, ContractError> {
        Ok(GetAdminResponse {
            admin: self.role.admin.current(deps)?,
        })
    }

    #[sv::msg(query)]
    fn get_admin_candidate(
        &self,
        QueryCtx { deps, env: _ }: QueryCtx,
    ) -> Result<GetAdminCandidateResponse, ContractError> {
        Ok(GetAdminCandidateResponse {
            admin_candidate: self.role.admin.candidate(deps)?,
        })
    }

    // -- moderator --
    #[sv::msg(exec)]
    pub fn assign_moderator(
        &self,
        ExecCtx { deps, env: _, info }: ExecCtx,
        address: String,
    ) -> Result<Response, ContractError> {
        let moderator_address = deps.api.addr_validate(&address)?;

        self.role
            .assign_moderator(info.sender, deps, moderator_address)?;

        Ok(Response::new()
            .add_attribute("method", "assign_moderator")
            .add_attribute("moderator", address))
    }

    #[sv::msg(query)]
    fn get_moderator(
        &self,
        QueryCtx { deps, env: _ }: QueryCtx,
    ) -> Result<GetModeratorResponse, ContractError> {
        Ok(GetModeratorResponse {
            moderator: self.role.moderator.get(deps)?,
        })
    }
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
pub struct ListAssetGroupsResponse {
    pub asset_groups: BTreeMap<String, AssetGroup>,
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
pub struct GetCorrruptedScopesResponse {
    pub corrupted_scopes: Vec<Scope>,
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

    use super::sv::*;
    use super::*;
    use crate::limiter::{ChangeLimiter, StaticLimiter, WindowConfig};
    use crate::sudo::SudoMsg;
    use crate::*;

    use cosmwasm_std::testing::{mock_dependencies, mock_env, mock_info};
    use cosmwasm_std::{
        attr, from_json, BankMsg, BlockInfo, Storage, SubMsgResponse, SubMsgResult, Uint64,
    };
    use osmosis_std::types::osmosis::tokenfactory::v1beta1::MsgBurn;

    #[test]
    fn test_invalid_subdenom() {
        let mut deps = mock_dependencies();

        // make denom has non-zero total supply
        deps.querier
            .update_balance("someone", vec![Coin::new(1, "tbtc"), Coin::new(1, "nbtc")]);

        let admin = "admin";
        let moderator = "moderator";
        let init_msg = InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("tbtc"),
                AssetConfig::from_denom_str("nbtc"),
            ],
            alloyed_asset_subdenom: "all/btc".to_string(),
            alloyed_asset_normalization_factor: Uint128::one(),
            admin: Some(admin.to_string()),
            moderator: moderator.to_string(),
        };
        let env = mock_env();
        let info = mock_info(admin, &[]);

        // Instantiate the contract.
        let err = instantiate(deps.as_mut(), env.clone(), info.clone(), init_msg).unwrap_err();

        assert_eq!(
            err,
            ContractError::SubDenomExtraPartsNotAllowed {
                subdenom: "all/btc".to_string()
            }
        )
    }

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
        let moderator = "moderator";
        let init_msg = InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("uosmo"),
                AssetConfig::from_denom_str("uion"),
            ],
            alloyed_asset_subdenom: "uosmouion".to_string(),
            alloyed_asset_normalization_factor: Uint128::one(),
            admin: Some(admin.to_string()),
            moderator: moderator.to_string(),
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

        // join pool
        let info = mock_info(
            "someone",
            &[
                Coin::new(1000000000, "uosmo"),
                Coin::new(1000000000, "uion"),
            ],
        );
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        execute(deps.as_mut(), env.clone(), info.clone(), join_pool_msg).unwrap();

        // Create asset group
        let create_asset_group_msg = ContractExecMsg::Transmuter(ExecMsg::CreateAssetGroup {
            label: "group1".to_string(),
            denoms: vec!["uosmo".to_string(), "uion".to_string()],
        });

        let info = mock_info(admin, &[]);
        execute(
            deps.as_mut(),
            env.clone(),
            info.clone(),
            create_asset_group_msg,
        )
        .unwrap();

        // set limiters
        let change_limiter_params = LimiterParams::ChangeLimiter {
            window_config: WindowConfig {
                window_size: Uint64::from(3600u64),
                division_count: Uint64::from(10u64),
            },
            boundary_offset: Decimal::percent(20),
        };

        let static_limiter_params = LimiterParams::StaticLimiter {
            upper_limit: Decimal::percent(60),
        };

        // Register limiter for the asset group
        let register_group_limiter_msg = ContractExecMsg::Transmuter(ExecMsg::RegisterLimiter {
            scope: Scope::AssetGroup("group1".to_string()),
            label: "group_change_limiter".to_string(),
            limiter_params: change_limiter_params.clone(),
        });

        execute(
            deps.as_mut(),
            env.clone(),
            info.clone(),
            register_group_limiter_msg,
        )
        .unwrap();

        let info = mock_info(admin, &[]);
        for denom in ["uosmo", "uion"] {
            let register_limiter_msg = ContractExecMsg::Transmuter(ExecMsg::RegisterLimiter {
                scope: Scope::Denom(denom.to_string()),
                label: "change_limiter".to_string(),
                limiter_params: change_limiter_params.clone(),
            });

            execute(
                deps.as_mut(),
                env.clone(),
                info.clone(),
                register_limiter_msg,
            )
            .unwrap();

            let register_limiter_msg = ContractExecMsg::Transmuter(ExecMsg::RegisterLimiter {
                scope: Scope::Denom(denom.to_string()),
                label: "static_limiter".to_string(),
                limiter_params: static_limiter_params.clone(),
            });

            execute(
                deps.as_mut(),
                env.clone(),
                info.clone(),
                register_limiter_msg,
            )
            .unwrap();
        }

        // join pool a bit more to make limiters dirty
        let mut env = env.clone();
        env.block.time = env.block.time.plus_nanos(360);

        let info = mock_info(
            "someone",
            &[Coin::new(550, "uosmo"), Coin::new(500, "uion")],
        );
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        execute(deps.as_mut(), env.clone(), info.clone(), join_pool_msg).unwrap();

        env.block.time = env.block.time.plus_nanos(3000);
        let info = mock_info(
            "someone",
            &[Coin::new(450, "uosmo"), Coin::new(500, "uion")],
        );
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        execute(deps.as_mut(), env.clone(), info.clone(), join_pool_msg).unwrap();

        for denom in ["uosmo", "uion"] {
            assert_dirty_change_limiters_by_scope!(
                &Scope::denom(denom),
                Transmuter::default().limiters,
                deps.as_ref().storage
            );
        }

        assert_dirty_change_limiters_by_scope!(
            &Scope::asset_group("group1"),
            Transmuter::default().limiters,
            deps.as_ref().storage
        );

        // Add new assets

        // Attempt to add assets with invalid denom
        let info = mock_info(admin, &[]);
        let invalid_denoms = vec!["invalid_asset1".to_string(), "invalid_asset2".to_string()];
        let add_invalid_assets_msg = ContractExecMsg::Transmuter(ExecMsg::AddNewAssets {
            asset_configs: invalid_denoms
                .into_iter()
                .map(|denom| AssetConfig::from_denom_str(denom.as_str()))
                .collect(),
        });

        env.block.time = env.block.time.plus_nanos(360);

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

        env.block.time = env.block.time.plus_nanos(360);

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

        env.block.time = env.block.time.plus_nanos(360);

        // successful asset addition
        execute(deps.as_mut(), env.clone(), info, add_assets_msg).unwrap();

        let reset_at = env.block.time;
        let transmuter = Transmuter::default();

        // Reset change limiter states if new assets are added
        for denom in ["uosmo", "uion"] {
            assert_reset_change_limiters_by_scope!(
                &Scope::denom(denom),
                reset_at,
                transmuter,
                deps.as_ref().storage
            );
        }

        assert_reset_change_limiters_by_scope!(
            &Scope::asset_group("group1"),
            reset_at,
            transmuter,
            deps.as_ref().storage
        );

        env.block.time = env.block.time.plus_nanos(360);

        // Check if the new assets were added
        let res = query(
            deps.as_ref(),
            env,
            ContractQueryMsg::Transmuter(QueryMsg::GetTotalPoolLiquidity {}),
        )
        .unwrap();
        let GetTotalPoolLiquidityResponse {
            total_pool_liquidity,
        } = from_json(res).unwrap();

        assert_eq!(
            total_pool_liquidity,
            vec![
                Coin::new(1000001000, "uosmo"),
                Coin::new(1000001000, "uion"),
                Coin::new(0, "new_asset1"),
                Coin::new(0, "new_asset2"),
            ]
        );
    }

    #[test]
    fn test_corrupted_assets() {
        let mut deps = mock_dependencies();

        // make denom has non-zero total supply
        deps.querier.update_balance(
            "someone",
            vec![
                Coin::new(1, "wbtc"),
                Coin::new(1, "tbtc"),
                Coin::new(1, "nbtc"),
                Coin::new(1, "stbtc"),
            ],
        );

        let admin = "admin";
        let moderator = "moderator";
        let alloyed_subdenom = "btc";
        let init_msg = InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("wbtc"),
                AssetConfig::from_denom_str("tbtc"),
                AssetConfig::from_denom_str("nbtc"),
                AssetConfig::from_denom_str("stbtc"),
            ],
            alloyed_asset_subdenom: alloyed_subdenom.to_string(),
            alloyed_asset_normalization_factor: Uint128::one(),
            admin: Some(admin.to_string()),
            moderator: moderator.to_string(),
        };
        let env = mock_env();

        // Instantiate the contract.
        let info = mock_info(admin, &[]);
        instantiate(deps.as_mut(), env.clone(), info.clone(), init_msg).unwrap();

        // Manually reply
        let res = reply(
            deps.as_mut(),
            env.clone(),
            Reply {
                id: 1,
                result: SubMsgResult::Ok(SubMsgResponse {
                    events: vec![],
                    data: Some(
                        MsgCreateDenomResponse {
                            new_token_denom: alloyed_subdenom.to_string(),
                        }
                        .into(),
                    ),
                }),
            },
        )
        .unwrap();

        let alloyed_token_denom_kv = res.attributes[0].clone();
        assert_eq!(alloyed_token_denom_kv.key, "alloyed_denom");
        let alloyed_denom = alloyed_token_denom_kv.value;

        // set limiters
        let change_limiter_params = LimiterParams::ChangeLimiter {
            window_config: WindowConfig {
                window_size: Uint64::from(3600000000000u64),
                division_count: Uint64::from(5u64),
            },
            boundary_offset: Decimal::percent(20),
        };

        let static_limiter_params = LimiterParams::StaticLimiter {
            upper_limit: Decimal::percent(30),
        };

        // Mark corrupted assets by non-moderator
        let info = mock_info("someone", &[]);
        let mark_corrupted_assets_msg = ContractExecMsg::Transmuter(ExecMsg::MarkCorruptedScopes {
            scopes: vec![Scope::denom("wbtc"), Scope::denom("tbtc")],
        });

        let res = execute(
            deps.as_mut(),
            env.clone(),
            info.clone(),
            mark_corrupted_assets_msg,
        );

        // Check if the attempt resulted in Unauthorized error
        assert_eq!(res.unwrap_err(), ContractError::Unauthorized {});

        // Corrupted denoms must be empty
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::GetCorruptedScopes {}),
        )
        .unwrap();
        let GetCorrruptedScopesResponse { corrupted_scopes } = from_json(res).unwrap();

        assert_eq!(corrupted_scopes, Vec::<Scope>::new());

        // The asset must not yet be removed
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::GetTotalPoolLiquidity {}),
        )
        .unwrap();
        let GetTotalPoolLiquidityResponse {
            total_pool_liquidity,
        } = from_json(res).unwrap();

        assert_eq!(
            total_pool_liquidity,
            vec![
                Coin::new(0, "wbtc"),
                Coin::new(0, "tbtc"),
                Coin::new(0, "nbtc"),
                Coin::new(0, "stbtc"),
            ]
        );

        // provide some liquidity
        let liquidity = vec![
            Coin::new(1_000_000_000_000, "wbtc"),
            Coin::new(1_000_000_000_000, "tbtc"),
            Coin::new(1_000_000_000_000, "nbtc"),
            Coin::new(1_000_000_000_000, "stbtc"),
        ];
        deps.querier.update_balance("someone", liquidity.clone());

        let info = mock_info("someone", &liquidity);
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});

        execute(deps.as_mut(), env.clone(), info.clone(), join_pool_msg).unwrap();

        // set limiters
        for denom in ["wbtc", "tbtc", "nbtc", "stbtc"] {
            let register_limiter_msg = ContractExecMsg::Transmuter(ExecMsg::RegisterLimiter {
                scope: Scope::Denom(denom.to_string()),
                label: "change_limiter".to_string(),
                limiter_params: change_limiter_params.clone(),
            });

            let info = mock_info(admin, &[]);
            execute(
                deps.as_mut(),
                env.clone(),
                info.clone(),
                register_limiter_msg,
            )
            .unwrap();

            let register_limiter_msg = ContractExecMsg::Transmuter(ExecMsg::RegisterLimiter {
                scope: Scope::Denom(denom.to_string()),
                label: "static_limiter".to_string(),
                limiter_params: static_limiter_params.clone(),
            });
            execute(
                deps.as_mut(),
                env.clone(),
                info.clone(),
                register_limiter_msg,
            )
            .unwrap();
        }

        // Create asset group
        let create_asset_group_msg = ContractExecMsg::Transmuter(ExecMsg::CreateAssetGroup {
            label: "btc_group1".to_string(),
            denoms: vec!["nbtc".to_string(), "stbtc".to_string()],
        });

        let info = mock_info(admin, &[]);
        execute(
            deps.as_mut(),
            env.clone(),
            info.clone(),
            create_asset_group_msg,
        )
        .unwrap();

        // Register change limiter for the asset group
        let register_group_limiter_msg = ContractExecMsg::Transmuter(ExecMsg::RegisterLimiter {
            scope: Scope::AssetGroup("btc_group1".to_string()),
            label: "group_change_limiter".to_string(),
            limiter_params: change_limiter_params.clone(),
        });

        execute(
            deps.as_mut(),
            env.clone(),
            info.clone(),
            register_group_limiter_msg,
        )
        .unwrap();

        // exit pool a bit to make sure the limiters are dirty
        deps.querier
            .update_balance("someone", vec![Coin::new(1_000, alloyed_denom.clone())]);
        let exit_pool_msg = ContractExecMsg::Transmuter(ExecMsg::ExitPool {
            tokens_out: vec![Coin::new(1_000, "nbtc")],
        });

        let info = mock_info("someone", &[]);
        execute(deps.as_mut(), env.clone(), info.clone(), exit_pool_msg).unwrap();

        // Mark corrupted assets by moderator
        let corrupted_scopes = vec![Scope::denom("wbtc"), Scope::denom("tbtc")];
        let mark_corrupted_assets_msg = ContractExecMsg::Transmuter(ExecMsg::MarkCorruptedScopes {
            scopes: corrupted_scopes.clone(),
        });

        let info = mock_info(moderator, &[]);
        let res = execute(
            deps.as_mut(),
            env.clone(),
            info.clone(),
            mark_corrupted_assets_msg,
        )
        .unwrap();
        // no bank message should be sent, the corrupted asset waits for withdrawal
        assert_eq!(res.messages, vec![]);

        // corrupted denoms must be updated
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::GetCorruptedScopes {}),
        )
        .unwrap();
        let get_corrupted_scopes_response: GetCorrruptedScopesResponse = from_json(res).unwrap();

        assert_eq!(
            get_corrupted_scopes_response.corrupted_scopes,
            corrupted_scopes
        );

        // Check if the assets were removed
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::GetTotalPoolLiquidity {}),
        )
        .unwrap();
        let GetTotalPoolLiquidityResponse {
            total_pool_liquidity,
        } = from_json(res).unwrap();

        assert_eq!(
            total_pool_liquidity,
            vec![
                Coin::new(1_000_000_000_000, "wbtc"),
                Coin::new(1_000_000_000_000, "tbtc"),
                Coin::new(999_999_999_000, "nbtc"),
                Coin::new(1_000_000_000_000, "stbtc"),
            ]
        );

        // warm up the limiters
        let env = increase_block_height(&env, 1);
        deps.querier
            .update_balance("someone", vec![Coin::new(4, alloyed_denom.clone())]);
        let exit_pool_msg = ContractExecMsg::Transmuter(ExecMsg::ExitPool {
            tokens_out: vec![
                Coin::new(1, "wbtc"),
                Coin::new(1, "tbtc"),
                Coin::new(1, "nbtc"),
                Coin::new(1, "stbtc"),
            ],
        });
        let info = mock_info("someone", &[]);
        execute(deps.as_mut(), env.clone(), info.clone(), exit_pool_msg).unwrap();

        for denom in ["wbtc", "tbtc", "nbtc", "stbtc"] {
            assert_dirty_change_limiters_by_scope!(
                &Scope::denom(denom),
                Transmuter::default().limiters,
                deps.as_ref().storage
            );
        }

        assert_dirty_change_limiters_by_scope!(
            &Scope::asset_group("btc_group1"),
            Transmuter::default().limiters,
            deps.as_ref().storage
        );

        let env = increase_block_height(&env, 1);

        deps.querier
            .update_balance("someone", vec![Coin::new(4, alloyed_denom.clone())]);
        let exit_pool_msg = ContractExecMsg::Transmuter(ExecMsg::ExitPool {
            tokens_out: vec![
                Coin::new(1, "wbtc"),
                Coin::new(1, "tbtc"),
                Coin::new(1, "nbtc"),
                Coin::new(1, "stbtc"),
            ],
        });
        let info = mock_info("someone", &[]);
        execute(deps.as_mut(), env.clone(), info.clone(), exit_pool_msg).unwrap();

        let env = increase_block_height(&env, 1);

        for scope in corrupted_scopes {
            let expected_err = ContractError::CorruptedScopeRelativelyIncreased {
                scope: scope.clone(),
            };

            let denom = match scope {
                Scope::Denom(denom) => denom,
                _ => unreachable!(),
            };

            // join with corrupted denom should fail
            let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
            let err = execute(
                deps.as_mut(),
                env.clone(),
                mock_info("user", &[Coin::new(1000, denom.clone())]),
                join_pool_msg,
            )
            .unwrap_err();
            assert_eq!(expected_err, err);

            // swap exact in with corrupted denom as token in should fail
            let swap_msg = SudoMsg::SwapExactAmountIn {
                token_in: Coin::new(1000, denom.clone()),
                swap_fee: Decimal::zero(),
                sender: "mock_sender".to_string(),
                token_out_denom: "nbtc".to_string(),
                token_out_min_amount: Uint128::new(500),
            };

            let err = sudo(deps.as_mut(), env.clone(), swap_msg).unwrap_err();
            assert_eq!(expected_err, err);

            // swap exact in with corrupted denom as token out should be ok since it decreases the corrupted asset
            let swap_msg = SudoMsg::SwapExactAmountIn {
                token_in: Coin::new(1000, "nbtc"),
                swap_fee: Decimal::zero(),
                sender: "mock_sender".to_string(),
                token_out_denom: denom.clone(),
                token_out_min_amount: Uint128::new(500),
            };

            sudo(deps.as_mut(), env.clone(), swap_msg).unwrap();

            // swap exact out with corrupted denom as token out should be ok since it decreases the corrupted asset
            let swap_msg = SudoMsg::SwapExactAmountOut {
                sender: "mock_sender".to_string(),
                token_out: Coin::new(500, denom.clone()),
                swap_fee: Decimal::zero(),
                token_in_denom: "nbtc".to_string(),
                token_in_max_amount: Uint128::new(1000),
            };

            sudo(deps.as_mut(), env.clone(), swap_msg).unwrap();

            // swap exact out with corrupted denom as token in should fail
            let swap_msg = SudoMsg::SwapExactAmountOut {
                sender: "mock_sender".to_string(),
                token_out: Coin::new(500, "nbtc"),
                swap_fee: Decimal::zero(),
                token_in_denom: denom.clone(),
                token_in_max_amount: Uint128::new(1000),
            };

            let err = sudo(deps.as_mut(), env.clone(), swap_msg).unwrap_err();
            assert_eq!(expected_err, err);

            // exit with by any denom requires corrupted denom to not increase in weight
            // (this case increase other remaining corrupted denom weight)
            deps.querier.update_balance(
                "someone",
                vec![Coin::new(4_000_000_000, alloyed_denom.clone())],
            );

            let exit_pool_msg = ContractExecMsg::Transmuter(ExecMsg::ExitPool {
                tokens_out: vec![Coin::new(1_000_000_000, "stbtc")],
            });

            let info = mock_info("someone", &[]);

            // this causes all corrupted denoms to be increased in weight
            let err = execute(deps.as_mut(), env.clone(), info, exit_pool_msg).unwrap_err();
            assert!(matches!(
                err,
                ContractError::CorruptedScopeRelativelyIncreased { .. }
            ));

            let exit_pool_msg = ContractExecMsg::Transmuter(ExecMsg::ExitPool {
                tokens_out: vec![
                    Coin::new(1_000_000_000, "nbtc"),
                    Coin::new(1_000_000_000, denom.clone()),
                ],
            });

            let info = mock_info("someone", &[]);

            // this causes other corrupted denom to be increased relatively
            let err = execute(deps.as_mut(), env.clone(), info, exit_pool_msg).unwrap_err();
            assert!(matches!(
                err,
                ContractError::CorruptedScopeRelativelyIncreased { .. }
            ));
        }

        // exit with corrupted denom requires all corrupted denom exit with the same value
        deps.querier.update_balance(
            "someone",
            vec![Coin::new(4_000_000_000, alloyed_denom.clone())],
        );
        let info = mock_info("someone", &[]);
        let exit_pool_msg = ContractExecMsg::Transmuter(ExecMsg::ExitPool {
            tokens_out: vec![
                Coin::new(2_000_000_000, "nbtc"),
                Coin::new(1_000_000_000, "wbtc"),
                Coin::new(1_000_000_000, "tbtc"),
            ],
        });
        execute(deps.as_mut(), env.clone(), info, exit_pool_msg).unwrap();

        // force redeem corrupted assets

        deps.querier.update_balance(
            "someone",
            vec![Coin::new(1_000_000_000_000, alloyed_denom.clone())],
        );
        let all_nbtc = total_liquidity_of("nbtc", &deps.storage);
        let force_redeem_corrupted_assets_msg = ContractExecMsg::Transmuter(ExecMsg::ExitPool {
            tokens_out: vec![all_nbtc],
        });

        let info = mock_info("someone", &[]);
        let err = execute(
            deps.as_mut(),
            env.clone(),
            info.clone(),
            force_redeem_corrupted_assets_msg,
        )
        .unwrap_err();

        assert_eq!(
            err,
            ContractError::CorruptedScopeRelativelyIncreased {
                scope: Scope::denom("wbtc")
            }
        );

        let all_wbtc = total_liquidity_of("wbtc", &deps.storage);
        let force_redeem_corrupted_assets_msg = ContractExecMsg::Transmuter(ExecMsg::ExitPool {
            tokens_out: vec![all_wbtc],
        });

        deps.querier.update_balance(
            "someone",
            vec![Coin::new(1_000_000_000_000, alloyed_denom.clone())],
        );

        let info = mock_info("someone", &[]);
        execute(
            deps.as_mut(),
            env.clone(),
            info.clone(),
            force_redeem_corrupted_assets_msg,
        )
        .unwrap();

        // check liquidity
        let GetTotalPoolLiquidityResponse {
            total_pool_liquidity,
        } = from_json(
            query(
                deps.as_ref(),
                env.clone(),
                ContractQueryMsg::Transmuter(QueryMsg::GetTotalPoolLiquidity {}),
            )
            .unwrap(),
        )
        .unwrap();

        assert_eq!(
            total_pool_liquidity,
            vec![
                Coin::new(998999998498, "tbtc"),
                Coin::new(998000001998, "nbtc"),
                Coin::new(999999999998, "stbtc"),
            ]
        );

        assert_eq!(
            Transmuter::default()
                .limiters
                .list_limiters_by_scope(&deps.storage, &Scope::denom("wbtc"))
                .unwrap(),
            vec![]
        );

        for denom in ["tbtc", "nbtc", "stbtc"] {
            assert_reset_change_limiters_by_scope!(
                &Scope::denom(denom),
                env.block.time,
                Transmuter::default(),
                deps.as_ref().storage
            );
        }

        assert_reset_change_limiters_by_scope!(
            &Scope::asset_group("btc_group1"),
            env.block.time,
            Transmuter::default(),
            deps.as_ref().storage
        );

        // try unmark nbtc should fail
        let unmark_corrupted_assets_msg =
            ContractExecMsg::Transmuter(ExecMsg::UnmarkCorruptedScopes {
                scopes: vec![Scope::denom("nbtc")],
            });

        let info = mock_info(moderator, &[]);
        let err = execute(
            deps.as_mut(),
            env.clone(),
            info.clone(),
            unmark_corrupted_assets_msg,
        )
        .unwrap_err();

        assert_eq!(
            err,
            ContractError::InvalidCorruptedAssetDenom {
                denom: "nbtc".to_string()
            }
        );

        // unmark tbtc by non moderator should fail
        let unmark_corrupted_assets_msg =
            ContractExecMsg::Transmuter(ExecMsg::UnmarkCorruptedScopes {
                scopes: vec![Scope::denom("tbtc")],
            });

        let info = mock_info("someone", &[]);
        let err = execute(
            deps.as_mut(),
            env.clone(),
            info.clone(),
            unmark_corrupted_assets_msg,
        )
        .unwrap_err();

        assert_eq!(err, ContractError::Unauthorized {});

        // unmark tbtc
        let unmark_corrupted_assets_msg =
            ContractExecMsg::Transmuter(ExecMsg::UnmarkCorruptedScopes {
                scopes: vec![Scope::denom("tbtc")],
            });

        let info = mock_info(moderator, &[]);
        execute(
            deps.as_mut(),
            env.clone(),
            info.clone(),
            unmark_corrupted_assets_msg,
        )
        .unwrap();

        // query corrupted denoms
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::GetCorruptedScopes {}),
        )
        .unwrap();

        let GetCorrruptedScopesResponse { corrupted_scopes } = from_json(res).unwrap();

        assert_eq!(corrupted_scopes, Vec::<Scope>::new());

        // no liquidity or pool assets changes
        let GetTotalPoolLiquidityResponse {
            total_pool_liquidity,
        } = from_json(
            query(
                deps.as_ref(),
                env.clone(),
                ContractQueryMsg::Transmuter(QueryMsg::GetTotalPoolLiquidity {}),
            )
            .unwrap(),
        )
        .unwrap();

        assert_eq!(
            total_pool_liquidity,
            vec![
                Coin::new(998999998498, "tbtc"),
                Coin::new(998000001998, "nbtc"),
                Coin::new(999999999998, "stbtc"),
            ]
        );

        // still has all the limiters
        assert_eq!(
            Transmuter::default()
                .limiters
                .list_limiters_by_scope(&deps.storage, &Scope::denom("tbtc"))
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn test_corrupted_asset_group() {
        let mut deps = mock_dependencies();

        deps.querier.update_balance(
            "admin",
            vec![
                Coin::new(1_000_000_000_000, "tbtc"),
                Coin::new(1_000_000_000_000, "nbtc"),
                Coin::new(1_000_000_000_000, "stbtc"),
            ],
        );

        let env = mock_env();
        let info = mock_info("admin", &[]);

        // Initialize contract with asset group
        let init_msg = InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("tbtc"),
                AssetConfig::from_denom_str("nbtc"),
                AssetConfig::from_denom_str("stbtc"),
            ],
            alloyed_asset_subdenom: "btc".to_string(),
            alloyed_asset_normalization_factor: Uint128::one(),
            admin: Some("admin".to_string()),
            moderator: "moderator".to_string(),
        };

        instantiate(deps.as_mut(), env.clone(), info.clone(), init_msg).unwrap();

        // Manually reply
        let res = reply(
            deps.as_mut(),
            env.clone(),
            Reply {
                id: 1,
                result: SubMsgResult::Ok(SubMsgResponse {
                    events: vec![],
                    data: Some(
                        MsgCreateDenomResponse {
                            new_token_denom: "btc".to_string(),
                        }
                        .into(),
                    ),
                }),
            },
        )
        .unwrap();

        let alloyed_denom = res
            .attributes
            .into_iter()
            .find(|attr| attr.key == "alloyed_denom")
            .unwrap()
            .value;

        deps.querier.update_balance(
            "user",
            vec![Coin::new(3_000_000_000_000, alloyed_denom.clone())],
        );

        // Create asset group
        let create_group_msg = ContractExecMsg::Transmuter(ExecMsg::CreateAssetGroup {
            label: "group1".to_string(),
            denoms: vec!["tbtc".to_string(), "nbtc".to_string()],
        });
        execute(deps.as_mut(), env.clone(), info.clone(), create_group_msg).unwrap();

        // Set change limiter for btc group
        let info = mock_info("admin", &[]);
        let set_limiter_msg = ContractExecMsg::Transmuter(ExecMsg::RegisterLimiter {
            scope: Scope::asset_group("group1"),
            label: "big_change_limiter".to_string(),
            limiter_params: LimiterParams::ChangeLimiter {
                window_config: WindowConfig {
                    window_size: Uint64::from(3600000000000u64), // 1 hour in nanoseconds
                    division_count: Uint64::from(6u64),
                },
                boundary_offset: Decimal::percent(20),
            },
        });
        execute(deps.as_mut(), env.clone(), info.clone(), set_limiter_msg).unwrap();

        // set change limiter for stbtc
        let set_limiter_msg = ContractExecMsg::Transmuter(ExecMsg::RegisterLimiter {
            scope: Scope::denom("stbtc"),
            label: "big_change_limiter".to_string(),
            limiter_params: LimiterParams::ChangeLimiter {
                window_config: WindowConfig {
                    window_size: Uint64::from(3600000000000u64), // 1 hour in nanoseconds
                    division_count: Uint64::from(6u64),
                },
                boundary_offset: Decimal::percent(20),
            },
        });
        execute(deps.as_mut(), env.clone(), info.clone(), set_limiter_msg).unwrap();

        // Add some liquidity
        let add_liquidity_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        execute(
            deps.as_mut(),
            env.clone(),
            mock_info(
                "user",
                &[
                    Coin::new(1_000_000_000_000, "tbtc"),
                    Coin::new(1_000_000_000_000, "nbtc"),
                    Coin::new(1_000_000_000_000, "stbtc"),
                ],
            ),
            add_liquidity_msg,
        )
        .unwrap();

        // Assert dirty change limiters for the asset group
        assert_dirty_change_limiters_by_scope!(
            &Scope::asset_group("group1"),
            &Transmuter::default().limiters,
            &deps.storage
        );

        // Mark asset group as corrupted
        let info = mock_info("moderator", &[]);
        let mark_corrupted_msg = ContractExecMsg::Transmuter(ExecMsg::MarkCorruptedScopes {
            scopes: vec![Scope::asset_group("group1")],
        });
        execute(deps.as_mut(), env.clone(), info.clone(), mark_corrupted_msg).unwrap();

        // Query corrupted scopes
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::GetCorruptedScopes {}),
        )
        .unwrap();

        let GetCorrruptedScopesResponse { corrupted_scopes } = from_json(res).unwrap();

        assert_eq!(corrupted_scopes, vec![Scope::asset_group("group1")]);

        // Exit pool with all corrupted assets
        let env = increase_block_height(&env, 1);
        let info = mock_info("user", &[]);
        let exit_pool_msg = ContractExecMsg::Transmuter(ExecMsg::ExitPool {
            tokens_out: vec![
                Coin::new(1_000_000_000_000, "tbtc"),
                Coin::new(1_000_000_000_000, "nbtc"),
            ],
        });
        execute(deps.as_mut(), env.clone(), info.clone(), exit_pool_msg).unwrap();

        // Assert reset change limiters for the asset group
        assert_reset_change_limiters_by_scope!(
            &Scope::asset_group("group1"),
            env.block.time,
            Transmuter::default(),
            &deps.storage
        );

        // Query corrupted scopes again to ensure the asset group is no longer corrupted
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::GetCorruptedScopes {}),
        )
        .unwrap();

        let GetCorrruptedScopesResponse { corrupted_scopes } = from_json(res).unwrap();

        assert!(
            corrupted_scopes.is_empty(),
            "Corrupted scopes should be empty after exiting pool"
        );

        let msg = ContractQueryMsg::Transmuter(QueryMsg::GetTotalPoolLiquidity {});
        let res = query(deps.as_ref(), env.clone(), msg).unwrap();
        let GetTotalPoolLiquidityResponse {
            total_pool_liquidity,
        } = from_json(res).unwrap();

        assert_eq!(
            total_pool_liquidity,
            vec![Coin::new(1_000_000_000_000, "stbtc")]
        );

        // Assert that only one limiter remains for stbtc
        let limiters = Transmuter::default()
            .limiters
            .list_limiters(&deps.storage)
            .unwrap()
            .into_iter()
            .map(|(k, v)| k)
            .collect::<Vec<_>>();
        assert_eq!(
            limiters,
            vec![("denom::stbtc".to_string(), "big_change_limiter".to_string())]
        );

        // Assert reset change limiters for the individual assets
        assert_reset_change_limiters_by_scope!(
            &Scope::denom("stbtc"),
            env.block.time,
            Transmuter::default(),
            &deps.storage
        );
    }

    fn increase_block_height(env: &Env, height: u64) -> Env {
        let block_time = 5; // hypothetical block time
        Env {
            block: BlockInfo {
                height: env.block.height + height,
                time: env.block.time.plus_seconds(block_time * height),
                chain_id: env.block.chain_id.clone(),
            },
            ..env.clone()
        }
    }

    fn total_liquidity_of(denom: &str, storage: &dyn Storage) -> Coin {
        Transmuter::default()
            .pool
            .load(storage)
            .unwrap()
            .pool_assets
            .into_iter()
            .find(|a| a.denom() == denom)
            .unwrap()
            .to_coin()
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
            alloyed_asset_normalization_factor: Uint128::one(),
            admin: Some(admin.to_string()),
            moderator: moderator.to_string(),
        };
        let env = mock_env();
        let info = mock_info(admin, &[]);

        // Instantiate the contract.
        instantiate(deps.as_mut(), env.clone(), info, init_msg).unwrap();

        // Manually set alloyed denom
        let alloyed_denom = "uosmo".to_string();

        let transmuter = Transmuter::default();
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
        let active_status: IsActiveResponse = from_json(res).unwrap();
        assert!(active_status.is_active);

        // Attempt to set the active status by a non-admin user.
        let non_admin_info = mock_info("non_moderator", &[]);
        let non_admin_msg = ContractExecMsg::Transmuter(ExecMsg::SetActiveStatus { active: false });
        let err = execute(deps.as_mut(), env.clone(), non_admin_info, non_admin_msg).unwrap_err();

        assert_eq!(err, ContractError::Unauthorized {});

        // Set the active status to false.
        let msg = ContractExecMsg::Transmuter(ExecMsg::SetActiveStatus { active: false });
        execute(
            deps.as_mut(),
            env.clone(),
            mock_info(moderator, &[]),
            msg.clone(),
        )
        .unwrap();

        // Check the active status again.
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::IsActive {}),
        )
        .unwrap();
        let active_status: IsActiveResponse = from_json(res).unwrap();
        assert!(!active_status.is_active);

        // try to set the active status to false again
        let err = execute(deps.as_mut(), env.clone(), mock_info(moderator, &[]), msg).unwrap_err();
        assert_eq!(err, ContractError::UnchangedActiveStatus { status: false });

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
        let active_status: IsActiveResponse = from_json(res).unwrap();
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
        let active_status: IsActiveResponse = from_json(res).unwrap();
        assert!(!active_status.is_active);

        // Set the active status back to true through sudo
        let set_active_status_msg = SudoMsg::SetActive { is_active: true };
        let res = sudo(deps.as_mut(), env.clone(), set_active_status_msg);
        assert!(res.is_ok());

        // Check the active status again.
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::IsActive {}),
        )
        .unwrap();
        let active_status: IsActiveResponse = from_json(res).unwrap();
        assert!(active_status.is_active);

        // try to set active status to true when it's already true
        let set_active_status_msg = SudoMsg::SetActive { is_active: true };

        let err = sudo(deps.as_mut(), env, set_active_status_msg).unwrap_err();

        assert_eq!(err, ContractError::UnchangedActiveStatus { status: true });
    }

    #[test]
    fn test_transfer_and_claim_admin() {
        let mut deps = mock_dependencies();

        // make denom has non-zero total supply
        deps.querier
            .update_balance("someone", vec![Coin::new(1, "uosmo"), Coin::new(1, "uion")]);

        let admin = "admin";
        let moderator = "moderator";
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
            alloyed_asset_normalization_factor: Uint128::one(),
            moderator: moderator.to_string(),
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
        let admin_candidate: GetAdminCandidateResponse = from_json(res).unwrap();
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
        let admin_candidate: GetAdminCandidateResponse = from_json(res).unwrap();
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
        let admin_candidate: GetAdminCandidateResponse = from_json(res).unwrap();
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
        let admin_candidate: GetAdminCandidateResponse = from_json(res).unwrap();
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
        let admin_candidate: GetAdminCandidateResponse = from_json(res).unwrap();
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
        let admin: GetAdminResponse = from_json(res).unwrap();
        assert_eq!(admin.admin.as_str(), candidate);
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
            alloyed_asset_normalization_factor: Uint128::one(),
            moderator: moderator.to_string(),
        };
        instantiate(deps.as_mut(), mock_env(), mock_info(admin, &[]), init_msg).unwrap();

        // Check the current moderator
        let res = query(
            deps.as_ref(),
            mock_env(),
            ContractQueryMsg::Transmuter(QueryMsg::GetModerator {}),
        )
        .unwrap();
        let moderator_response: GetModeratorResponse = from_json(res).unwrap();
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
        let moderator_response: GetModeratorResponse = from_json(res).unwrap();
        assert_eq!(moderator_response.moderator, new_moderator);
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
            moderator: "moderator".to_string(),
            alloyed_asset_subdenom: "usomoion".to_string(),
            alloyed_asset_normalization_factor: Uint128::one(),
        };

        instantiate(deps.as_mut(), mock_env(), mock_info(admin, &[]), init_msg).unwrap();

        // normal user can't register limiter
        let err = execute(
            deps.as_mut(),
            mock_env(),
            mock_info(user, &[]),
            ContractExecMsg::Transmuter(ExecMsg::RegisterLimiter {
                scope: Scope::Denom("uosmo".to_string()),
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
                scope: Scope::Denom("uosmo".to_string()),
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
                scope: Scope::Denom("uosmo".to_string()),
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
            attr("label", "1h"),
            attr("scope", "denom::uosmo"),
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
                scope: Scope::Denom("invalid_denom".to_string()),
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
        let limiters: ListLimitersResponse = from_json(res).unwrap();

        assert_eq!(
            limiters.limiters,
            vec![(
                (Scope::denom("uosmo").key(), String::from("1h")),
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
                scope: Scope::Denom("uosmo".to_string()),
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
            attr("label", "1w"),
            attr("scope", "denom::uosmo"),
            attr("limiter_type", "change_limiter"),
            attr("window_size", "604800000000"),
            attr("division_count", "5"),
            attr("boundary_offset", "0.01"),
        ];

        assert_eq!(res.attributes, attrs_1w);

        // Query the list of limiters
        let query_msg = ContractQueryMsg::Transmuter(QueryMsg::ListLimiters {});
        let res = query(deps.as_ref(), mock_env(), query_msg).unwrap();
        let limiters: ListLimitersResponse = from_json(res).unwrap();

        assert_eq!(
            limiters.limiters,
            vec![
                (
                    (Scope::denom("uosmo").key(), String::from("1h")),
                    Limiter::ChangeLimiter(
                        ChangeLimiter::new(window_config_1h, Decimal::percent(1)).unwrap()
                    )
                ),
                (
                    (Scope::denom("uosmo").key(), String::from("1w")),
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
                scope: Scope::Denom("uosmo".to_string()),
                label: "static".to_string(),
                limiter_params: LimiterParams::StaticLimiter {
                    upper_limit: Decimal::percent(60),
                },
            }),
        )
        .unwrap();

        let attrs = vec![
            attr("method", "register_limiter"),
            attr("label", "static"),
            attr("scope", "denom::uosmo"),
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
                scope: Scope::Denom("uosmo".to_string()),
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
                scope: Scope::Denom("uosmo".to_string()),
                label: "1h".to_string(),
            }),
        )
        .unwrap();

        let attrs = vec![
            attr("method", "deregister_limiter"),
            attr("scope", "denom::uosmo"),
            attr("label", "1h"),
        ];

        assert_eq!(res.attributes, attrs);

        // Query the list of limiters
        let query_msg = ContractQueryMsg::Transmuter(QueryMsg::ListLimiters {});
        let res = query(deps.as_ref(), mock_env(), query_msg).unwrap();
        let limiters: ListLimitersResponse = from_json(res).unwrap();

        assert_eq!(
            limiters.limiters,
            vec![
                (
                    (Scope::denom("uosmo").key(), String::from("1w")),
                    Limiter::ChangeLimiter(
                        ChangeLimiter::new(window_config_1w.clone(), Decimal::percent(1)).unwrap()
                    )
                ),
                (
                    (Scope::denom("uosmo").key(), String::from("static")),
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
                scope: Scope::Denom("uosmo".to_string()),
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
                scope: Scope::Denom("uosmo".to_string()),
                label: "1h".to_string(),
                boundary_offset: Decimal::zero(),
            }),
        )
        .unwrap_err();

        assert_eq!(
            err,
            ContractError::LimiterDoesNotExist {
                scope: Scope::denom("uosmo"),
                label: "1h".to_string()
            }
        );

        // set boundary offset by admin for existing limiter should work
        let res = execute(
            deps.as_mut(),
            mock_env(),
            mock_info(admin, &[]),
            ContractExecMsg::Transmuter(ExecMsg::SetChangeLimiterBoundaryOffset {
                scope: Scope::Denom("uosmo".to_string()),
                label: "1w".to_string(),
                boundary_offset: Decimal::percent(10),
            }),
        )
        .unwrap();

        let attrs = vec![
            attr("method", "set_change_limiter_boundary_offset"),
            attr("scope", "denom::uosmo"),
            attr("label", "1w"),
            attr("boundary_offset", "0.1"),
        ];

        assert_eq!(res.attributes, attrs);

        // Query the list of limiters
        let query_msg = ContractQueryMsg::Transmuter(QueryMsg::ListLimiters {});
        let res = query(deps.as_ref(), mock_env(), query_msg).unwrap();
        let limiters: ListLimitersResponse = from_json(res).unwrap();

        assert_eq!(
            limiters.limiters,
            vec![
                (
                    (Scope::denom("uosmo").key(), String::from("1w")),
                    Limiter::ChangeLimiter(
                        ChangeLimiter::new(window_config_1w.clone(), Decimal::percent(10)).unwrap()
                    )
                ),
                (
                    (Scope::denom("uosmo").key(), String::from("static")),
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
                scope: Scope::Denom("uosmo".to_string()),
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
                scope: Scope::Denom("uosmo".to_string()),
                label: "1h".to_string(),
                upper_limit: Decimal::percent(50),
            }),
        )
        .unwrap_err();

        assert_eq!(
            err,
            ContractError::LimiterDoesNotExist {
                scope: Scope::denom("uosmo"),
                label: "1h".to_string()
            }
        );

        // set upper limit by admin for change limiter should fail
        let err = execute(
            deps.as_mut(),
            mock_env(),
            mock_info(admin, &[]),
            ContractExecMsg::Transmuter(ExecMsg::SetStaticLimiterUpperLimit {
                scope: Scope::denom("uosmo"),
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
                scope: Scope::Denom("uosmo".to_string()),
                label: "static".to_string(),
                upper_limit: Decimal::percent(50),
            }),
        )
        .unwrap();

        let attrs = vec![
            attr("method", "set_static_limiter_upper_limit"),
            attr("scope", "denom::uosmo"),
            attr("label", "static"),
            attr("upper_limit", "0.5"),
        ];

        assert_eq!(res.attributes, attrs);

        // Query the list of limiters
        let query_msg = ContractQueryMsg::Transmuter(QueryMsg::ListLimiters {});
        let res = query(deps.as_ref(), mock_env(), query_msg).unwrap();
        let limiters: ListLimitersResponse = from_json(res).unwrap();

        assert_eq!(
            limiters.limiters,
            vec![
                (
                    (Scope::denom("uosmo").key(), String::from("1w")),
                    Limiter::ChangeLimiter(
                        ChangeLimiter::new(window_config_1w, Decimal::percent(10)).unwrap()
                    )
                ),
                (
                    (Scope::denom("uosmo").key(), String::from("static")),
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
            moderator: "moderator".to_string(),
            alloyed_asset_normalization_factor: Uint128::one(),
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
            uri: String::new(),
            uri_hash: String::new(),
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
            alloyed_asset_normalization_factor: Uint128::one(),
            moderator: "moderator".to_string(),
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
        } = from_json(
            query(
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
            alloyed_asset_normalization_factor: Uint128::one(),
            moderator: "moderator".to_string(),
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
            alloyed_asset_normalization_factor: Uint128::one(),
            moderator: "moderator".to_string(),
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
        let shares: GetSharesResponse = from_json(res).unwrap();
        assert_eq!(shares.shares.u128(), 2000u128);

        // Query the total shares
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::GetTotalShares {}),
        )
        .unwrap();
        let total_shares: GetTotalSharesResponse = from_json(res).unwrap();
        assert_eq!(total_shares.total_shares.u128(), 2000u128);

        // Query the total pool liquidity
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::GetTotalPoolLiquidity {}),
        )
        .unwrap();
        let total_pool_liquidity: GetTotalPoolLiquidityResponse = from_json(res).unwrap();
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

        let total_shares: GetTotalSharesResponse = from_json(res).unwrap();

        assert_eq!(total_shares.total_shares.u128(), 3000u128);

        // Query the total pool liquidity
        let res = query(
            deps.as_ref(),
            env,
            ContractQueryMsg::Transmuter(QueryMsg::GetTotalPoolLiquidity {}),
        )
        .unwrap();

        let total_pool_liquidity: GetTotalPoolLiquidityResponse = from_json(res).unwrap();

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
            alloyed_asset_normalization_factor: Uint128::one(),
            moderator: "moderator".to_string(),
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

        let share_denom: GetShareDenomResponse = from_json(res).unwrap();
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
            alloyed_asset_subdenom: "uosmoion".to_string(),
            alloyed_asset_normalization_factor: Uint128::one(),
            moderator: "moderator".to_string(),
        };
        let env = mock_env();
        let info = mock_info(admin, &[]);

        // Instantiate the contract.
        instantiate(deps.as_mut(), env.clone(), info, init_msg).unwrap();

        // Manually reply
        let alloyed_denom = "uosmoion";

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
                reason: "quote_asset_denom is not in swappable assets: must be one of [\"uion\", \"uosmo\", \"uosmoion\"] but got uatom".to_string()
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
                reason: "base_asset_denom is not in swappable assets: must be one of [\"uion\", \"uosmo\", \"uosmoion\"] but got uatom".to_string()
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

        let spot_price: SpotPriceResponse = from_json(res).unwrap();
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

        let spot_price: SpotPriceResponse = from_json(res).unwrap();
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

        let spot_price: SpotPriceResponse = from_json(res).unwrap();
        assert_eq!(spot_price.spot_price, Decimal::one());
    }

    #[test]
    fn test_spot_price_with_different_norm_factor() {
        let mut deps = mock_dependencies();

        // make denom has non-zero total supply
        deps.querier
            .update_balance("someone", vec![Coin::new(1, "tbtc"), Coin::new(1, "nbtc")]);

        let admin = "admin";
        let init_msg = InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig {
                    denom: "tbtc".to_string(),
                    normalization_factor: Uint128::from(1u128),
                },
                AssetConfig {
                    denom: "nbtc".to_string(),
                    normalization_factor: Uint128::from(100u128),
                },
            ],
            admin: Some(admin.to_string()),
            alloyed_asset_subdenom: "allbtc".to_string(),
            alloyed_asset_normalization_factor: Uint128::from(100u128),
            moderator: "moderator".to_string(),
        };
        let env = mock_env();
        let info = mock_info(admin, &[]);

        // Instantiate the contract.
        instantiate(deps.as_mut(), env.clone(), info, init_msg).unwrap();

        // Manually reply
        let alloyed_denom = "allbtc";

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

        // Test spot price with pool assets
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::SpotPrice {
                base_asset_denom: "nbtc".to_string(),
                quote_asset_denom: "tbtc".to_string(),
            }),
        )
        .unwrap();

        // tbtc/1 = nbtc/100
        // tbtc = 1nbtc/100
        let spot_price: SpotPriceResponse = from_json(res).unwrap();
        assert_eq!(spot_price.spot_price, Decimal::from_ratio(1u128, 100u128));

        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::SpotPrice {
                base_asset_denom: "tbtc".to_string(),
                quote_asset_denom: "nbtc".to_string(),
            }),
        )
        .unwrap();

        // nbtc/100 = tbtc/1
        // nbtc = 100tbtc
        let spot_price: SpotPriceResponse = from_json(res).unwrap();
        assert_eq!(spot_price.spot_price, Decimal::from_ratio(100u128, 1u128));

        // Test spot price with alloyed denom
        let res = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::SpotPrice {
                quote_asset_denom: "nbtc".to_string(),
                base_asset_denom: alloyed_denom.to_string(),
            }),
        )
        .unwrap();

        // nbtc/100 = allbtc/100
        // nbtc = 1allbtc
        let spot_price: SpotPriceResponse = from_json(res).unwrap();
        assert_eq!(spot_price.spot_price, Decimal::one());

        let res = query(
            deps.as_ref(),
            env,
            ContractQueryMsg::Transmuter(QueryMsg::SpotPrice {
                quote_asset_denom: alloyed_denom.to_string(),
                base_asset_denom: "tbtc".to_string(),
            }),
        )
        .unwrap();

        // allbtc/100 = tbtc/1
        // tbtc = 100allbtc
        let spot_price: SpotPriceResponse = from_json(res).unwrap();
        assert_eq!(spot_price.spot_price, Decimal::from_ratio(100u128, 1u128));
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
            alloyed_asset_normalization_factor: Uint128::one(),
            moderator: "moderator".to_string(),
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
                expected: Err(ContractError::SameDenomNotAllowed {
                    denom: "axlusdc".to_string(),
                }),
            },
            Case {
                name: String::from("same denom error (alloyed asset)"),
                token_in: Coin::new(1000, "alloyedusdc"),
                token_out_denom: "alloyedusdc".to_string(),
                swap_fee: Decimal::zero(),
                expected: Err(ContractError::SameDenomNotAllowed {
                    denom: "alloyedusdc".to_string(),
                }),
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
            .map(|value| from_json(value).unwrap());

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
            alloyed_asset_normalization_factor: Uint128::one(),
            moderator: "moderator".to_string(),
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
                expected: Err(ContractError::SameDenomNotAllowed {
                    denom: "axlusdc".to_string(),
                }),
            },
            Case {
                name: String::from("same denom error (alloyed asset)"),
                token_in_denom: "alloyedusdc".to_string(),
                token_out: Coin::new(1000, "alloyedusdc"),
                swap_fee: Decimal::zero(),
                expected: Err(ContractError::SameDenomNotAllowed {
                    denom: "alloyedusdc".to_string(),
                }),
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
            .map(|value| from_json(value).unwrap());

            assert_eq!(res, expected, "case: {}", name);
        }
    }

    #[test]
    fn test_rescale_normalization_factor() {
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
            alloyed_asset_normalization_factor: Uint128::from(100u128),
            moderator: "moderator".to_string(),
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

        // list asset configs
        let res: Result<ListAssetConfigsResponse, ContractError> = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::ListAssetConfigs {}),
        )
        .map(|value| from_json(value).unwrap());

        assert_eq!(
            res,
            Ok(ListAssetConfigsResponse {
                asset_configs: vec![
                    AssetConfig::from_denom_str("axlusdc"),
                    AssetConfig::from_denom_str("whusdc"),
                    AssetConfig {
                        denom: alloyed_denom.to_string(),
                        normalization_factor: Uint128::from(100u128),
                    }
                ]
            })
        );

        // scale up
        let rescale_msg = ContractExecMsg::Transmuter(ExecMsg::RescaleNormalizationFactor {
            numerator: Uint128::from(100u128),
            denominator: Uint128::one(),
        });

        execute(
            deps.as_mut(),
            env.clone(),
            mock_info(admin, &[]),
            rescale_msg,
        )
        .unwrap();

        // list asset configs
        let res: Result<ListAssetConfigsResponse, ContractError> = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::ListAssetConfigs {}),
        )
        .map(|value| from_json(value).unwrap());

        assert_eq!(
            res,
            Ok(ListAssetConfigsResponse {
                asset_configs: vec![
                    AssetConfig {
                        denom: "axlusdc".to_string(),
                        normalization_factor: Uint128::from(100u128),
                    },
                    AssetConfig {
                        denom: "whusdc".to_string(),
                        normalization_factor: Uint128::from(100u128),
                    },
                    AssetConfig {
                        denom: alloyed_denom.to_string(),
                        normalization_factor: Uint128::from(10000u128),
                    }
                ]
            })
        );

        // scale down
        let rescale_msg = ContractExecMsg::Transmuter(ExecMsg::RescaleNormalizationFactor {
            numerator: Uint128::one(),
            denominator: Uint128::from(100u128),
        });

        execute(
            deps.as_mut(),
            env.clone(),
            mock_info(admin, &[]),
            rescale_msg,
        )
        .unwrap();

        // list asset configs
        let res: Result<ListAssetConfigsResponse, ContractError> = query(
            deps.as_ref(),
            env.clone(),
            ContractQueryMsg::Transmuter(QueryMsg::ListAssetConfigs {}),
        )
        .map(|value| from_json(value).unwrap());

        assert_eq!(
            res,
            Ok(ListAssetConfigsResponse {
                asset_configs: vec![
                    AssetConfig {
                        denom: "axlusdc".to_string(),
                        normalization_factor: Uint128::from(1u128),
                    },
                    AssetConfig {
                        denom: "whusdc".to_string(),
                        normalization_factor: Uint128::from(1u128),
                    },
                    AssetConfig {
                        denom: alloyed_denom.to_string(),
                        normalization_factor: Uint128::from(100u128),
                    }
                ]
            })
        );
    }

    #[test]
    fn test_asset_group() {
        let mut deps = mock_dependencies();
        let env = mock_env();
        let admin = "admin";

        // Setup balance for each asset
        deps.querier.update_balance(
            env.contract.address.clone(),
            vec![
                Coin::new(1000000, "asset1"),
                Coin::new(1000000, "asset2"),
                Coin::new(1000000, "asset3"),
            ],
        );

        // Initialize the contract
        let instantiate_msg = InstantiateMsg {
            admin: Some(admin.to_string()),
            moderator: "moderator".to_string(),
            pool_asset_configs: vec![
                AssetConfig {
                    denom: "asset1".to_string(),
                    normalization_factor: Uint128::from(1000000u128),
                },
                AssetConfig {
                    denom: "asset2".to_string(),
                    normalization_factor: Uint128::from(1000000u128),
                },
                AssetConfig {
                    denom: "asset3".to_string(),
                    normalization_factor: Uint128::from(1000000u128),
                },
            ],
            alloyed_asset_subdenom: "alloyed".to_string(),
            alloyed_asset_normalization_factor: Uint128::from(1000000u128),
        };

        let info = mock_info(admin, &[]);
        instantiate(deps.as_mut(), env.clone(), info.clone(), instantiate_msg).unwrap();

        // Create asset group
        let create_asset_group_msg = ContractExecMsg::Transmuter(ExecMsg::CreateAssetGroup {
            label: "group1".to_string(),
            denoms: vec!["asset1".to_string(), "asset2".to_string()],
        });

        // Test non-admin trying to create asset group
        let non_admin_info = mock_info("non_admin", &[]);
        let non_admin_create_msg = ContractExecMsg::Transmuter(ExecMsg::CreateAssetGroup {
            label: "group1".to_string(),
            denoms: vec!["asset1".to_string(), "asset2".to_string()],
        });
        let err = execute(
            deps.as_mut(),
            env.clone(),
            non_admin_info,
            non_admin_create_msg,
        )
        .unwrap_err();
        assert_eq!(err, ContractError::Unauthorized {});

        // Test admin creating asset group
        let res = execute(deps.as_mut(), env.clone(), info, create_asset_group_msg).unwrap();

        assert_eq!(
            res.attributes,
            vec![
                attr("method", "create_asset_group"),
                attr("label", "group1"),
            ]
        );

        // List asset groups
        let list_asset_groups_msg = ContractQueryMsg::Transmuter(QueryMsg::ListAssetGroups {});
        let list_asset_groups_res: Result<ListAssetGroupsResponse, ContractError> =
            query(deps.as_ref(), env.clone(), list_asset_groups_msg)
                .map(|value| from_json(value).unwrap());

        assert_eq!(
            list_asset_groups_res,
            Ok(ListAssetGroupsResponse {
                asset_groups: BTreeMap::from([(
                    "group1".to_string(),
                    AssetGroup::new(vec!["asset1".to_string(), "asset2".to_string()]),
                )]),
            })
        );

        // Try setting limiter with non-existent group
        let register_limiter_msg = ContractExecMsg::Transmuter(ExecMsg::RegisterLimiter {
            label: "limiter1".to_string(),
            scope: Scope::asset_group("group2"),
            limiter_params: LimiterParams::ChangeLimiter {
                window_config: WindowConfig {
                    window_size: 86400u64.into(),
                    division_count: 10u64.into(),
                },
                boundary_offset: Decimal::percent(10),
            },
        });

        let register_limiter_info = mock_info("admin", &[]);
        let err = execute(
            deps.as_mut(),
            env.clone(),
            register_limiter_info.clone(),
            register_limiter_msg.clone(),
        )
        .unwrap_err();

        assert!(matches!(err, ContractError::AssetGroupNotFound { .. }));

        // Create group2
        let create_asset_group_msg2 = ContractExecMsg::Transmuter(ExecMsg::CreateAssetGroup {
            label: "group2".to_string(),
            denoms: vec!["asset3".to_string()],
        });

        let create_asset_group_info2 = mock_info("admin", &[]);
        let res2 = execute(
            deps.as_mut(),
            env.clone(),
            create_asset_group_info2,
            create_asset_group_msg2,
        )
        .unwrap();

        assert_eq!(
            res2.attributes,
            vec![
                attr("method", "create_asset_group"),
                attr("label", "group2"),
            ]
        );

        // Verify group2 was created
        let list_asset_groups_msg2 = ContractQueryMsg::Transmuter(QueryMsg::ListAssetGroups {});
        let list_asset_groups_res2: Result<ListAssetGroupsResponse, ContractError> =
            query(deps.as_ref(), env.clone(), list_asset_groups_msg2)
                .map(|value| from_json(value).unwrap());

        assert_eq!(
            list_asset_groups_res2,
            Ok(ListAssetGroupsResponse {
                asset_groups: BTreeMap::from([
                    (
                        "group1".to_string(),
                        AssetGroup::new(vec!["asset1".to_string(), "asset2".to_string()])
                    ),
                    (
                        "group2".to_string(),
                        AssetGroup::new(vec!["asset3".to_string()])
                    ),
                ]),
            })
        );

        // Try to register limiter for group2
        let res3 = execute(
            deps.as_mut(),
            env.clone(),
            register_limiter_info,
            register_limiter_msg,
        )
        .unwrap();

        assert_eq!(
            res3.attributes,
            vec![
                attr("method", "register_limiter"),
                attr("label", "limiter1"),
                attr("scope", "asset_group::group2"),
                attr("limiter_type", "change_limiter"),
                attr("window_size", "86400"),
                attr("division_count", "10"),
                attr("boundary_offset", "0.1"),
            ]
        );

        // Verify limiter was registered
        let list_limiters_msg = ContractQueryMsg::Transmuter(QueryMsg::ListLimiters {});
        let list_limiters_res: ListLimitersResponse =
            from_json(query(deps.as_ref(), env.clone(), list_limiters_msg).unwrap()).unwrap();

        assert_eq!(
            list_limiters_res.limiters,
            vec![(
                (
                    Scope::asset_group("group2").to_string(),
                    "limiter1".to_string()
                ),
                Limiter::ChangeLimiter(
                    ChangeLimiter::new(
                        WindowConfig {
                            window_size: 86400u64.into(),
                            division_count: 10u64.into(),
                        },
                        Decimal::percent(10),
                    )
                    .unwrap()
                )
            )]
        );

        // Try to create a group with a non-existing asset
        let create_invalid_group_msg = ContractExecMsg::Transmuter(ExecMsg::CreateAssetGroup {
            label: "invalid_group".to_string(),
            denoms: vec!["asset1".to_string(), "non_existing_asset".to_string()],
        });

        let admin_info = mock_info(admin, &[]);
        let err = execute(
            deps.as_mut(),
            env.clone(),
            admin_info,
            create_invalid_group_msg,
        )
        .unwrap_err();

        assert_eq!(
            err,
            ContractError::InvalidPoolAssetDenom {
                denom: "non_existing_asset".to_string()
            }
        );

        // Verify that the invalid group was not created
        let list_asset_groups_msg = ContractQueryMsg::Transmuter(QueryMsg::ListAssetGroups {});
        let list_asset_groups_res: ListAssetGroupsResponse =
            from_json(query(deps.as_ref(), env.clone(), list_asset_groups_msg).unwrap()).unwrap();

        assert_eq!(
            list_asset_groups_res.asset_groups,
            BTreeMap::from([
                (
                    "group1".to_string(),
                    AssetGroup::new(vec!["asset1".to_string(), "asset2".to_string()])
                ),
                (
                    "group2".to_string(),
                    AssetGroup::new(vec!["asset3".to_string()])
                ),
            ])
        );

        // Test removing an asset group
        let remove_group_msg = ContractExecMsg::Transmuter(ExecMsg::RemoveAssetGroup {
            label: "group2".to_string(),
        });

        // Try to remove the group with a non-admin account
        let non_admin_info = mock_info("non_admin", &[]);
        let err = execute(
            deps.as_mut(),
            env.clone(),
            non_admin_info,
            remove_group_msg.clone(),
        )
        .unwrap_err();

        assert_eq!(err, ContractError::Unauthorized {});

        // Remove the group with the admin account
        let admin_info = mock_info(admin, &[]);
        let res = execute(deps.as_mut(), env.clone(), admin_info, remove_group_msg).unwrap();

        assert_eq!(
            res.attributes,
            vec![
                attr("method", "remove_asset_group"),
                attr("label", "group2"),
            ]
        );

        // Verify that the group was removed
        let list_asset_groups_msg = ContractQueryMsg::Transmuter(QueryMsg::ListAssetGroups {});
        let list_asset_groups_res: ListAssetGroupsResponse =
            from_json(query(deps.as_ref(), env.clone(), list_asset_groups_msg).unwrap()).unwrap();

        assert_eq!(
            list_asset_groups_res.asset_groups,
            BTreeMap::from([(
                "group1".to_string(),
                AssetGroup::new(vec!["asset1".to_string(), "asset2".to_string()])
            )])
        );

        // Test that limiter1 is removed along with the asset group
        let list_limiters_msg = ContractQueryMsg::Transmuter(QueryMsg::ListLimiters {});
        let list_limiters_res: ListLimitersResponse =
            from_json(query(deps.as_ref(), env.clone(), list_limiters_msg).unwrap()).unwrap();

        // Check that limiter1 is not in the list of limiters
        assert_eq!(list_limiters_res.limiters, vec![]);

        // Test removing a non-existing asset group
        let remove_nonexistent_group_msg = ContractExecMsg::Transmuter(ExecMsg::RemoveAssetGroup {
            label: "non_existent_group".to_string(),
        });

        let admin_info = mock_info(admin, &[]);
        let err = execute(
            deps.as_mut(),
            env.clone(),
            admin_info,
            remove_nonexistent_group_msg,
        )
        .unwrap_err();

        assert_eq!(
            err,
            ContractError::AssetGroupNotFound {
                label: "non_existent_group".to_string()
            }
        );
    }

    #[test]
    fn test_mark_corrupted_scopes() {
        let mut deps = mock_dependencies();
        let env = mock_env();
        let admin = "admin";
        let moderator = "moderator";
        let user = "user";

        // Add supply for denoms using deps.querier.update_balance
        deps.querier.update_balance(
            env.contract.address.clone(),
            vec![Coin::new(1000000, "asset1"), Coin::new(2000000, "asset2")],
        );

        // Initialize the contract
        let init_msg = InstantiateMsg {
            admin: Some(admin.to_string()),
            moderator: moderator.to_string(),
            pool_asset_configs: vec![
                AssetConfig {
                    denom: "asset1".to_string(),
                    normalization_factor: Uint128::new(1),
                },
                AssetConfig {
                    denom: "asset2".to_string(),
                    normalization_factor: Uint128::new(1),
                },
            ],
            alloyed_asset_subdenom: "alloyed".to_string(),
            alloyed_asset_normalization_factor: Uint128::new(1),
        };
        let info = mock_info(admin, &[]);
        instantiate(deps.as_mut(), env.clone(), info, init_msg).unwrap();

        // Mark corrupted scopes
        let mark_corrupted_scopes_msg = ContractExecMsg::Transmuter(ExecMsg::MarkCorruptedScopes {
            scopes: vec![Scope::Denom("asset1".to_string())],
        });
        let moderator_info = mock_info(moderator, &[]);
        let res = execute(
            deps.as_mut(),
            env.clone(),
            moderator_info.clone(),
            mark_corrupted_scopes_msg,
        )
        .unwrap();

        // Check the response
        assert_eq!(
            res.attributes,
            vec![attr("method", "mark_corrupted_scopes")]
        );

        // Verify that the scope is marked as corrupted
        // Query the contract to get the corrupted denoms
        let query_msg = ContractQueryMsg::Transmuter(QueryMsg::GetCorruptedScopes {});
        let query_res: GetCorrruptedScopesResponse =
            from_json(&query(deps.as_ref(), env.clone(), query_msg).unwrap()).unwrap();

        // Check that "asset1" is in the corrupted denoms list
        assert_eq!(query_res.corrupted_scopes, vec![Scope::denom("asset1")]);

        // Try to mark corrupted scopes as a non-moderator (should fail)
        let user_info = mock_info(user, &[]);
        let unauthorized_mark_msg = ContractExecMsg::Transmuter(ExecMsg::MarkCorruptedScopes {
            scopes: vec![Scope::denom("asset2")],
        });
        let err =
            execute(deps.as_mut(), env.clone(), user_info, unauthorized_mark_msg).unwrap_err();
        assert_eq!(err, ContractError::Unauthorized {});

        // Test create_asset_group
        let create_asset_group_msg = ContractExecMsg::Transmuter(ExecMsg::CreateAssetGroup {
            label: "group1".to_string(),
            denoms: vec!["asset1".to_string(), "asset2".to_string()],
        });
        let admin_info = mock_info(admin, &[]);
        let res = execute(
            deps.as_mut(),
            env.clone(),
            admin_info.clone(),
            create_asset_group_msg,
        )
        .unwrap();

        // Check the response
        assert_eq!(
            res.attributes,
            vec![
                attr("method", "create_asset_group"),
                attr("label", "group1"),
            ]
        );

        // Test mark_asset_group_as_corrupted
        let moderator_info = mock_info(moderator, &[]);
        let mark_group_corrupted_msg = ContractExecMsg::Transmuter(ExecMsg::MarkCorruptedScopes {
            scopes: vec![Scope::asset_group("group1")],
        });
        let res = execute(
            deps.as_mut(),
            env.clone(),
            moderator_info.clone(),
            mark_group_corrupted_msg,
        )
        .unwrap();

        // Check the response
        assert_eq!(
            res.attributes,
            vec![attr("method", "mark_corrupted_scopes"),]
        );

        // Verify that the asset group is marked as corrupted
        let query_msg = ContractQueryMsg::Transmuter(QueryMsg::GetCorruptedScopes {});
        let query_res: GetCorrruptedScopesResponse =
            from_json(&query(deps.as_ref(), env.clone(), query_msg).unwrap()).unwrap();

        // Check that both "asset1" and "asset2" are in the corrupted scopes list
        assert_eq!(
            query_res.corrupted_scopes,
            vec![Scope::denom("asset1"), Scope::asset_group("group1")]
        );

        // Test unmark_corrupted_scopes for the asset group
        let unmark_group_corrupted_msg =
            ContractExecMsg::Transmuter(ExecMsg::UnmarkCorruptedScopes {
                scopes: vec![Scope::asset_group("group1")],
            });
        let res = execute(
            deps.as_mut(),
            env.clone(),
            moderator_info.clone(),
            unmark_group_corrupted_msg,
        )
        .unwrap();

        // Check the response
        assert_eq!(
            res.attributes,
            vec![attr("method", "unmark_corrupted_scopes"),]
        );

        // Verify that the asset group is no longer marked as corrupted
        let query_msg = ContractQueryMsg::Transmuter(QueryMsg::GetCorruptedScopes {});
        let query_res: GetCorrruptedScopesResponse =
            from_json(&query(deps.as_ref(), env.clone(), query_msg).unwrap()).unwrap();

        // Check that the asset group is no longer in the corrupted scopes list
        assert_eq!(query_res.corrupted_scopes, vec![Scope::denom("asset1")]);
    }
}
