use std::collections::HashMap;

use cosmwasm_schema::cw_serde;
use cosmwasm_std::{ensure, ensure_eq, Coin, DepsMut, Response, Storage, Uint128};
use cw_storage_plus::Item;
use thiserror::Error;

use crate::{
    asset::{Asset, AssetConfig},
    contract::{key, CONTRACT_NAME, CONTRACT_VERSION},
    limiter::Limiters,
    transmuter_pool::TransmuterPool,
    ContractError,
};

const FROM_VERSION: &str = "2.0.0";
const TO_VERSION: &str = "3.0.0";

#[derive(Error, Debug, PartialEq)]
pub enum MigrationError {
    #[error("Missing normalization factor for {denom}")]
    MissingNormalizationFactor { denom: String },

    #[error("Adding normalization factor for non-pool asset: {denom}")]
    AddingNormalizationFactorForNonPoolAsset { denom: String },
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

    // reset all staled change limiter states as normalization factor will affect weight calculation
    Limiters::new(key::LIMITERS).reset_change_limiter_states(deps.storage)?;

    // Set the contract version to the target version after successful migration
    cw2::set_contract_version(deps.storage, CONTRACT_NAME, TO_VERSION)?;

    // Return a response with an attribute indicating the method that was executed
    Ok(Response::new().add_attribute("method", "v3_0_0/execute_migraiton"))
}

fn add_normalization_factor_to_pool_assets(
    asset_configs: Vec<AssetConfig>,
    storage: &mut dyn Storage,
) -> Result<(), ContractError> {
    let pool_v2: Item<'_, TransmuterPoolV2> = Item::new(key::POOL);
    let pool_v3: Item<'_, TransmuterPool> = Item::new(key::POOL);

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

    // ensure that all normalization factors are part of the pool assets
    asset_norm_factors
        .keys()
        .try_for_each(|denom| -> Result<(), ContractError> {
            ensure!(
                pool_assets.iter().any(|asset| asset.denom() == *denom),
                MigrationError::AddingNormalizationFactorForNonPoolAsset {
                    denom: denom.clone()
                }
            );

            Ok(())
        })?;

    pool_v3.save(storage, &TransmuterPool { pool_assets })?;
    Ok(())
}

fn set_alloyed_asset_normalization_factor(
    alloyed_asset_normalization_factor: Uint128,
    storage: &mut dyn Storage,
) -> Result<(), ContractError> {
    Item::<'_, Uint128>::new(key::ALLOYED_ASSET_NORMALIZATION_FACTOR)
        .save(storage, &alloyed_asset_normalization_factor)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use cosmwasm_std::{testing::mock_dependencies, Decimal, Timestamp, Uint64};

    use crate::limiter::{LimiterParams, WindowConfig};

    use super::*;

    #[test]
    fn test_successful_migration() {
        let mut deps = mock_dependencies();

        cw2::set_contract_version(&mut deps.storage, CONTRACT_NAME, FROM_VERSION).unwrap();

        Item::new(key::POOL)
            .save(
                &mut deps.storage,
                &TransmuterPoolV2 {
                    pool_assets: vec![Coin::new(10000, "denom1"), Coin::new(20000, "denom2")],
                },
            )
            .unwrap();

        let limiters = Limiters::new(key::LIMITERS);

        // set change limiter
        limiters
            .register(
                &mut deps.storage,
                "denom1",
                "1h",
                LimiterParams::ChangeLimiter {
                    window_config: WindowConfig {
                        window_size: Uint64::from(3_600_000_000_000u64),
                        division_count: Uint64::from(3u64),
                    },
                    boundary_offset: Decimal::percent(1),
                },
            )
            .unwrap();

        limiters
            .check_limits_and_update(
                &mut deps.storage,
                vec![("denom1".to_string(), Decimal::percent(5))],
                Timestamp::from_nanos(100000000000000u64),
            )
            .unwrap();

        let err = limiters
            .check_limits_and_update(
                &mut deps.storage,
                vec![("denom1".to_string(), Decimal::percent(10))],
                Timestamp::from_nanos(100000000010000u64),
            )
            .unwrap_err();

        assert_eq!(
            err,
            ContractError::UpperLimitExceeded {
                denom: "denom1".to_string(),
                upper_limit: Decimal::percent(6),
                value: Decimal::percent(10)
            }
        );

        let msg = MigrateMsg {
            asset_configs: vec![
                AssetConfig {
                    denom: "denom1".to_string(),
                    normalization_factor: Uint128::from(1000u128),
                },
                AssetConfig {
                    denom: "denom2".to_string(),
                    normalization_factor: Uint128::from(10000u128),
                },
            ],
            alloyed_asset_normalization_factor: Uint128::from(2u128),
        };

        let res = execute_migration(deps.as_mut(), msg).unwrap();

        assert_eq!(
            res,
            Response::new().add_attribute("method", "v3_0_0/execute_migraiton")
        );

        let TransmuterPool { pool_assets } = Item::new("pool").load(&deps.storage).unwrap();

        assert_eq!(
            pool_assets,
            vec![
                Asset::new(10000u128, "denom1", 1000u128),
                Asset::new(20000u128, "denom2", 10000u128)
            ]
        );

        let alloyed_asset_normalization_factor =
            Item::<'_, Uint128>::new(key::ALLOYED_ASSET_NORMALIZATION_FACTOR)
                .load(&deps.storage)
                .unwrap();

        assert_eq!(alloyed_asset_normalization_factor, Uint128::from(2u128));

        // running check_limits_and_update again with the same params should not fail
        limiters
            .check_limits_and_update(
                &mut deps.storage,
                vec![("denom1".to_string(), Decimal::percent(10))],
                Timestamp::from_nanos(100000000010000u64),
            )
            .unwrap();
    }

    #[test]
    fn test_missing_normalization_factor() {
        let mut deps = mock_dependencies();

        cw2::set_contract_version(&mut deps.storage, CONTRACT_NAME, FROM_VERSION).unwrap();

        Item::new(key::POOL)
            .save(
                &mut deps.storage,
                &TransmuterPoolV2 {
                    pool_assets: vec![Coin::new(10000, "denom1"), Coin::new(20000, "denom2")],
                },
            )
            .unwrap();

        let msg = MigrateMsg {
            asset_configs: vec![AssetConfig {
                denom: "denom1".to_string(),
                normalization_factor: Uint128::from(1000u128),
            }],
            alloyed_asset_normalization_factor: Uint128::from(2u128),
        };

        let err = execute_migration(deps.as_mut(), msg).unwrap_err();

        assert_eq!(
            err,
            ContractError::MigrationError(MigrationError::MissingNormalizationFactor {
                denom: "denom2".to_string()
            })
        );
    }

    #[test]
    fn test_adding_normalization_factor_for_non_pool_asset() {
        let mut deps = mock_dependencies();

        cw2::set_contract_version(&mut deps.storage, CONTRACT_NAME, FROM_VERSION).unwrap();

        Item::new(key::POOL)
            .save(
                &mut deps.storage,
                &TransmuterPoolV2 {
                    pool_assets: vec![Coin::new(10000, "denom1"), Coin::new(20000, "denom2")],
                },
            )
            .unwrap();

        let msg = MigrateMsg {
            asset_configs: vec![
                AssetConfig {
                    denom: "denom1".to_string(),
                    normalization_factor: Uint128::from(1000u128),
                },
                AssetConfig {
                    denom: "denom2".to_string(),
                    normalization_factor: Uint128::from(10000u128),
                },
                AssetConfig {
                    denom: "denom3".to_string(),
                    normalization_factor: Uint128::from(10000u128),
                },
            ],
            alloyed_asset_normalization_factor: Uint128::from(2u128),
        };

        let err = execute_migration(deps.as_mut(), msg).unwrap_err();

        assert_eq!(
            err,
            ContractError::MigrationError(
                MigrationError::AddingNormalizationFactorForNonPoolAsset {
                    denom: "denom3".to_string()
                }
            )
        );
    }

    #[test]
    fn test_invalid_version() {
        let mut deps = mock_dependencies();

        cw2::set_contract_version(&mut deps.storage, CONTRACT_NAME, "1.0.0").unwrap();

        let msg = MigrateMsg {
            asset_configs: vec![],
            alloyed_asset_normalization_factor: Uint128::zero(),
        };

        let err = execute_migration(deps.as_mut(), msg).unwrap_err();
        assert_eq!(
            err,
            ContractError::VersionError(cw2::VersionError::WrongVersion {
                expected: FROM_VERSION.to_string(),
                found: "1.0.0".to_string()
            })
        );
    }
}
