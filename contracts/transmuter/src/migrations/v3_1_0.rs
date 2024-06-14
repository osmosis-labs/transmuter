use cosmwasm_schema::cw_serde;

use cosmwasm_std::{ensure_eq, DepsMut, Response};

use crate::{
    contract::{CONTRACT_NAME, CONTRACT_VERSION},
    ContractError,
};

const FROM_VERSION: &str = "3.0.0";
const TO_VERSION: &str = "3.1.0";

#[cw_serde]
pub struct MigrateMsg {}

pub fn execute_migration(deps: DepsMut) -> Result<Response, ContractError> {
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

    // Set the contract version to the target version after successful migration
    cw2::set_contract_version(deps.storage, CONTRACT_NAME, TO_VERSION)?;

    // Return a response with an attribute indicating the method that was executed
    Ok(Response::new().add_attribute("method", "v3_1_0/execute_migraiton"))
}

#[cfg(test)]
mod tests {
    use cosmwasm_std::testing::mock_dependencies;

    use super::*;

    #[test]
    fn test_successful_migration() {
        let mut deps = mock_dependencies();

        cw2::set_contract_version(&mut deps.storage, CONTRACT_NAME, FROM_VERSION).unwrap();

        let res = execute_migration(deps.as_mut()).unwrap();

        assert_eq!(
            res,
            Response::new().add_attribute("method", "v3_1_0/execute_migraiton")
        );
    }

    #[test]
    fn test_invalid_version() {
        let mut deps = mock_dependencies();

        cw2::set_contract_version(&mut deps.storage, CONTRACT_NAME, "2.0.0").unwrap();

        let err = execute_migration(deps.as_mut()).unwrap_err();
        assert_eq!(
            err,
            ContractError::VersionError(cw2::VersionError::WrongVersion {
                expected: FROM_VERSION.to_string(),
                found: "2.0.0".to_string()
            })
        );
    }
}
