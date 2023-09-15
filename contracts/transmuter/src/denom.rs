use cosmwasm_std::Deps;
use osmosis_std::types::cosmos::bank::v1beta1::BankQuerier;

use crate::ContractError;

/// Denom is a wrapper around String to ensure that the denom exists
pub struct Denom(String);

impl Denom {
    pub fn validate(deps: Deps, denom: String) -> Result<Self, ContractError> {
        let bank_querier = BankQuerier::new(&deps.querier);

        // query denom metadata to check that the denom exists
        bank_querier.denom_metadata(denom.clone()).map_err(|_| {
            ContractError::DenomDoesNotExist {
                denom: denom.clone(),
            }
        })?;

        Ok(Self(denom))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[cfg(test)]
    pub fn unchecked(denom: &str) -> Denom {
        Denom(denom.to_string())
    }
}

// TODO: test validate
