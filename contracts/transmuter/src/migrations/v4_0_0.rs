use std::collections::BTreeMap;

use cosmwasm_schema::cw_serde;

use cosmwasm_std::{ensure_eq, DepsMut, Response, Storage};
use cw2::{ContractVersion, VersionError, CONTRACT};
use cw_storage_plus::Item;

use crate::{
    asset::Asset,
    contract::{key, CONTRACT_NAME, CONTRACT_VERSION},
    transmuter_pool::TransmuterPool,
    ContractError,
};

const FROM_VERSION: &str = "3.2.0";
const TO_VERSION: &str = "4.0.0";

#[cw_serde]
pub struct MigrateMsg {}

#[cw_serde]
pub struct TransmuterPoolV3 {
    pub pool_assets: Vec<Asset>,
    // [to-be-added] pub asset_groups: BTreeMap<String, AssetGroup>,
}

pub fn execute_migration(deps: DepsMut) -> Result<Response, ContractError> {
    // Assert that the stored contract version matches the expected version before migration
    assert_contract_versions(deps.storage, CONTRACT_NAME, FROM_VERSION)?;

    // Ensure that the current contract version matches the target version to prevent migration to an incorrect version
    ensure_eq!(
        CONTRACT_VERSION,
        TO_VERSION,
        cw2::VersionError::WrongVersion {
            expected: TO_VERSION.to_string(),
            found: CONTRACT_VERSION.to_string()
        }
    );

    // add asset groups to the pool
    let pool_v3: TransmuterPoolV3 = Item::<TransmuterPoolV3>::new(key::POOL).load(deps.storage)?;

    let pool_v4 = TransmuterPool {
        pool_assets: pool_v3.pool_assets,
        asset_groups: BTreeMap::new(),
    };

    Item::<TransmuterPool>::new(key::POOL).save(deps.storage, &pool_v4)?;

    // Set the contract version to the target version after successful migration
    cw2::set_contract_version(deps.storage, CONTRACT_NAME, TO_VERSION)?;

    // Return a response with an attribute indicating the method that was executed
    Ok(Response::new().add_attribute("method", "v4_0_0/execute_migraiton"))
}

/// Assert that the stored contract version info matches the given value.
/// This is useful during migrations, for making sure that the correct contract
/// is being migrated, and it's being migrated from the correct version.
fn assert_contract_versions(
    storage: &dyn Storage,
    expected_contract: &str,
    expected_version: &str,
) -> Result<(), VersionError> {
    let ContractVersion { contract, version } = match CONTRACT.may_load(storage)? {
        Some(contract) => contract,
        None => return Err(VersionError::NotFound),
    };

    if contract != expected_contract {
        return Err(VersionError::WrongContract {
            expected: expected_contract.into(),
            found: contract,
        });
    }

    if version.as_str() != expected_version {
        return Err(VersionError::WrongVersion {
            expected: expected_version.to_string(),
            found: version,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use cosmwasm_std::{testing::mock_dependencies, Uint128};

    use super::*;

    #[test]
    fn test_successful_migration() {
        let mut deps = mock_dependencies();

        cw2::set_contract_version(&mut deps.storage, CONTRACT_NAME, FROM_VERSION).unwrap();

        let pool_assets = vec![
            Asset::new(Uint128::from(100u128), "uusdt", Uint128::from(1u128)).unwrap(),
            Asset::new(Uint128::from(200u128), "uusdc", Uint128::from(1u128)).unwrap(),
        ];
        let pool_v3 = TransmuterPoolV3 {
            pool_assets: pool_assets.clone(),
        };

        Item::new(key::POOL)
            .save(&mut deps.storage, &pool_v3)
            .unwrap();

        let res = execute_migration(deps.as_mut()).unwrap();

        let pool = Item::<TransmuterPool>::new(key::POOL)
            .load(&deps.storage)
            .unwrap();

        assert_eq!(
            pool,
            TransmuterPool {
                pool_assets,
                asset_groups: BTreeMap::new() // migrgate with empty asset groups
            }
        );

        assert_eq!(
            res,
            Response::new().add_attribute("method", "v4_0_0/execute_migraiton")
        );
    }

    #[test]
    fn test_invalid_version() {
        let mut deps = mock_dependencies();

        let pool_assets = vec![
            Asset::new(Uint128::from(100u128), "uusdt", Uint128::from(1u128)).unwrap(),
            Asset::new(Uint128::from(200u128), "uusdc", Uint128::from(1u128)).unwrap(),
        ];
        let pool_v3 = TransmuterPoolV3 {
            pool_assets: pool_assets.clone(),
        };

        Item::new(key::POOL)
            .save(&mut deps.storage, &pool_v3)
            .unwrap();

        cw2::set_contract_version(&mut deps.storage, CONTRACT_NAME, "3.0.0").unwrap();

        let err = execute_migration(deps.as_mut()).unwrap_err();
        assert_eq!(
            err,
            ContractError::VersionError(cw2::VersionError::WrongVersion {
                expected: FROM_VERSION.to_string(),
                found: "3.0.0".to_string()
            })
        );
    }
}
