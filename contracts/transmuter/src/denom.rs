use cosmwasm_std::{ensure, Deps, Uint128};

use crate::ContractError;

/// Denom is a wrapper around String to ensure that the denom exists
#[derive(Debug, PartialEq, Eq)]
pub struct Denom(String);

impl Denom {
    pub fn validate(deps: Deps, denom: String) -> Result<Self, ContractError> {
        let supply = deps.querier.query_supply(denom.as_str())?;

        // check for supply instead of metadata
        // since some denom (eg. ibc denom) could have no metadata
        ensure!(
            supply.amount > Uint128::zero(),
            ContractError::DenomHasNoSupply { denom }
        );

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

#[cfg(test)]
mod tests {
    use cosmwasm_std::{testing::mock_dependencies_with_balances, Coin};

    use super::*;

    #[test]
    fn test_validate_denom() {
        let deps = mock_dependencies_with_balances(&[
            ("addr1", &[Coin::new(1, "denom1")]),
            ("addr2", &[Coin::new(1, "denom2")]),
        ]);

        // denom1
        assert_eq!(
            Denom::validate(deps.as_ref(), "denom1".to_string()).unwrap(),
            Denom::unchecked("denom1")
        );

        // denom2
        assert_eq!(
            Denom::validate(deps.as_ref(), "denom2".to_string()).unwrap(),
            Denom::unchecked("denom2")
        );

        // denom3
        assert_eq!(
            Denom::validate(deps.as_ref(), "denom3".to_string()).unwrap_err(),
            ContractError::DenomHasNoSupply {
                denom: "denom3".to_string()
            }
        );
    }
}
