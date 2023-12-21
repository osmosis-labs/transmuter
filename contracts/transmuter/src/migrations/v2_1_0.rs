use cosmwasm_schema::cw_serde;
use cosmwasm_std::{ensure_eq, DepsMut, Response};

use crate::{
    contract::{CONTRACT_NAME, CONTRACT_VERSION},
    ContractError,
};

const FROM_VERSION: &str = "2.0.0";
const TO_VERSION: &str = "2.1.0";

#[cw_serde]
pub struct MigrateMsg {}

// TODO: add normalization factor for each asset
// TODO: add `alloyed_denom_normalization_factor` to store

pub fn execute_migration(deps: DepsMut, _msg: MigrateMsg) -> Result<Response, ContractError> {
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
    Ok(Response::new().add_attribute("method", "v2_1_0/execute_migraiton"))
}
