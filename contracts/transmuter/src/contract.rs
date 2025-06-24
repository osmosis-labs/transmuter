use crate::{corruptable::Corruptable, rebalancer::Rebalancer, scope::Scope};
use std::{collections::BTreeMap, iter};

use crate::{
    alloyed_asset::AlloyedAsset,
    asset::{Asset, AssetConfig},
    ensure_admin_authority, ensure_moderator_authority,
    error::{non_empty_input_required, nonpayable, ContractError},
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

use sylvia::{contract, ctx};
use transmuter_math::rebalancing::config::RebalancingConfig;

/// version info for migration
pub const CONTRACT_NAME: &str = "crates.io:transmuter";
pub const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

const CREATE_ALLOYED_DENOM_REPLY_ID: u64 = 1;

/// Prefix for alloyed asset denom
const ALLOYED_PREFIX: &str = "alloyed";

pub struct Transmuter {
    pub(crate) active_status: Item<bool>,
    pub(crate) pool: Item<TransmuterPool>,
    pub(crate) alloyed_asset: AlloyedAsset,
    pub(crate) role: Role,
    pub(crate) rebalancer: Rebalancer,
}

pub mod key {
    pub const ACTIVE_STATUS: &str = "active_status";
    pub const POOL: &str = "pool";
    pub const ALLOYED_ASSET_DENOM: &str = "alloyed_denom";
    pub const ALLOYED_ASSET_NORMALIZATION_FACTOR: &str = "alloyed_asset_normalization_factor";
    pub const ADMIN: &str = "admin";
    pub const MODERATOR: &str = "moderator";
    pub const REBALANCER: &str = "rebalancer"; // TODO: migrate data from limiters
}

#[contract]
#[sv::error(ContractError)]
impl Transmuter {
    /// Create a Transmuter instance.
    pub const fn new() -> Self {
        Self {
            active_status: Item::new(key::ACTIVE_STATUS),
            pool: Item::new(key::POOL),
            alloyed_asset: AlloyedAsset::new(
                key::ALLOYED_ASSET_DENOM,
                key::ALLOYED_ASSET_NORMALIZATION_FACTOR,
            ),
            role: Role::new(key::ADMIN, key::MODERATOR),
            rebalancer: Rebalancer::new(key::REBALANCER),
        }
    }

    /// Instantiate the contract.
    #[sv::msg(instantiate)]
    pub fn instantiate(
        &self,
        ctx::InstantiateCtx {
            deps, env, info, ..
        }: ctx::InstantiateCtx,
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
        ctx::ExecCtx { deps, info, .. }: ctx::ExecCtx,
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
        ctx::ExecCtx { deps, info, .. }: ctx::ExecCtx,
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

        Ok(Response::new().add_attribute("method", "add_new_assets"))
    }

    #[sv::msg(exec)]
    fn create_asset_group(
        &self,
        ctx::ExecCtx { deps, info, .. }: ctx::ExecCtx,
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
        ctx::ExecCtx { deps, info, .. }: ctx::ExecCtx,
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

        // remove all rebalancing configs for asset group
        let configs = self
            .rebalancer
            .list_by_scope(deps.storage, &Scope::AssetGroup(label.clone()))?;

        for (config_label, _) in configs {
            self.rebalancer.unchecked_remove_config(
                deps.storage,
                Scope::AssetGroup(label.clone()),
                &config_label,
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
        ctx::ExecCtx { deps, info, .. }: ctx::ExecCtx,
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
        ctx::ExecCtx { deps, info, .. }: ctx::ExecCtx,
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
    fn add_rebalancing_config(
        &self,
        ctx::ExecCtx { deps, info, .. }: ctx::ExecCtx,
        scope: Scope,
        label: String,
        rebalancing_config: RebalancingConfig,
    ) -> Result<Response, ContractError> {
        nonpayable(&info.funds)?;

        // only admin can add rebalancing config
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
            ("method", "add_rebalancing_config"),
            ("label", &label),
            ("scope", &scope_key),
        ];

        let RebalancingConfig {
            ideal_upper,
            ideal_lower,
            critical_upper,
            critical_lower,
            limit,
            adjustment_rate_strained,
            adjustment_rate_critical,
        } = rebalancing_config;

        let rebalancing_config_attrs = vec![
            (String::from("ideal_upper"), ideal_upper.to_string()),
            (String::from("ideal_lower"), ideal_lower.to_string()),
            (String::from("critical_upper"), critical_upper.to_string()),
            (String::from("critical_lower"), critical_lower.to_string()),
            (String::from("limit"), limit.to_string()),
            (
                String::from("adjustment_rate_strained"),
                adjustment_rate_strained.to_string(),
            ),
            (
                String::from("adjustment_rate_critical"),
                adjustment_rate_critical.to_string(),
            ),
        ];

        self.rebalancer
            .add_config(deps.storage, scope, &label, rebalancing_config)?;

        Ok(Response::new()
            .add_attributes(base_attrs)
            .add_attributes(rebalancing_config_attrs))
    }

    #[sv::msg(exec)]
    fn remove_rebalancing_config(
        &self,
        ctx::ExecCtx { deps, info, .. }: ctx::ExecCtx,
        scope: Scope,
        label: String,
    ) -> Result<Response, ContractError> {
        nonpayable(&info.funds)?;

        // only admin can remove rebalancing config
        ensure_admin_authority!(info.sender, self.role.admin, deps.as_ref());

        let scope_key = scope.key();
        let attrs = vec![
            ("method", "remove_rebalancing_config"),
            ("scope", &scope_key),
            ("label", &label),
        ];

        // remove rebalancing config
        self.rebalancer.remove_config(deps.storage, scope, &label)?;

        Ok(Response::new().add_attributes(attrs))
    }

    #[sv::msg(exec)]
    fn update_rebalancing_config(
        &self,
        ctx::ExecCtx { deps, info, .. }: ctx::ExecCtx,
        scope: Scope,
        label: String,
        rebalancing_config: RebalancingConfig,
    ) -> Result<Response, ContractError> {
        nonpayable(&info.funds)?;

        // only admin can update rebalancing config
        ensure_admin_authority!(info.sender, self.role.admin, deps.as_ref());

        let RebalancingConfig {
            ideal_upper,
            ideal_lower,
            critical_upper,
            critical_lower,
            limit,
            adjustment_rate_strained,
            adjustment_rate_critical,
        } = rebalancing_config;
        let scope_key = scope.key();

        // update rebalancing config
        self.rebalancer
            .update_config(deps.storage, scope, &label, &rebalancing_config)?;

        let attrs = vec![
            (
                String::from("method"),
                String::from("update_rebalancing_config"),
            ),
            (String::from("scope"), scope_key),
            (String::from("label"), label),
            (String::from("ideal_upper"), ideal_upper.to_string()),
            (String::from("ideal_lower"), ideal_lower.to_string()),
            (String::from("critical_upper"), critical_upper.to_string()),
            (String::from("critical_lower"), critical_lower.to_string()),
            (String::from("limit"), limit.to_string()),
            (
                String::from("adjustment_rate_strained"),
                adjustment_rate_strained.to_string(),
            ),
            (
                String::from("adjustment_rate_critical"),
                adjustment_rate_critical.to_string(),
            ),
        ];

        Ok(Response::new().add_attributes(attrs))
    }

    #[sv::msg(exec)]
    pub fn set_alloyed_denom_metadata(
        &self,
        ctx::ExecCtx {
            deps, env, info, ..
        }: ctx::ExecCtx,
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
        ctx::ExecCtx { deps, info, .. }: ctx::ExecCtx,
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
        ctx::ExecCtx {
            deps, env, info, ..
        }: ctx::ExecCtx,
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
        ctx::ExecCtx {
            deps, env, info, ..
        }: ctx::ExecCtx,
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
        ctx::QueryCtx { deps, .. }: ctx::QueryCtx,
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
    fn list_rebalancing_configs(
        &self,
        ctx::QueryCtx { deps, .. }: ctx::QueryCtx,
    ) -> Result<ListRebalancingConfigResponse, ContractError> {
        let rebalancing_configs = self.rebalancer.list_configs(deps.storage)?;

        Ok(ListRebalancingConfigResponse {
            rebalancing_configs,
        })
    }

    #[sv::msg(query)]
    fn list_asset_groups(
        &self,
        ctx::QueryCtx { deps, .. }: ctx::QueryCtx,
    ) -> Result<ListAssetGroupsResponse, ContractError> {
        let pool = self.pool.load(deps.storage)?;

        Ok(ListAssetGroupsResponse {
            asset_groups: pool.asset_groups,
        })
    }

    #[sv::msg(query)]
    pub fn get_shares(
        &self,
        ctx::QueryCtx { deps, .. }: ctx::QueryCtx,
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
        ctx::QueryCtx { deps, .. }: ctx::QueryCtx,
    ) -> Result<GetShareDenomResponse, ContractError> {
        Ok(GetShareDenomResponse {
            share_denom: self.alloyed_asset.get_alloyed_denom(deps.storage)?,
        })
    }

    #[sv::msg(query)]
    pub(crate) fn get_swap_fee(
        &self,
        _ctx: ctx::QueryCtx,
    ) -> Result<GetSwapFeeResponse, ContractError> {
        Ok(GetSwapFeeResponse { swap_fee: SWAP_FEE })
    }

    #[sv::msg(query)]
    pub(crate) fn is_active(
        &self,
        ctx::QueryCtx { deps, .. }: ctx::QueryCtx,
    ) -> Result<IsActiveResponse, ContractError> {
        Ok(IsActiveResponse {
            is_active: self.active_status.load(deps.storage)?,
        })
    }

    #[sv::msg(query)]
    pub(crate) fn get_total_shares(
        &self,
        ctx::QueryCtx { deps, .. }: ctx::QueryCtx,
    ) -> Result<GetTotalSharesResponse, ContractError> {
        let total_shares = self.alloyed_asset.get_total_supply(deps)?;
        Ok(GetTotalSharesResponse { total_shares })
    }

    #[sv::msg(query)]
    pub(crate) fn get_total_pool_liquidity(
        &self,
        ctx::QueryCtx { deps, .. }: ctx::QueryCtx,
    ) -> Result<GetTotalPoolLiquidityResponse, ContractError> {
        let pool = self.pool.load(deps.storage)?;

        Ok(GetTotalPoolLiquidityResponse {
            total_pool_liquidity: pool.pool_assets.iter().map(Asset::to_coin).collect(),
        })
    }

    #[sv::msg(query)]
    pub(crate) fn spot_price(
        &self,
        ctx::QueryCtx { deps, .. }: ctx::QueryCtx,
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
        ctx::QueryCtx { deps, .. }: ctx::QueryCtx,
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
        ctx::QueryCtx { deps, .. }: ctx::QueryCtx,
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
        ctx::QueryCtx { deps, .. }: ctx::QueryCtx,
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
        ctx::ExecCtx { deps, info, .. }: ctx::ExecCtx,
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
        ctx::ExecCtx { deps, info, .. }: ctx::ExecCtx,
    ) -> Result<Response, ContractError> {
        self.role.admin.cancel_transfer(deps, info.sender)?;

        Ok(Response::new().add_attribute("method", "cancel_admin_transfer"))
    }

    #[sv::msg(exec)]
    pub fn reject_admin_transfer(
        &self,
        ctx::ExecCtx { deps, info, .. }: ctx::ExecCtx,
    ) -> Result<Response, ContractError> {
        self.role.admin.reject_transfer(deps, info.sender)?;

        Ok(Response::new().add_attribute("method", "reject_admin_transfer"))
    }

    #[sv::msg(exec)]
    pub fn claim_admin(
        &self,
        ctx::ExecCtx { deps, info, .. }: ctx::ExecCtx,
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
        ctx::QueryCtx { deps, .. }: ctx::QueryCtx,
    ) -> Result<GetAdminResponse, ContractError> {
        Ok(GetAdminResponse {
            admin: self.role.admin.current(deps)?,
        })
    }

    #[sv::msg(query)]
    fn get_admin_candidate(
        &self,
        ctx::QueryCtx { deps, .. }: ctx::QueryCtx,
    ) -> Result<GetAdminCandidateResponse, ContractError> {
        Ok(GetAdminCandidateResponse {
            admin_candidate: self.role.admin.candidate(deps)?,
        })
    }

    // -- moderator --
    #[sv::msg(exec)]
    pub fn assign_moderator(
        &self,
        ctx::ExecCtx { deps, info, .. }: ctx::ExecCtx,
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
        ctx::QueryCtx { deps, .. }: ctx::QueryCtx,
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
pub struct ListRebalancingConfigResponse {
    pub rebalancing_configs: Vec<((String, String), RebalancingConfig)>,
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
    use crate::sudo::SudoMsg;
    use crate::*;

    use cosmwasm_std::testing::{message_info, mock_dependencies, mock_env};
    use cosmwasm_std::{
        attr, coin, from_json, BankMsg, Binary, MsgResponse, Storage, SubMsgResponse, SubMsgResult,
    };
    use osmosis_std::types::osmosis::tokenfactory::v1beta1::MsgBurn;

    #[test]
    fn test_invalid_subdenom() {
        let mut deps = mock_dependencies();

        // make denom has non-zero total supply
        deps.querier
            .bank
            .update_balance("someone", vec![coin(1, "tbtc"), coin(1, "nbtc")]);

        let admin = deps.api.addr_make("admin");
        let moderator = deps.api.addr_make("moderator");
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
        let info = message_info(&admin, &[]);

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

        let someone = deps.api.addr_make("someone");

        // make denom has non-zero total supply
        deps.querier.bank.update_balance(
            someone,
            vec![
                coin(1, "uosmo"),
                coin(1, "uion"),
                coin(1, "new_asset1"),
                coin(1, "new_asset2"),
            ],
        );

        let admin = deps.api.addr_make("admin");
        let moderator = deps.api.addr_make("moderator");
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
        let info = message_info(&admin, &[]);

        // Instantiate the contract.
        instantiate(deps.as_mut(), env.clone(), info.clone(), init_msg).unwrap();

        // Manually reply
        let alloyed_denom = "usomoion";

        let msg_create_denom_response = MsgCreateDenomResponse {
            new_token_denom: alloyed_denom.to_string(),
        };

        reply(
            deps.as_mut(),
            env.clone(),
            Reply {
                id: 1,
                result: SubMsgResult::Ok(
                    #[allow(deprecated)]
                    SubMsgResponse {
                        events: vec![],
                        data: Some(msg_create_denom_response.clone().into()), // DEPRECATED
                        msg_responses: vec![MsgResponse {
                            type_url: MsgCreateDenomResponse::TYPE_URL.to_string(),
                            value: msg_create_denom_response.into(),
                        }],
                    },
                ),
                payload: Binary::new(vec![]),
                gas_used: 0,
            },
        )
        .unwrap();

        // join pool
        let someone = deps.api.addr_make("someone");
        let info = message_info(
            &someone,
            &[coin(1000000000, "uosmo"), coin(1000000000, "uion")],
        );
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        execute(deps.as_mut(), env.clone(), info.clone(), join_pool_msg).unwrap();

        // Create asset group
        let create_asset_group_msg = ContractExecMsg::Transmuter(ExecMsg::CreateAssetGroup {
            label: "group1".to_string(),
            denoms: vec!["uosmo".to_string(), "uion".to_string()],
        });

        let info = message_info(&admin, &[]);
        execute(
            deps.as_mut(),
            env.clone(),
            info.clone(),
            create_asset_group_msg,
        )
        .unwrap();

        // Add rebalancing config for individual denoms only (not the asset group to avoid conflicts)
        let config = RebalancingConfig::limit_only(Decimal::percent(60)).unwrap();

        let info = message_info(&admin, &[]);
        for denom in ["uosmo", "uion"] {
            let add_rebalancing_config_msg =
                ContractExecMsg::Transmuter(ExecMsg::AddRebalancingConfig {
                    scope: Scope::Denom(denom.to_string()),
                    label: "static_config".to_string(),
                    rebalancing_config: config.clone(),
                });

            execute(
                deps.as_mut(),
                env.clone(),
                info.clone(),
                add_rebalancing_config_msg,
            )
            .unwrap();
        }

        // join pool a bit more to test the limit (small amounts to stay within limits)
        let someone = deps.api.addr_make("someone");
        let info = message_info(&someone, &[coin(100, "uosmo"), coin(100, "uion")]);
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        execute(deps.as_mut(), env.clone(), info.clone(), join_pool_msg).unwrap();

        let info = message_info(&someone, &[coin(50, "uosmo"), coin(50, "uion")]);
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        execute(deps.as_mut(), env.clone(), info.clone(), join_pool_msg).unwrap();

        // Add new assets

        // Attempt to add assets with invalid denom
        let admin = deps.api.addr_make("admin");
        let info = message_info(&admin, &[]);
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

        let non_admin = deps.api.addr_make("non_admin");
        // Attempt to add assets by non-admin
        let non_admin_info = message_info(&non_admin, &[]);
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

        // successful asset addition
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
        } = from_json(res).unwrap();

        assert_eq!(
            total_pool_liquidity,
            vec![
                coin(1000000150, "uosmo"),
                coin(1000000150, "uion"),
                coin(0, "new_asset1"),
                coin(0, "new_asset2"),
            ]
        );
    }

    #[test]
    fn test_corrupted_assets() {
        let mut deps = mock_dependencies();

        let someone = deps.api.addr_make("someone");
        // make denom has non-zero total supply
        deps.querier.bank.update_balance(
            &someone,
            vec![
                coin(1, "wbtc"),
                coin(1, "tbtc"),
                coin(1, "nbtc"),
                coin(1, "stbtc"),
            ],
        );

        let admin = deps.api.addr_make("admin");
        let moderator = deps.api.addr_make("moderator");
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
        let info = message_info(&admin, &[]);
        instantiate(deps.as_mut(), env.clone(), info.clone(), init_msg).unwrap();

        // Manually reply
        let res = reply(
            deps.as_mut(),
            env.clone(),
            reply_create_denom_response(alloyed_subdenom),
        )
        .unwrap();

        let alloyed_token_denom_kv = res.attributes[0].clone();
        assert_eq!(alloyed_token_denom_kv.key, "alloyed_denom");
        let alloyed_denom = alloyed_token_denom_kv.value;

        // set rebalancing configs
        let config = RebalancingConfig::limit_only(Decimal::percent(30)).unwrap();

        // Mark corrupted assets by non-moderator
        let info = message_info(&someone, &[]);
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
                coin(0, "wbtc"),
                coin(0, "tbtc"),
                coin(0, "nbtc"),
                coin(0, "stbtc"),
            ]
        );

        // provide some liquidity
        let liquidity = vec![
            coin(1_000_000_000_000, "wbtc"),
            coin(1_000_000_000_000, "tbtc"),
            coin(1_000_000_000_000, "nbtc"),
            coin(1_000_000_000_000, "stbtc"),
        ];
        deps.querier
            .bank
            .update_balance("someone", liquidity.clone());

        let info = message_info(&someone, &liquidity);
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});

        execute(deps.as_mut(), env.clone(), info.clone(), join_pool_msg).unwrap();

        // set rebalancing configs
        let config = RebalancingConfig::limit_only(Decimal::percent(30)).unwrap();

        let info = message_info(&admin, &[]);
        // set rebalancing configs
        for denom in ["wbtc", "tbtc", "nbtc", "stbtc"] {
            let add_rebalancing_config_msg =
                ContractExecMsg::Transmuter(ExecMsg::AddRebalancingConfig {
                    scope: Scope::Denom(denom.to_string()),
                    label: "static_config".to_string(),
                    rebalancing_config: config.clone(),
                });
            execute(
                deps.as_mut(),
                env.clone(),
                info.clone(),
                add_rebalancing_config_msg,
            )
            .unwrap();
        }

        // Create asset group
        let create_asset_group_msg = ContractExecMsg::Transmuter(ExecMsg::CreateAssetGroup {
            label: "btc_group1".to_string(),
            denoms: vec!["nbtc".to_string(), "stbtc".to_string()],
        });

        execute(
            deps.as_mut(),
            env.clone(),
            info.clone(),
            create_asset_group_msg,
        )
        .unwrap();

        deps.querier
            .bank
            .update_balance(&someone, vec![coin(1_000, alloyed_denom.clone())]);
        let exit_pool_msg = ContractExecMsg::Transmuter(ExecMsg::ExitPool {
            tokens_out: vec![coin(1_000, "nbtc")],
        });

        // Use empty funds for nonpayable execute
        let info = message_info(&someone, &[]);
        execute(deps.as_mut(), env.clone(), info.clone(), exit_pool_msg).unwrap();

        // Mark corrupted assets by moderator
        let corrupted_scopes = vec![Scope::denom("wbtc"), Scope::denom("tbtc")];
        let mark_corrupted_assets_msg = ContractExecMsg::Transmuter(ExecMsg::MarkCorruptedScopes {
            scopes: corrupted_scopes.clone(),
        });

        let info = message_info(&moderator, &[]);
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
                coin(1_000_000_000_000, "wbtc"),
                coin(1_000_000_000_000, "tbtc"),
                coin(999_999_999_000, "nbtc"),
                coin(1_000_000_000_000, "stbtc"),
            ]
        );

        deps.querier
            .bank
            .update_balance(&someone, vec![coin(4, alloyed_denom.clone())]);
        let exit_pool_msg = ContractExecMsg::Transmuter(ExecMsg::ExitPool {
            tokens_out: vec![
                coin(1, "wbtc"),
                coin(1, "tbtc"),
                coin(1, "nbtc"),
                coin(1, "stbtc"),
            ],
        });
        let info = message_info(&someone, &[]);
        execute(deps.as_mut(), env.clone(), info.clone(), exit_pool_msg).unwrap();

        deps.querier
            .bank
            .update_balance("someone", vec![coin(4, alloyed_denom.clone())]);
        let exit_pool_msg = ContractExecMsg::Transmuter(ExecMsg::ExitPool {
            tokens_out: vec![
                coin(1, "wbtc"),
                coin(1, "tbtc"),
                coin(1, "nbtc"),
                coin(1, "stbtc"),
            ],
        });
        let info = message_info(&someone, &[]);
        execute(deps.as_mut(), env.clone(), info.clone(), exit_pool_msg).unwrap();

        for scope in corrupted_scopes {
            let expected_err = ContractError::CorruptedScopeRelativelyIncreased {
                scope: scope.clone(),
            };

            let denom = match scope {
                Scope::Denom(denom) => denom,
                _ => unreachable!(),
            };

            // join with corrupted denom should fail
            let user = deps.api.addr_make("user");
            let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
            let err = execute(
                deps.as_mut(),
                env.clone(),
                message_info(&user, &[coin(1000, denom.clone())]),
                join_pool_msg,
            )
            .unwrap_err();
            assert_eq!(expected_err, err);

            let mock_sender = deps.api.addr_make("mock_sender");
            // swap exact in with corrupted denom as token in should fail
            let swap_msg = SudoMsg::SwapExactAmountIn {
                token_in: coin(1000, denom.clone()),
                swap_fee: Decimal::zero(),
                sender: mock_sender.to_string(),
                token_out_denom: "nbtc".to_string(),
                token_out_min_amount: Uint128::new(500),
            };

            let err = sudo(deps.as_mut(), env.clone(), swap_msg).unwrap_err();
            assert_eq!(expected_err, err);

            // swap exact in with corrupted denom as token out should be ok since it decreases the corrupted asset
            let swap_msg = SudoMsg::SwapExactAmountIn {
                token_in: coin(1000, "nbtc"),
                swap_fee: Decimal::zero(),
                sender: mock_sender.to_string(),
                token_out_denom: denom.clone(),
                token_out_min_amount: Uint128::new(500),
            };

            sudo(deps.as_mut(), env.clone(), swap_msg).unwrap();

            // swap exact out with corrupted denom as token out should be ok since it decreases the corrupted asset
            let swap_msg = SudoMsg::SwapExactAmountOut {
                sender: mock_sender.to_string(),
                token_out: coin(500, denom.clone()),
                swap_fee: Decimal::zero(),
                token_in_denom: "nbtc".to_string(),
                token_in_max_amount: Uint128::new(1000),
            };

            sudo(deps.as_mut(), env.clone(), swap_msg).unwrap();

            // swap exact out with corrupted denom as token in should fail
            let swap_msg = SudoMsg::SwapExactAmountOut {
                sender: mock_sender.to_string(),
                token_out: coin(500, "nbtc"),
                swap_fee: Decimal::zero(),
                token_in_denom: denom.clone(),
                token_in_max_amount: Uint128::new(1000),
            };

            let err = sudo(deps.as_mut(), env.clone(), swap_msg).unwrap_err();
            assert_eq!(expected_err, err);

            // exit with by any denom requires corrupted denom to not increase in weight
            // (this case increase other remaining corrupted denom weight)
            deps.querier
                .bank
                .update_balance(&someone, vec![coin(4_000_000_000, alloyed_denom.clone())]);

            let exit_pool_msg = ContractExecMsg::Transmuter(ExecMsg::ExitPool {
                tokens_out: vec![coin(1_000_000_000, "stbtc")],
            });

            let info = message_info(&someone, &[]);

            // this causes all corrupted denoms to be increased in weight
            let err = execute(deps.as_mut(), env.clone(), info, exit_pool_msg).unwrap_err();
            assert!(matches!(
                err,
                ContractError::CorruptedScopeRelativelyIncreased { .. }
            ));

            let exit_pool_msg = ContractExecMsg::Transmuter(ExecMsg::ExitPool {
                tokens_out: vec![
                    coin(1_000_000_000, "nbtc"),
                    coin(1_000_000_000, denom.clone()),
                ],
            });

            let info = message_info(&someone, &[]);

            // this causes other corrupted denom to be increased relatively
            let err = execute(deps.as_mut(), env.clone(), info, exit_pool_msg).unwrap_err();
            assert!(matches!(
                err,
                ContractError::CorruptedScopeRelativelyIncreased { .. }
            ));
        }

        // exit with corrupted denom requires all corrupted denom exit with the same value
        deps.querier
            .bank
            .update_balance(&someone, vec![coin(4_000_000_000, alloyed_denom.clone())]);
        let info = message_info(&someone, &[]);
        let exit_pool_msg = ContractExecMsg::Transmuter(ExecMsg::ExitPool {
            tokens_out: vec![
                coin(2_000_000_000, "nbtc"),
                coin(1_000_000_000, "wbtc"),
                coin(1_000_000_000, "tbtc"),
            ],
        });
        execute(deps.as_mut(), env.clone(), info, exit_pool_msg).unwrap();

        // force redeem corrupted assets

        deps.querier.bank.update_balance(
            &someone,
            vec![coin(1_000_000_000_000, alloyed_denom.clone())],
        );
        let all_nbtc = total_liquidity_of("nbtc", &deps.storage);
        let force_redeem_corrupted_assets_msg = ContractExecMsg::Transmuter(ExecMsg::ExitPool {
            tokens_out: vec![all_nbtc],
        });

        let info = message_info(&someone, &[]);
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

        deps.querier.bank.update_balance(
            &someone,
            vec![coin(1_000_000_000_000, alloyed_denom.clone())],
        );

        let info = message_info(&someone, &[]);
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
                coin(998999998498, "tbtc"),
                coin(998000001998, "nbtc"),
                coin(999999999998, "stbtc"),
            ]
        );

        assert_eq!(
            Transmuter::new()
                .rebalancer
                .list_by_scope(&deps.storage, &Scope::denom("wbtc"))
                .unwrap(),
            vec![]
        );

        // try unmark nbtc should fail
        let unmark_corrupted_assets_msg =
            ContractExecMsg::Transmuter(ExecMsg::UnmarkCorruptedScopes {
                scopes: vec![Scope::denom("nbtc")],
            });

        let info = message_info(&moderator, &[]);
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

        let info = message_info(&someone, &[]);
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

        let info = message_info(&moderator, &[]);
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
                coin(998999998498, "tbtc"),
                coin(998000001998, "nbtc"),
                coin(999999999998, "stbtc"),
            ]
        );

        // still has all the rebalancing configs
        assert_eq!(
            Transmuter::new()
                .rebalancer
                .list_by_scope(&deps.storage, &Scope::denom("tbtc"))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn test_corrupted_asset_group() {
        let mut deps = mock_dependencies();
        let admin = deps.api.addr_make("admin");
        let user = deps.api.addr_make("user");
        let moderator = deps.api.addr_make("moderator");

        let info = message_info(&admin, &[]);

        deps.querier.bank.update_balance(
            &admin,
            vec![
                coin(1_000_000_000_000, "tbtc"),
                coin(1_000_000_000_000, "nbtc"),
                coin(1_000_000_000_000, "stbtc"),
            ],
        );

        let env = mock_env();

        // Initialize contract with asset group
        let init_msg = InstantiateMsg {
            admin: Some(admin.to_string()),
            moderator: moderator.to_string(),
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("tbtc"),
                AssetConfig::from_denom_str("nbtc"),
                AssetConfig::from_denom_str("stbtc"),
            ],
            alloyed_asset_subdenom: "btc".to_string(),
            alloyed_asset_normalization_factor: Uint128::one(),
        };

        instantiate(deps.as_mut(), env.clone(), info.clone(), init_msg).unwrap();

        // Manually reply
        let res = reply(
            deps.as_mut(),
            env.clone(),
            reply_create_denom_response("btc"),
        )
        .unwrap();

        let alloyed_denom = res
            .attributes
            .into_iter()
            .find(|attr| attr.key == "alloyed_denom")
            .unwrap()
            .value;

        deps.querier
            .bank
            .update_balance(&user, vec![coin(3_000_000_000_000, alloyed_denom.clone())]);

        // Create asset group
        let create_group_msg = ContractExecMsg::Transmuter(ExecMsg::CreateAssetGroup {
            label: "group1".to_string(),
            denoms: vec!["tbtc".to_string(), "nbtc".to_string()],
        });
        execute(deps.as_mut(), env.clone(), info.clone(), create_group_msg).unwrap();

        // Set static limiter for btc group
        let info = message_info(&admin, &[]);
        let set_config_msg = ContractExecMsg::Transmuter(ExecMsg::AddRebalancingConfig {
            scope: Scope::asset_group("group1"),
            label: "big_static_config".to_string(),
            rebalancing_config: RebalancingConfig::limit_only(Decimal::percent(70)).unwrap(),
        });
        execute(deps.as_mut(), env.clone(), info.clone(), set_config_msg).unwrap();

        // set static limiter for stbtc
        let set_config_msg = ContractExecMsg::Transmuter(ExecMsg::AddRebalancingConfig {
            scope: Scope::denom("stbtc"),
            label: "big_static_config".to_string(),
            rebalancing_config: RebalancingConfig::limit_only(Decimal::percent(70)).unwrap(),
        });
        execute(deps.as_mut(), env.clone(), info.clone(), set_config_msg).unwrap();

        // Add some liquidity
        let add_liquidity_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        execute(
            deps.as_mut(),
            env.clone(),
            message_info(
                &user,
                &[
                    coin(1_000_000_000_000, "tbtc"),
                    coin(1_000_000_000_000, "nbtc"),
                    coin(1_000_000_000_000, "stbtc"),
                ],
            ),
            add_liquidity_msg,
        )
        .unwrap();

        // Mark asset group as corrupted
        let info = message_info(&moderator, &[]);
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
        let info = message_info(&user, &[]);
        let exit_pool_msg = ContractExecMsg::Transmuter(ExecMsg::ExitPool {
            tokens_out: vec![
                coin(1_000_000_000_000, "tbtc"),
                coin(1_000_000_000_000, "nbtc"),
            ],
        });
        execute(deps.as_mut(), env.clone(), info.clone(), exit_pool_msg).unwrap();

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

        assert_eq!(total_pool_liquidity, vec![coin(1_000_000_000_000, "stbtc")]);

        // Assert that only one limiter remains for stbtc
        let limiters = Transmuter::new()
            .rebalancer
            .list_configs(&deps.storage)
            .unwrap()
            .into_iter()
            .map(|(k, _)| k)
            .collect::<Vec<_>>();
        assert_eq!(
            limiters,
            vec![("denom::stbtc".to_string(), "big_static_config".to_string())]
        );
    }

    fn total_liquidity_of(denom: &str, storage: &dyn Storage) -> Coin {
        Transmuter::new()
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

        let someone = deps.api.addr_make("someone");
        let user = deps.api.addr_make("user");
        let admin = deps.api.addr_make("admin");
        let moderator = deps.api.addr_make("moderator");
        let non_moderator = deps.api.addr_make("non_moderator");
        // make denom has non-zero total supply
        deps.querier
            .bank
            .update_balance(&someone, vec![coin(1, "uosmo"), coin(1, "uion")]);

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
        let info = message_info(&admin, &[]);

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
        let active_status: IsActiveResponse = from_json(res).unwrap();
        assert!(active_status.is_active);

        // Attempt to set the active status by a non-admin user.
        let non_moderator_info = message_info(&non_moderator, &[]);
        let non_moderator_msg =
            ContractExecMsg::Transmuter(ExecMsg::SetActiveStatus { active: false });
        let err = execute(
            deps.as_mut(),
            env.clone(),
            non_moderator_info,
            non_moderator_msg,
        )
        .unwrap_err();

        assert_eq!(err, ContractError::Unauthorized {});

        // Set the active status to false.
        let msg = ContractExecMsg::Transmuter(ExecMsg::SetActiveStatus { active: false });
        execute(
            deps.as_mut(),
            env.clone(),
            message_info(&moderator, &[]),
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
        let err = execute(
            deps.as_mut(),
            env.clone(),
            message_info(&moderator, &[]),
            msg,
        )
        .unwrap_err();
        assert_eq!(err, ContractError::UnchangedActiveStatus { status: false });

        // Test that JoinPool is blocked when active status is false
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        let err = execute(
            deps.as_mut(),
            env.clone(),
            message_info(&user, &[coin(1000, "uion"), coin(1000, "uosmo")]),
            join_pool_msg,
        )
        .unwrap_err();
        assert_eq!(err, ContractError::InactivePool {});

        // Test that SwapExactAmountIn is blocked when active status is false
        let swap_exact_amount_in_msg = SudoMsg::SwapExactAmountIn {
            token_in: coin(1000, "uion"),
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
            token_out: coin(500, "uosmo"),
            swap_fee: Decimal::zero(),
            token_in_denom: "uion".to_string(),
            token_in_max_amount: Uint128::new(1000),
        };
        let err = sudo(deps.as_mut(), env.clone(), swap_exact_amount_out_msg).unwrap_err();
        assert_eq!(err, ContractError::InactivePool {});

        // Test that ExitPool is blocked when active status is false
        let exit_pool_msg = ContractExecMsg::Transmuter(ExecMsg::ExitPool {
            tokens_out: vec![coin(1000, "uion"), coin(1000, "uosmo")],
        });
        let err = execute(
            deps.as_mut(),
            env.clone(),
            message_info(&user, &[coin(1000, "uion"), coin(1000, "uosmo")]),
            exit_pool_msg,
        )
        .unwrap_err();
        assert_eq!(err, ContractError::InactivePool {});

        // Set the active status back to true
        let msg = ContractExecMsg::Transmuter(ExecMsg::SetActiveStatus { active: true });
        execute(
            deps.as_mut(),
            env.clone(),
            message_info(&moderator, &[]),
            msg,
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
        assert!(active_status.is_active);

        // Test that JoinPool is active when active status is true
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        let res = execute(
            deps.as_mut(),
            env.clone(),
            message_info(&user, &[coin(1000, "uion"), coin(1000, "uosmo")]),
            join_pool_msg,
        );
        assert!(res.is_ok());

        let mock_sender = deps.api.addr_make("mock_sender");

        // Test that SwapExactAmountIn is active when active status is true
        let swap_exact_amount_in_msg = SudoMsg::SwapExactAmountIn {
            token_in: coin(100, "uion"),
            swap_fee: Decimal::zero(),
            sender: mock_sender.to_string(),
            token_out_denom: "uosmo".to_string(),
            token_out_min_amount: Uint128::new(100),
        };
        let res = sudo(deps.as_mut(), env.clone(), swap_exact_amount_in_msg);
        assert!(res.is_ok());

        let mock_sender = deps.api.addr_make("mock_sender");
        // Test that SwapExactAmountOut is active when active status is true
        let swap_exact_amount_out_msg = SudoMsg::SwapExactAmountOut {
            sender: mock_sender.into_string(),
            token_out: coin(100, "uosmo"),
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
            .bank
            .update_balance("someone", vec![coin(1, "uosmo"), coin(1, "uion")]);

        let admin = deps.api.addr_make("admin");
        let moderator = deps.api.addr_make("moderator");
        let canceling_candidate = deps.api.addr_make("canceling_candidate");
        let rejecting_candidate = deps.api.addr_make("rejecting_candidate");
        let candidate = deps.api.addr_make("candidate");
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
        let info = message_info(&admin, &[]);

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
            canceling_candidate.as_str()
        );

        // Cancel admin rights transfer
        let cancel_admin_transfer_msg =
            ContractExecMsg::Transmuter(ExecMsg::CancelAdminTransfer {});

        execute(
            deps.as_mut(),
            env.clone(),
            message_info(&admin, &[]),
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
            rejecting_candidate.as_str()
        );

        // Reject admin rights transfer
        let reject_admin_transfer_msg =
            ContractExecMsg::Transmuter(ExecMsg::RejectAdminTransfer {});

        execute(
            deps.as_mut(),
            env.clone(),
            message_info(&rejecting_candidate, &[]),
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
        assert_eq!(
            admin_candidate.admin_candidate.unwrap().as_str(),
            candidate.as_str()
        );

        // Claim admin rights by the candidate
        let claim_admin_msg = ContractExecMsg::Transmuter(ExecMsg::ClaimAdmin {});
        execute(
            deps.as_mut(),
            env.clone(),
            message_info(&candidate, &[]),
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
        assert_eq!(admin.admin.as_str(), candidate.as_str());
    }

    #[test]
    fn test_assign_and_remove_moderator() {
        let mut deps = mock_dependencies();
        let admin = deps.api.addr_make("admin");
        let moderator = deps.api.addr_make("moderator");
        let someone = deps.api.addr_make("someone");

        // make denom has non-zero total supply
        deps.querier
            .bank
            .update_balance(someone, vec![coin(1, "uosmo"), coin(1, "uion")]);

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
        instantiate(
            deps.as_mut(),
            mock_env(),
            message_info(&admin, &[]),
            init_msg,
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
        assert_eq!(
            moderator_response.moderator.into_string(),
            moderator.into_string()
        );

        let new_moderator = deps.api.addr_make("new_moderator");

        let non_admin = deps.api.addr_make("non_admin");
        // Try to assign new moderator by non admin
        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&non_admin, &[]),
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
            message_info(&admin, &[]),
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
        assert_eq!(
            moderator_response.moderator.to_string(),
            new_moderator.to_string()
        );
    }

    #[test]
    fn test_limiter_registration_and_config() {
        // register limiter
        let mut deps = mock_dependencies();

        // make denom has non-zero total supply
        deps.querier
            .bank
            .update_balance("someone", vec![coin(1, "uosmo"), coin(1, "uion")]);

        let admin = deps.api.addr_make("admin");
        let user = deps.api.addr_make("user");
        let moderator = deps.api.addr_make("moderator");
        let init_msg = InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("uosmo"),
                AssetConfig::from_denom_str("uion"),
            ],
            admin: Some(admin.to_string()),
            moderator: moderator.to_string(),
            alloyed_asset_subdenom: "usomoion".to_string(),
            alloyed_asset_normalization_factor: Uint128::one(),
        };

        instantiate(
            deps.as_mut(),
            mock_env(),
            message_info(&admin, &[]),
            init_msg,
        )
        .unwrap();

        // normal user can't register limiter
        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&user, &[]),
            ContractExecMsg::Transmuter(ExecMsg::AddRebalancingConfig {
                scope: Scope::Denom("uosmo".to_string()),
                label: "static".to_string(),
                rebalancing_config: RebalancingConfig::limit_only(Decimal::percent(60)).unwrap(),
            }),
        )
        .unwrap_err();

        assert_eq!(err, ContractError::Unauthorized {});

        // admin can register limiter
        let res = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin, &[]),
            ContractExecMsg::Transmuter(ExecMsg::AddRebalancingConfig {
                scope: Scope::Denom("uosmo".to_string()),
                label: "static".to_string(),
                rebalancing_config: RebalancingConfig::limit_only(Decimal::percent(60)).unwrap(),
            }),
        )
        .unwrap();

        let attrs = vec![
            attr("method", "add_rebalancing_config"),
            attr("label", "static"),
            attr("scope", "denom::uosmo"),
            attr("ideal_upper", "0.6"),
            attr("ideal_lower", "0"),
            attr("critical_upper", "0.6"),
            attr("critical_lower", "0"),
            attr("limit", "0.6"),
            attr("adjustment_rate_strained", "0"),
            attr("adjustment_rate_critical", "0"),
        ];

        assert_eq!(res.attributes, attrs);

        // denom that is not in the pool can't be registered
        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin, &[]),
            ContractExecMsg::Transmuter(ExecMsg::AddRebalancingConfig {
                scope: Scope::Denom("invalid_denom".to_string()),
                label: "static".to_string(),
                rebalancing_config: RebalancingConfig::limit_only(Decimal::percent(60)).unwrap(),
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
        let query_msg = ContractQueryMsg::Transmuter(QueryMsg::ListRebalancingConfigs {});
        let res = query(deps.as_ref(), mock_env(), query_msg).unwrap();
        let limiters: ListRebalancingConfigResponse = from_json(res).unwrap();

        assert_eq!(
            limiters.rebalancing_configs,
            vec![(
                (Scope::denom("uosmo").key(), String::from("static")),
                RebalancingConfig::limit_only(Decimal::percent(60)).unwrap()
            )]
        );

        // register another static limiter with different label
        let res = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin, &[]),
            ContractExecMsg::Transmuter(ExecMsg::AddRebalancingConfig {
                scope: Scope::Denom("uosmo".to_string()),
                label: "static2".to_string(),
                rebalancing_config: RebalancingConfig::limit_only(Decimal::percent(70)).unwrap(),
            }),
        )
        .unwrap();

        let attrs_static2 = vec![
            attr("method", "add_rebalancing_config"),
            attr("label", "static2"),
            attr("scope", "denom::uosmo"),
            attr("ideal_upper", "0.7"),
            attr("ideal_lower", "0"),
            attr("critical_upper", "0.7"),
            attr("critical_lower", "0"),
            attr("limit", "0.7"),
            attr("adjustment_rate_strained", "0"),
            attr("adjustment_rate_critical", "0"),
        ];

        assert_eq!(res.attributes, attrs_static2);

        // Query the list of limiters
        let query_msg = ContractQueryMsg::Transmuter(QueryMsg::ListRebalancingConfigs {});
        let res = query(deps.as_ref(), mock_env(), query_msg).unwrap();
        let limiters: ListRebalancingConfigResponse = from_json(res).unwrap();

        assert_eq!(
            limiters.rebalancing_configs,
            vec![
                (
                    (Scope::denom("uosmo").key(), String::from("static")),
                    RebalancingConfig::limit_only(Decimal::percent(60)).unwrap()
                ),
                (
                    (Scope::denom("uosmo").key(), String::from("static2")),
                    RebalancingConfig::limit_only(Decimal::percent(70)).unwrap()
                )
            ]
        );

        // deregister limiter by user is unauthorized
        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&user, &[]),
            ContractExecMsg::Transmuter(ExecMsg::RemoveRebalancingConfig {
                scope: Scope::Denom("uosmo".to_string()),
                label: "static".to_string(),
            }),
        )
        .unwrap_err();

        assert_eq!(err, ContractError::Unauthorized {});

        // deregister limiter by admin should work
        let res = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin, &[]),
            ContractExecMsg::Transmuter(ExecMsg::RemoveRebalancingConfig {
                scope: Scope::Denom("uosmo".to_string()),
                label: "static".to_string(),
            }),
        )
        .unwrap();

        let attrs = vec![
            attr("method", "remove_rebalancing_config"),
            attr("scope", "denom::uosmo"),
            attr("label", "static"),
        ];

        assert_eq!(res.attributes, attrs);

        // Query the list of limiters
        let query_msg = ContractQueryMsg::Transmuter(QueryMsg::ListRebalancingConfigs {});
        let res = query(deps.as_ref(), mock_env(), query_msg).unwrap();
        let limiters: ListRebalancingConfigResponse = from_json(res).unwrap();

        assert_eq!(
            limiters.rebalancing_configs,
            vec![(
                (Scope::denom("uosmo").key(), String::from("static2")),
                RebalancingConfig::limit_only(Decimal::percent(70)).unwrap()
            )]
        );

        // set upper limit by user is unauthorized
        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&user, &[]),
            ContractExecMsg::Transmuter(ExecMsg::UpdateRebalancingConfig {
                scope: Scope::Denom("uosmo".to_string()),
                label: "static2".to_string(),
                rebalancing_config: RebalancingConfig::limit_only(Decimal::percent(50)).unwrap(),
            }),
        )
        .unwrap_err();

        assert_eq!(err, ContractError::Unauthorized {});

        // Query the list of limiters
        let query_msg = ContractQueryMsg::Transmuter(QueryMsg::ListRebalancingConfigs {});
        let res = query(deps.as_ref(), mock_env(), query_msg).unwrap();
        let limiters: ListRebalancingConfigResponse = from_json(res).unwrap();

        assert_eq!(
            limiters.rebalancing_configs,
            vec![(
                (Scope::denom("uosmo").key(), String::from("static2")),
                RebalancingConfig::limit_only(Decimal::percent(70)).unwrap()
            )]
        );

        // set upper limit by admin for non-existent limiter should fail
        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin, &[]),
            ContractExecMsg::Transmuter(ExecMsg::UpdateRebalancingConfig {
                scope: Scope::Denom("uosmo".to_string()),
                label: "non_existent".to_string(),
                rebalancing_config: RebalancingConfig::limit_only(Decimal::percent(50)).unwrap(),
            }),
        )
        .unwrap_err();

        assert_eq!(
            err,
            ContractError::ConfigDoesNotExist {
                scope: Scope::denom("uosmo"),
                label: "non_existent".to_string()
            }
        );

        // set upper limit by admin for static limiter should work
        let res = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin, &[]),
            ContractExecMsg::Transmuter(ExecMsg::UpdateRebalancingConfig {
                scope: Scope::Denom("uosmo".to_string()),
                label: "static2".to_string(),
                rebalancing_config: RebalancingConfig::limit_only(Decimal::percent(50)).unwrap(),
            }),
        )
        .unwrap();

        let attrs = vec![
            attr("method", "update_rebalancing_config"),
            attr("scope", "denom::uosmo"),
            attr("label", "static2"),
            attr("ideal_upper", "0.5"),
            attr("ideal_lower", "0"),
            attr("critical_upper", "0.5"),
            attr("critical_lower", "0"),
            attr("limit", "0.5"),
            attr("adjustment_rate_strained", "0"),
            attr("adjustment_rate_critical", "0"),
        ];

        assert_eq!(res.attributes, attrs);

        // Query the list of limiters
        let query_msg = ContractQueryMsg::Transmuter(QueryMsg::ListRebalancingConfigs {});
        let res = query(deps.as_ref(), mock_env(), query_msg).unwrap();
        let limiters: ListRebalancingConfigResponse = from_json(res).unwrap();

        assert_eq!(
            limiters.rebalancing_configs,
            vec![(
                (Scope::denom("uosmo").key(), String::from("static2")),
                RebalancingConfig::limit_only(Decimal::percent(50)).unwrap()
            )]
        );

        // set upper limit by admin for non-existent limiter should fail
        let update_config_msg = ContractExecMsg::Transmuter(ExecMsg::UpdateRebalancingConfig {
            scope: Scope::denom("uosmo"),
            label: "nonexistent".to_string(),
            rebalancing_config: RebalancingConfig::limit_only(Decimal::percent(25)).unwrap(),
        });

        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin, &[]),
            update_config_msg,
        )
        .unwrap_err();

        assert_eq!(
            err,
            ContractError::ConfigDoesNotExist {
                scope: Scope::denom("uosmo"),
                label: "nonexistent".to_string()
            }
        );

        // set upper limit by admin for static limiter should work
        let update_config_msg = ContractExecMsg::Transmuter(ExecMsg::UpdateRebalancingConfig {
            scope: Scope::denom("uosmo"),
            label: "static2".to_string(),
            rebalancing_config: RebalancingConfig::limit_only(Decimal::percent(25)).unwrap(),
        });

        let res = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin, &[]),
            update_config_msg,
        )
        .unwrap();

        assert_eq!(
            res.attributes,
            vec![
                attr("method", "update_rebalancing_config"),
                attr("scope", "denom::uosmo"),
                attr("label", "static2"),
                attr("ideal_upper", "0.25"),
                attr("ideal_lower", "0"),
                attr("critical_upper", "0.25"),
                attr("critical_lower", "0"),
                attr("limit", "0.25"),
                attr("adjustment_rate_strained", "0"),
                attr("adjustment_rate_critical", "0"),
            ]
        );

        // Query the list of limiters
        let query_msg = ContractQueryMsg::Transmuter(QueryMsg::ListRebalancingConfigs {});
        let res = query(deps.as_ref(), mock_env(), query_msg).unwrap();
        let limiters: ListRebalancingConfigResponse = from_json(res).unwrap();

        assert_eq!(
            limiters.rebalancing_configs,
            vec![(
                (Scope::denom("uosmo").key(), String::from("static2")),
                RebalancingConfig::limit_only(Decimal::percent(25)).unwrap()
            )]
        );
    }

    #[test]
    fn test_set_alloyed_denom_metadata() {
        let mut deps = mock_dependencies();

        // make denom has non-zero total supply
        deps.querier
            .bank
            .update_balance("someone", vec![coin(1, "uosmo"), coin(1, "uion")]);

        let admin = deps.api.addr_make("admin");
        let non_admin = deps.api.addr_make("non_admin");
        let moderator = deps.api.addr_make("moderator");
        let init_msg = InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("uosmo"),
                AssetConfig::from_denom_str("uion"),
            ],
            alloyed_asset_subdenom: "uosmouion".to_string(),
            admin: Some(admin.to_string()),
            moderator: moderator.to_string(),
            alloyed_asset_normalization_factor: Uint128::one(),
        };
        let env = mock_env();
        let info = message_info(&admin, &[]);

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
        let non_admin_info = message_info(&non_admin, &[]);
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
            .bank
            .update_balance("someone", vec![coin(1, "uosmo"), coin(1, "uion")]);

        let admin = deps.api.addr_make("admin");
        let user = deps.api.addr_make("user");
        let moderator = deps.api.addr_make("moderator");
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
        let info = message_info(&admin, &[]);

        // Instantiate the contract.
        instantiate(deps.as_mut(), env.clone(), info, init_msg).unwrap();

        // Manually reply
        let alloyed_denom = "usomoion";

        reply(
            deps.as_mut(),
            env.clone(),
            reply_create_denom_response(alloyed_denom),
        )
        .unwrap();

        // join pool with amount 0 coin should error
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        let info = message_info(&user, &[coin(1000, "uion"), coin(0, "uosmo")]);
        let err = execute(deps.as_mut(), env.clone(), info, join_pool_msg).unwrap_err();

        assert_eq!(err, ContractError::ZeroValueOperation {});

        // join pool properly works
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        let info = message_info(&user, &[coin(1000, "uion"), coin(1000, "uosmo")]);
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
            vec![coin(1000, "uosmo"), coin(1000, "uion")]
        );
    }

    #[test]
    fn test_exit_pool() {
        let mut deps = mock_dependencies();
        let someone = deps.api.addr_make("someone");
        let admin = deps.api.addr_make("admin");
        let user = deps.api.addr_make("user");
        let moderator = deps.api.addr_make("moderator");
        // make denom has non-zero total supply
        deps.querier
            .bank
            .update_balance(someone, vec![coin(1, "uosmo"), coin(1, "uion")]);

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
        let info = message_info(&admin, &[]);

        // Instantiate the contract.
        instantiate(deps.as_mut(), env.clone(), info, init_msg).unwrap();

        // Manually reply
        let alloyed_denom = "usomoion";

        reply(
            deps.as_mut(),
            env.clone(),
            reply_create_denom_response(alloyed_denom),
        )
        .unwrap();

        // join pool by others for sufficient amount
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        let info = message_info(&admin, &[coin(1000, "uion"), coin(1000, "uosmo")]);
        execute(deps.as_mut(), env.clone(), info, join_pool_msg).unwrap();

        // User tries to exit pool
        let exit_pool_msg = ContractExecMsg::Transmuter(ExecMsg::ExitPool {
            tokens_out: vec![coin(1000, "uion"), coin(1000, "uosmo")],
        });
        let err = execute(
            deps.as_mut(),
            env.clone(),
            message_info(&user, &[]),
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
            message_info(&user, &[coin(1000, "uion"), coin(1000, "uosmo")]),
            join_pool_msg,
        );
        assert!(res.is_ok());

        deps.querier
            .bank
            .update_balance(&user, vec![coin(2000, alloyed_denom)]);

        // User tries to exit pool with zero amount
        let exit_pool_msg = ContractExecMsg::Transmuter(ExecMsg::ExitPool {
            tokens_out: vec![coin(0, "uion"), coin(1, "uosmo")],
        });
        let err = execute(
            deps.as_mut(),
            env.clone(),
            message_info(&user, &[]),
            exit_pool_msg,
        )
        .unwrap_err();
        assert_eq!(err, ContractError::ZeroValueOperation {});

        // User tries to exit pool again
        let exit_pool_msg = ContractExecMsg::Transmuter(ExecMsg::ExitPool {
            tokens_out: vec![coin(1000, "uion"), coin(1000, "uosmo")],
        });
        let res = execute(
            deps.as_mut(),
            env.clone(),
            message_info(&user, &[]),
            exit_pool_msg,
        )
        .unwrap();

        let expected = Response::new()
            .add_attribute("method", "exit_pool")
            .add_message(MsgBurn {
                sender: env.contract.address.to_string(),
                amount: Some(coin(2000u128, alloyed_denom).into()),
                burn_from_address: user.to_string(),
            })
            .add_message(BankMsg::Send {
                to_address: user.to_string(),
                amount: vec![coin(1000, "uion"), coin(1000, "uosmo")],
            });

        assert_eq!(res, expected);
    }

    #[test]
    fn test_shares_and_liquidity() {
        let mut deps = mock_dependencies();
        let someone = deps.api.addr_make("someone");
        let moderator = deps.api.addr_make("moderator");

        // make denom has non-zero total supply
        deps.querier
            .bank
            .update_balance(&someone, vec![coin(1, "uosmo"), coin(1, "uion")]);

        let admin = deps.api.addr_make("admin");
        let user_1 = deps.api.addr_make("user_1");
        let user_2 = deps.api.addr_make("user_2");
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
        let info = message_info(&admin, &[]);

        // Instantiate the contract.
        instantiate(deps.as_mut(), env.clone(), info, init_msg).unwrap();

        // Manually reply
        let alloyed_denom = "usomoion";

        reply(
            deps.as_mut(),
            env.clone(),
            reply_create_denom_response(alloyed_denom),
        )
        .unwrap();

        // Join pool
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        execute(
            deps.as_mut(),
            env.clone(),
            message_info(&user_1, &[coin(1000, "uion"), coin(1000, "uosmo")]),
            join_pool_msg,
        )
        .unwrap();

        // Update alloyed asset denom balance for user
        deps.querier
            .bank
            .update_balance(&user_1, vec![coin(2000, "usomoion")]);

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
            vec![coin(1000, "uosmo"), coin(1000, "uion")]
        );

        // Join pool
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        execute(
            deps.as_mut(),
            env.clone(),
            message_info(&user_2, &[coin(1000, "uion")]),
            join_pool_msg,
        )
        .unwrap();

        // Update balance for user 2
        deps.querier
            .bank
            .update_balance(user_2, vec![coin(1000, "usomoion")]);

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
            vec![coin(1000, "uosmo"), coin(2000, "uion")]
        );
    }

    #[test]
    fn test_denom() {
        let mut deps = mock_dependencies();

        // make denom has non-zero total supply
        deps.querier
            .bank
            .update_balance("someone", vec![coin(1, "uosmo"), coin(1, "uion")]);

        let admin = deps.api.addr_make("admin");
        let moderator = deps.api.addr_make("moderator");
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
        let info = message_info(&admin, &[]);

        // Instantiate the contract.
        instantiate(deps.as_mut(), env.clone(), info, init_msg).unwrap();

        // Manually reply
        let alloyed_denom = "usomoion";

        reply(
            deps.as_mut(),
            env.clone(),
            reply_create_denom_response(alloyed_denom),
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

        let someone = deps.api.addr_make("someone");
        let moderator = deps.api.addr_make("moderator");

        // make denom has non-zero total supply
        deps.querier
            .bank
            .update_balance(&someone, vec![coin(1, "uosmo"), coin(1, "uion")]);

        let admin = deps.api.addr_make("admin");
        let init_msg = InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("uosmo"),
                AssetConfig::from_denom_str("uion"),
            ],
            admin: Some(admin.to_string()),
            alloyed_asset_subdenom: "uosmoion".to_string(),
            alloyed_asset_normalization_factor: Uint128::one(),
            moderator: moderator.to_string(),
        };
        let env = mock_env();
        let info = message_info(&admin, &[]);

        // Instantiate the contract.
        instantiate(deps.as_mut(), env.clone(), info, init_msg).unwrap();

        // Manually reply
        let alloyed_denom = "uosmoion";

        reply(
            deps.as_mut(),
            env.clone(),
            reply_create_denom_response(alloyed_denom),
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
        let someone = deps.api.addr_make("someone");
        let admin = deps.api.addr_make("admin");
        let moderator = deps.api.addr_make("moderator");

        // make denom has non-zero total supply
        deps.querier
            .bank
            .update_balance(&someone, vec![coin(1, "tbtc"), coin(1, "nbtc")]);

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
            moderator: moderator.to_string(),
        };
        let env = mock_env();
        let info = message_info(&admin, &[]);

        // Instantiate the contract.
        instantiate(deps.as_mut(), env.clone(), info, init_msg).unwrap();

        // Manually reply
        let alloyed_denom = "allbtc";

        reply(
            deps.as_mut(),
            env.clone(),
            reply_create_denom_response(alloyed_denom),
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
        let admin = deps.api.addr_make("admin");
        let someone = deps.api.addr_make("someone");
        let moderator = deps.api.addr_make("moderator");

        // make denom has non-zero total supply
        deps.querier
            .bank
            .update_balance(&someone, vec![coin(1, "axlusdc"), coin(1, "whusdc")]);

        let init_msg = InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("axlusdc"),
                AssetConfig::from_denom_str("whusdc"),
            ],
            admin: Some(admin.to_string()),
            alloyed_asset_subdenom: "alloyedusdc".to_string(),
            alloyed_asset_normalization_factor: Uint128::one(),
            moderator: moderator.to_string(),
        };
        let env = mock_env();
        let info = message_info(&admin, &[]);

        // Instantiate the contract.
        instantiate(deps.as_mut(), env.clone(), info, init_msg).unwrap();

        // Manually reply
        let alloyed_denom = "alloyedusdc";

        reply(
            deps.as_mut(),
            env.clone(),
            reply_create_denom_response(alloyed_denom),
        )
        .unwrap();

        // join pool
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        execute(
            deps.as_mut(),
            env.clone(),
            message_info(&admin, &[coin(1000, "axlusdc"), coin(2000, "whusdc")]),
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
                token_in: coin(1000, "axlusdc"),
                token_out_denom: "whusdc".to_string(),
                swap_fee: Decimal::zero(),
                expected: Ok(CalcOutAmtGivenInResponse {
                    token_out: coin(1000, "whusdc"),
                }),
            },
            Case {
                name: String::from("whusdc to axlusdc - ok"),
                token_in: coin(1000, "whusdc"),
                token_out_denom: "axlusdc".to_string(),
                swap_fee: Decimal::zero(),
                expected: Ok(CalcOutAmtGivenInResponse {
                    token_out: coin(1000, "axlusdc"),
                }),
            },
            Case {
                name: String::from("whusdc to axlusdc - token out not enough"),
                token_in: coin(1001, "whusdc"),
                token_out_denom: "axlusdc".to_string(),
                swap_fee: Decimal::zero(),
                expected: Err(ContractError::InsufficientPoolAsset {
                    required: coin(1001, "axlusdc"),
                    available: coin(1000, "axlusdc"),
                }),
            },
            Case {
                name: String::from("same denom error (pool asset)"),
                token_in: coin(1000, "axlusdc"),
                token_out_denom: "axlusdc".to_string(),
                swap_fee: Decimal::zero(),
                expected: Err(ContractError::SameDenomNotAllowed {
                    denom: "axlusdc".to_string(),
                }),
            },
            Case {
                name: String::from("same denom error (alloyed asset)"),
                token_in: coin(1000, "alloyedusdc"),
                token_out_denom: "alloyedusdc".to_string(),
                swap_fee: Decimal::zero(),
                expected: Err(ContractError::SameDenomNotAllowed {
                    denom: "alloyedusdc".to_string(),
                }),
            },
            Case {
                name: String::from("alloyedusdc to axlusdc - ok"),
                token_in: coin(1000, "alloyedusdc"),
                token_out_denom: "axlusdc".to_string(),
                swap_fee: Decimal::zero(),
                expected: Ok(CalcOutAmtGivenInResponse {
                    token_out: coin(1000, "axlusdc"),
                }),
            },
            Case {
                name: String::from("alloyedusdc to whusdc - ok"),
                token_in: coin(1000, "alloyedusdc"),
                token_out_denom: "whusdc".to_string(),
                swap_fee: Decimal::zero(),
                expected: Ok(CalcOutAmtGivenInResponse {
                    token_out: coin(1000, "whusdc"),
                }),
            },
            Case {
                name: String::from("alloyedusdc to axlusdc - token out not enough"),
                token_in: coin(1001, "alloyedusdc"),
                token_out_denom: "axlusdc".to_string(),
                swap_fee: Decimal::zero(),
                expected: Err(ContractError::InsufficientPoolAsset {
                    required: coin(1001, "axlusdc"),
                    available: coin(1000, "axlusdc"),
                }),
            },
            Case {
                name: String::from("axlusdc to alloyedusdc - ok"),
                token_in: coin(1000, "axlusdc"),
                token_out_denom: "alloyedusdc".to_string(),
                swap_fee: Decimal::zero(),
                expected: Ok(CalcOutAmtGivenInResponse {
                    token_out: coin(1000, "alloyedusdc"),
                }),
            },
            Case {
                name: String::from("whusdc to alloyedusdc - ok"),
                token_in: coin(1000, "whusdc"),
                token_out_denom: "alloyedusdc".to_string(),
                swap_fee: Decimal::zero(),
                expected: Ok(CalcOutAmtGivenInResponse {
                    token_out: coin(1000, "alloyedusdc"),
                }),
            },
            Case {
                name: String::from("invalid swap fee"),
                token_in: coin(1000, "axlusdc"),
                token_out_denom: "whusdc".to_string(),
                swap_fee: Decimal::percent(1),
                expected: Err(ContractError::InvalidSwapFee {
                    expected: Decimal::zero(),
                    actual: Decimal::percent(1),
                }),
            },
            Case {
                name: String::from("invalid swap fee (alloyed asset as token in)"),
                token_in: coin(1000, "alloyedusdc"),
                token_out_denom: "whusdc".to_string(),
                swap_fee: Decimal::percent(1),
                expected: Err(ContractError::InvalidSwapFee {
                    expected: Decimal::zero(),
                    actual: Decimal::percent(1),
                }),
            },
            Case {
                name: String::from("invalid swap fee (alloyed asset as token out)"),
                token_in: coin(1000, "axlusdc"),
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
        let someone = deps.api.addr_make("someone");
        let admin = deps.api.addr_make("admin");
        let moderator = deps.api.addr_make("moderator");

        // make denom has non-zero total supply
        deps.querier
            .bank
            .update_balance(&someone, vec![coin(1, "axlusdc"), coin(1, "whusdc")]);

        let init_msg = InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("axlusdc"),
                AssetConfig::from_denom_str("whusdc"),
            ],
            admin: Some(admin.to_string()),
            alloyed_asset_subdenom: "alloyedusdc".to_string(),
            alloyed_asset_normalization_factor: Uint128::one(),
            moderator: moderator.to_string(),
        };
        let env = mock_env();
        let info = message_info(&admin, &[]);

        // Instantiate the contract.
        instantiate(deps.as_mut(), env.clone(), info, init_msg).unwrap();

        // Manually reply
        let alloyed_denom = "alloyedusdc";

        reply(
            deps.as_mut(),
            env.clone(),
            reply_create_denom_response(alloyed_denom),
        )
        .unwrap();

        // join pool
        let join_pool_msg = ContractExecMsg::Transmuter(ExecMsg::JoinPool {});
        execute(
            deps.as_mut(),
            env.clone(),
            message_info(&admin, &[coin(1000, "axlusdc"), coin(2000, "whusdc")]),
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
                token_out: coin(1000, "whusdc"),
                swap_fee: Decimal::zero(),
                expected: Ok(CalcInAmtGivenOutResponse {
                    token_in: coin(1000, "axlusdc"),
                }),
            },
            Case {
                name: String::from("whusdc to axlusdc - ok"),
                token_in_denom: "whusdc".to_string(),
                token_out: coin(1000, "axlusdc"),
                swap_fee: Decimal::zero(),
                expected: Ok(CalcInAmtGivenOutResponse {
                    token_in: coin(1000, "whusdc"),
                }),
            },
            Case {
                name: String::from("whusdc to axlusdc - token out not enough"),
                token_in_denom: "whusdc".to_string(),
                token_out: coin(1001, "axlusdc"),
                swap_fee: Decimal::zero(),
                expected: Err(ContractError::InsufficientPoolAsset {
                    required: coin(1001, "axlusdc"),
                    available: coin(1000, "axlusdc"),
                }),
            },
            Case {
                name: String::from("same denom error (pool asset)"),
                token_in_denom: "axlusdc".to_string(),
                token_out: coin(1000, "axlusdc"),
                swap_fee: Decimal::zero(),
                expected: Err(ContractError::SameDenomNotAllowed {
                    denom: "axlusdc".to_string(),
                }),
            },
            Case {
                name: String::from("same denom error (alloyed asset)"),
                token_in_denom: "alloyedusdc".to_string(),
                token_out: coin(1000, "alloyedusdc"),
                swap_fee: Decimal::zero(),
                expected: Err(ContractError::SameDenomNotAllowed {
                    denom: "alloyedusdc".to_string(),
                }),
            },
            Case {
                name: String::from("alloyedusdc to axlusdc - ok"),
                token_in_denom: "alloyedusdc".to_string(),
                token_out: coin(1000, "axlusdc"),
                swap_fee: Decimal::zero(),
                expected: Ok(CalcInAmtGivenOutResponse {
                    token_in: coin(1000, "alloyedusdc"),
                }),
            },
            Case {
                name: String::from("alloyedusdc to whusdc - ok"),
                token_in_denom: "alloyedusdc".to_string(),
                token_out: coin(1000, "whusdc"),
                swap_fee: Decimal::zero(),
                expected: Ok(CalcInAmtGivenOutResponse {
                    token_in: coin(1000, "alloyedusdc"),
                }),
            },
            Case {
                name: String::from("alloyedusdc to axlusdc - token out not enough"),
                token_in_denom: "alloyedusdc".to_string(),
                token_out: coin(1001, "axlusdc"),
                swap_fee: Decimal::zero(),
                expected: Err(ContractError::InsufficientPoolAsset {
                    required: coin(1001, "axlusdc"),
                    available: coin(1000, "axlusdc"),
                }),
            },
            Case {
                name: String::from("pool asset to alloyed asset - ok"),
                token_in_denom: "axlusdc".to_string(),
                token_out: coin(1000, "alloyedusdc"),
                swap_fee: Decimal::zero(),
                expected: Ok(CalcInAmtGivenOutResponse {
                    token_in: coin(1000, "axlusdc"),
                }),
            },
            Case {
                name: String::from("invalid swap fee"),
                token_in_denom: "whusdc".to_string(),
                token_out: coin(1000, "axlusdc"),
                swap_fee: Decimal::percent(1),
                expected: Err(ContractError::InvalidSwapFee {
                    expected: Decimal::zero(),
                    actual: Decimal::percent(1),
                }),
            },
            Case {
                name: String::from("invalid swap fee (alloyed asset as token in)"),
                token_in_denom: "alloyedusdc".to_string(),
                token_out: coin(1000, "axlusdc"),
                swap_fee: Decimal::percent(1),
                expected: Err(ContractError::InvalidSwapFee {
                    expected: Decimal::zero(),
                    actual: Decimal::percent(1),
                }),
            },
            Case {
                name: String::from("invalid swap fee (alloyed asset as token out)"),
                token_in_denom: "whusdc".to_string(),
                token_out: coin(1000, "alloyedusdc"),
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
        let someone = deps.api.addr_make("someone");
        let admin = deps.api.addr_make("admin");
        let moderator = deps.api.addr_make("moderator");

        // make denom has non-zero total supply
        deps.querier
            .bank
            .update_balance(&someone, vec![coin(1, "axlusdc"), coin(1, "whusdc")]);

        let init_msg = InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("axlusdc"),
                AssetConfig::from_denom_str("whusdc"),
            ],
            admin: Some(admin.to_string()),
            alloyed_asset_subdenom: "alloyedusdc".to_string(),
            alloyed_asset_normalization_factor: Uint128::from(100u128),
            moderator: moderator.to_string(),
        };
        let env = mock_env();
        let info = message_info(&admin, &[]);

        // Instantiate the contract.
        instantiate(deps.as_mut(), env.clone(), info, init_msg).unwrap();

        // Manually reply
        let alloyed_denom = "alloyedusdc";

        reply(
            deps.as_mut(),
            env.clone(),
            reply_create_denom_response(alloyed_denom),
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
            message_info(&admin, &[]),
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
            message_info(&admin, &[]),
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
        let admin = deps.api.addr_make("admin");
        let moderator = deps.api.addr_make("moderator");

        // Setup balance for each asset
        deps.querier.bank.update_balance(
            env.contract.address.clone(),
            vec![
                coin(1000000, "asset1"),
                coin(1000000, "asset2"),
                coin(1000000, "asset3"),
            ],
        );

        // Initialize the contract
        let instantiate_msg = InstantiateMsg {
            admin: Some(admin.to_string()),
            moderator: moderator.to_string(),
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

        let info = message_info(&admin, &[]);
        instantiate(deps.as_mut(), env.clone(), info.clone(), instantiate_msg).unwrap();

        // Create asset group
        let create_asset_group_msg = ContractExecMsg::Transmuter(ExecMsg::CreateAssetGroup {
            label: "group1".to_string(),
            denoms: vec!["asset1".to_string(), "asset2".to_string()],
        });

        // Test non-admin trying to create asset group
        let non_admin = deps.api.addr_make("non_admin");
        let non_admin_info = message_info(&non_admin, &[]);
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
        let add_rebalancing_config_msg =
            ContractExecMsg::Transmuter(ExecMsg::AddRebalancingConfig {
                label: "config1".to_string(),
                scope: Scope::asset_group("group2"),
                rebalancing_config: RebalancingConfig::limit_only(Decimal::percent(60)).unwrap(),
            });

        let add_rebalancing_config_info = message_info(&admin, &[]);
        let err = execute(
            deps.as_mut(),
            env.clone(),
            add_rebalancing_config_info.clone(),
            add_rebalancing_config_msg.clone(),
        )
        .unwrap_err();

        assert!(matches!(err, ContractError::AssetGroupNotFound { .. }));

        // Create group2
        let create_asset_group_msg2 = ContractExecMsg::Transmuter(ExecMsg::CreateAssetGroup {
            label: "group2".to_string(),
            denoms: vec!["asset3".to_string()],
        });

        let create_asset_group_info2 = message_info(&admin, &[]);
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

        // Try to add rebalancing config for group2
        let res3 = execute(
            deps.as_mut(),
            env.clone(),
            add_rebalancing_config_info,
            add_rebalancing_config_msg,
        )
        .unwrap();

        assert_eq!(
            res3.attributes,
            vec![
                attr("method", "add_rebalancing_config"),
                attr("label", "config1"),
                attr("scope", "asset_group::group2"),
                attr("ideal_upper", "0.6"),
                attr("ideal_lower", "0"),
                attr("critical_upper", "0.6"),
                attr("critical_lower", "0"),
                attr("limit", "0.6"),
                attr("adjustment_rate_strained", "0"),
                attr("adjustment_rate_critical", "0"),
            ]
        );

        // Verify limiter was registered
        let list_limiters_msg = ContractQueryMsg::Transmuter(QueryMsg::ListRebalancingConfigs {});
        let list_limiters_res: ListRebalancingConfigResponse =
            from_json(query(deps.as_ref(), env.clone(), list_limiters_msg).unwrap()).unwrap();

        assert_eq!(
            list_limiters_res.rebalancing_configs,
            vec![(
                (
                    Scope::asset_group("group2").to_string(),
                    "config1".to_string()
                ),
                RebalancingConfig::limit_only(Decimal::percent(60)).unwrap()
            )]
        );

        // Try to create a group with a non-existing asset
        let create_invalid_group_msg = ContractExecMsg::Transmuter(ExecMsg::CreateAssetGroup {
            label: "invalid_group".to_string(),
            denoms: vec!["asset1".to_string(), "non_existing_asset".to_string()],
        });

        let admin_info = message_info(&admin, &[]);
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
        let non_admin_info = message_info(&non_admin, &[]);
        let err = execute(
            deps.as_mut(),
            env.clone(),
            non_admin_info,
            remove_group_msg.clone(),
        )
        .unwrap_err();

        assert_eq!(err, ContractError::Unauthorized {});

        // Remove the group with the admin account
        let admin_info = message_info(&admin, &[]);
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
        let list_limiters_msg = ContractQueryMsg::Transmuter(QueryMsg::ListRebalancingConfigs {});
        let list_limiters_res: ListRebalancingConfigResponse =
            from_json(query(deps.as_ref(), env.clone(), list_limiters_msg).unwrap()).unwrap();

        // Check that limiter1 is not in the list of limiters
        assert_eq!(list_limiters_res.rebalancing_configs, vec![]);

        // Test removing a non-existing asset group
        let remove_nonexistent_group_msg = ContractExecMsg::Transmuter(ExecMsg::RemoveAssetGroup {
            label: "non_existent_group".to_string(),
        });

        let admin_info = message_info(&admin, &[]);
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
        let admin = deps.api.addr_make("admin");
        let moderator = deps.api.addr_make("moderator");
        let user = deps.api.addr_make("user");

        // Add supply for denoms using deps.querier.bank.update_balance
        deps.querier.bank.update_balance(
            env.contract.address.clone(),
            vec![coin(1000000, "asset1"), coin(2000000, "asset2")],
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
        let info = message_info(&admin, &[]);
        instantiate(deps.as_mut(), env.clone(), info, init_msg).unwrap();

        // Mark corrupted scopes
        let mark_corrupted_scopes_msg = ContractExecMsg::Transmuter(ExecMsg::MarkCorruptedScopes {
            scopes: vec![Scope::Denom("asset1".to_string())],
        });
        let moderator_info = message_info(&moderator, &[]);
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
        let user_info = message_info(&user, &[]);
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
        let admin_info = message_info(&admin, &[]);
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
        let moderator_info = message_info(&moderator, &[]);
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

    fn reply_create_denom_response(alloyed_denom: &str) -> Reply {
        let msg_create_denom_response = MsgCreateDenomResponse {
            new_token_denom: alloyed_denom.to_string(),
        };

        Reply {
            id: 1,
            result: SubMsgResult::Ok(
                #[allow(deprecated)]
                SubMsgResponse {
                    events: vec![],
                    data: Some(msg_create_denom_response.clone().into()), // DEPRECATED
                    msg_responses: vec![MsgResponse {
                        type_url: MsgCreateDenomResponse::TYPE_URL.to_string(),
                        value: msg_create_denom_response.into(),
                    }],
                },
            ),
            payload: Binary::new(vec![]),
            gas_used: 0,
        }
    }
}
