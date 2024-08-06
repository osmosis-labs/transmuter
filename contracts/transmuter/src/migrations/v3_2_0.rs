use cosmwasm_schema::cw_serde;

use cosmwasm_std::{ensure_eq, DepsMut, Response, Storage};
use cw2::{ContractVersion, VersionError, CONTRACT};

use crate::{
    contract::{CONTRACT_NAME, CONTRACT_VERSION},
    ContractError,
};

const FROM_VERSIONS: &[&str] = &["3.0.0", "3.1.0"];
const TO_VERSION: &str = "3.2.0";

#[cw_serde]
pub struct MigrateMsg {}

pub fn execute_migration(deps: DepsMut) -> Result<Response, ContractError> {
    // Assert that the stored contract version matches the expected version before migration
    assert_contract_versions(deps.storage, CONTRACT_NAME, FROM_VERSIONS)?;

    // Ensure that the current contract version matches the target version to prevent migration to an incorrect version
    ensure_eq!(
        CONTRACT_VERSION,
        TO_VERSION,
        cw2::VersionError::WrongVersion {
            expected: TO_VERSION.to_string(),
            found: CONTRACT_VERSION.to_string()
        }
    );

    // Set the contract version to the target version after successful migration
    cw2::set_contract_version(deps.storage, CONTRACT_NAME, TO_VERSION)?;

    // Return a response with an attribute indicating the method that was executed
    Ok(Response::new().add_attribute("method", "v3_2_0/execute_migraiton"))
}

/// Assert that the stored contract version info matches the given value.
/// This is useful during migrations, for making sure that the correct contract
/// is being migrated, and it's being migrated from the correct version.
fn assert_contract_versions(
    storage: &dyn Storage,
    expected_contract: &str,
    expected_versions: &[&str],
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

    if !expected_versions.contains(&version.as_str()) {
        return Err(VersionError::WrongVersion {
            expected: expected_versions.join(","),
            found: version,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use cosmwasm_std::testing::mock_dependencies;

    use super::*;

    #[test]
    fn test_successful_migration() {
        let mut deps = mock_dependencies();

        for from_version in FROM_VERSIONS {
            cw2::set_contract_version(&mut deps.storage, CONTRACT_NAME, from_version.to_string())
                .unwrap();

            let res = execute_migration(deps.as_mut()).unwrap();

            assert_eq!(
                res,
                Response::new().add_attribute("method", "v3_2_0/execute_migraiton")
            );
        }
    }

    #[test]
    fn test_invalid_version() {
        let mut deps = mock_dependencies();

        cw2::set_contract_version(&mut deps.storage, CONTRACT_NAME, "2.0.0").unwrap();

        let err = execute_migration(deps.as_mut()).unwrap_err();
        assert_eq!(
            err,
            ContractError::VersionError(cw2::VersionError::WrongVersion {
                expected: FROM_VERSIONS.join(","),
                found: "2.0.0".to_string()
            })
        );
    }
}
