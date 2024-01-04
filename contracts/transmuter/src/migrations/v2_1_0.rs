use std::collections::HashMap;

use cosmwasm_schema::cw_serde;
use cosmwasm_std::{ensure_eq, Coin, DepsMut, Response, Storage, Uint128};
use cw_storage_plus::Item;
use thiserror::Error;

use crate::{
    asset::{Asset, AssetConfig},
    contract::{CONTRACT_NAME, CONTRACT_VERSION},
    transmuter_pool::TransmuterPool,
    ContractError,
};

const FROM_VERSION: &str = "2.0.0";
const TO_VERSION: &str = "2.1.0";

#[derive(Error, Debug, PartialEq)]
pub enum MigrationError {
    #[error("Missing normalization factor for {denom}")]
    MissingNormalizationFactor { denom: String },
}

#[cw_serde]
pub struct MigrateMsg {
    pub asset_configs: Vec<AssetConfig>,
    pub alloyed_asset_normalization_factor: Uint128,
}

#[cw_serde]
pub struct TransmuterPoolV2 {
    pub pool_assets: Vec<Coin>,
}

pub fn execute_migration(deps: DepsMut, msg: MigrateMsg) -> Result<Response, ContractError> {
    // Assert that the stored contract version matches the expected version before migration
    cw2::assert_contract_version(deps.storage, CONTRACT_NAME, FROM_VERSION)?;

    // Ensure that the current contract version matches the target version to prevent migration to an incorrect version
    ensure_eq!(
        CONTRACT_VERSION,
        TO_VERSION,
        cw2::VersionError::WrongVersion {
            expected: TO_VERSION.to_string(),
            found: CONTRACT_VERSION.to_string()
        }
    );

    // migrating states
    add_normalization_factor_to_pool_assets(msg.asset_configs, deps.storage)?;
    set_alloyed_asset_normalization_factor(msg.alloyed_asset_normalization_factor, deps.storage)?;

    // Set the contract version to the target version after successful migration
    cw2::set_contract_version(deps.storage, CONTRACT_NAME, TO_VERSION)?;

    // Return a response with an attribute indicating the method that was executed
    Ok(Response::new().add_attribute("method", "v2_1_0/execute_migraiton"))
}

fn add_normalization_factor_to_pool_assets(
    asset_configs: Vec<AssetConfig>,
    storage: &mut dyn Storage,
) -> Result<(), ContractError> {
    let pool_v2: Item<'_, TransmuterPoolV2> = Item::new("pool");
    let pool_v2_1: Item<'_, TransmuterPool> = Item::new("pool");

    // transform pool assets from Coin -> Asset (adding normalization factor)
    let asset_norm_factors = asset_configs
        .into_iter()
        .map(|asset_config| (asset_config.denom, asset_config.normalization_factor))
        .collect::<HashMap<String, Uint128>>();

    let pool_assets = pool_v2
        .load(storage)?
        .pool_assets
        .into_iter()
        .map(|coin| {
            let normalization_factor = asset_norm_factors.get(&coin.denom).ok_or_else(|| {
                MigrationError::MissingNormalizationFactor {
                    denom: coin.denom.clone(),
                }
            })?;

            Ok(Asset::new(coin.amount, &coin.denom, *normalization_factor))
        })
        .collect::<Result<Vec<Asset>, ContractError>>()?;

    pool_v2_1.save(storage, &TransmuterPool { pool_assets })?;
    Ok(())
}

fn set_alloyed_asset_normalization_factor(
    alloyed_asset_normalization_factor: Uint128,
    storage: &mut dyn Storage,
) -> Result<(), ContractError> {
    Item::<'_, Uint128>::new("alloyed_asset_normalization_factor")
        .save(storage, &alloyed_asset_normalization_factor)?;

    Ok(())
}
