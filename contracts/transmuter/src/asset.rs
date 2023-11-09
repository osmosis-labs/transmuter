use cosmwasm_schema::cw_serde;
use cosmwasm_std::{ensure, Coin, Deps, Uint128};

use crate::ContractError;

#[cw_serde]
pub struct AssetConfig {
    pub denom: String,
    pub normalization_factor: Uint128,
}

impl AssetConfig {
    pub fn from_denom_str(denom: &str) -> Self {
        Self {
            denom: denom.to_string(),
            normalization_factor: Uint128::one(),
        }
    }

    pub fn checked_init_asset(self, deps: Deps) -> Result<Asset, ContractError> {
        let supply = deps.querier.query_supply(self.denom.as_str())?;

        // check for supply instead of metadata
        // since some denom (eg. ibc denom) could have no metadata
        ensure!(
            supply.amount > Uint128::zero(),
            ContractError::DenomHasNoSupply { denom: self.denom }
        );

        Ok(Asset {
            amount: Uint128::zero(),
            denom: self.denom,
            normalization_factor: self.normalization_factor,
        })
    }
}

#[cw_serde]
pub struct Asset {
    amount: Uint128,
    denom: String,
    normalization_factor: Uint128,
}

impl Asset {
    pub fn update_amount<F>(&mut self, f: F) -> Result<(), ContractError>
    where
        F: FnOnce(Uint128) -> Result<Uint128, ContractError>,
    {
        self.amount = f(self.amount)?;
        Ok(())
    }

    pub fn set_normalization_factor(
        mut self,
        normalization_factor: Uint128,
    ) -> Result<Self, ContractError> {
        ensure!(
            normalization_factor > Uint128::zero(),
            ContractError::NormalizationFactorMustBePositive {
                normalization_factor
            }
        );

        self.normalization_factor = normalization_factor;
        Ok(self)
    }

    pub fn denom(&self) -> &str {
        &self.denom
    }

    pub fn amount(&self) -> Uint128 {
        self.amount
    }

    pub fn normalization_factor(&self) -> Uint128 {
        self.normalization_factor
    }

    pub fn to_coin(&self) -> Coin {
        Coin {
            denom: self.denom.clone(),
            amount: self.amount,
        }
    }

    #[cfg(test)]
    pub fn unchecked(amount: Uint128, denom: &str, normalization_factor: Uint128) -> Self {
        Self {
            amount,
            denom: denom.to_string(),
            normalization_factor,
        }
    }

    #[cfg(test)]
    pub fn unchecked_equal_assets(denoms: &[&str]) -> Vec<Self> {
        denoms
            .iter()
            .map(|denom| Self::unchecked(Uint128::zero(), denom, Uint128::one()))
            .collect()
    }

    #[cfg(test)]
    pub fn unchecked_equal_assets_from_coins(coins: &[Coin]) -> Vec<Self> {
        coins
            .iter()
            .map(|coin| Self::unchecked(coin.amount, &coin.denom, Uint128::one()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmwasm_std::{testing::mock_dependencies_with_balances, Coin};

    #[test]
    fn test_checked_init_asset() {
        let deps = mock_dependencies_with_balances(&[
            ("addr1", &[Coin::new(1, "denom1")]),
            ("addr2", &[Coin::new(1, "denom2")]),
        ]);

        // denom1
        let asset_config = AssetConfig::from_denom_str("denom1");
        assert_eq!(
            asset_config.checked_init_asset(deps.as_ref()).unwrap(),
            Asset {
                amount: Uint128::zero(),
                denom: "denom1".to_string(),
                normalization_factor: Uint128::one(),
            }
        );

        // denom2
        let asset_config = AssetConfig {
            denom: "denom2".to_string(),
            normalization_factor: Uint128::from(1000000u128),
        };
        assert_eq!(
            asset_config.checked_init_asset(deps.as_ref()).unwrap(),
            Asset {
                amount: Uint128::zero(),
                denom: "denom2".to_string(),
                normalization_factor: Uint128::from(1000000u128),
            }
        );

        // denom3
        let asset_config = AssetConfig::from_denom_str("denom3");
        assert_eq!(
            asset_config.checked_init_asset(deps.as_ref()).unwrap_err(),
            ContractError::DenomHasNoSupply {
                denom: "denom3".to_string()
            }
        );
    }
}
