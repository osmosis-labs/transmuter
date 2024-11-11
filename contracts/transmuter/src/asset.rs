use cosmwasm_schema::cw_serde;
use cosmwasm_std::{ensure, Coin, Deps, StdError, Uint128, Uint256};

use crate::{corruptable::Corruptable, ContractError};

#[derive(PartialEq)]
pub enum Rounding {
    Up,
    Down,
}

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

        // check for zero normalization factor
        ensure!(
            self.normalization_factor > Uint128::zero(),
            ContractError::NormalizationFactorMustBePositive {}
        );

        Ok(Asset {
            amount: Uint128::zero(),
            denom: self.denom,
            normalization_factor: self.normalization_factor,
            is_corrupted: false,
        })
    }
}

#[cw_serde]
pub struct Asset {
    amount: Uint128, // TODO: change to Uint256
    denom: String,
    normalization_factor: Uint128,
    is_corrupted: bool,
}

impl Asset {
    pub fn new(
        amount: impl Into<Uint128>,
        denom: &str,
        normalization_factor: impl Into<Uint128>,
    ) -> Result<Self, ContractError> {
        let normalization_factor = normalization_factor.into();
        ensure!(
            normalization_factor > Uint128::zero(),
            ContractError::NormalizationFactorMustBePositive {}
        );

        Ok(Self {
            amount: amount.into(),
            denom: denom.to_string(),
            normalization_factor,
            is_corrupted: false,
        })
    }

    pub fn update_amount<F>(&'_ mut self, f: F) -> Result<&'_ Self, ContractError>
    where
        F: FnOnce(Uint128) -> Result<Uint128, ContractError>,
    {
        self.amount = f(self.amount)?;
        Ok(self)
    }

    pub fn increase_amount(
        &'_ mut self,
        increasing_amount: Uint128,
    ) -> Result<&'_ Self, ContractError> {
        self.update_amount(|amount| {
            amount
                .checked_add(increasing_amount)
                .map_err(StdError::overflow)
                .map_err(ContractError::Std)
        })
    }

    pub fn decrease_amount(
        &'_ mut self,
        decreasing_amount: Uint128,
    ) -> Result<&'_ Self, ContractError> {
        self.update_amount(|amount| {
            amount
                .checked_sub(decreasing_amount)
                .map_err(StdError::overflow)
                .map_err(ContractError::Std)
        })
    }

    pub fn set_normalization_factor(
        &'_ mut self,
        normalization_factor: Uint128,
    ) -> Result<&'_ Self, ContractError> {
        ensure!(
            normalization_factor > Uint128::zero(),
            ContractError::NormalizationFactorMustBePositive {}
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

    pub fn config(&self) -> AssetConfig {
        AssetConfig {
            denom: self.denom.clone(),
            normalization_factor: self.normalization_factor,
        }
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
            is_corrupted: false,
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

impl Corruptable for Asset {
    fn is_corrupted(&self) -> bool {
        self.is_corrupted
    }

    fn mark_as_corrupted(&mut self) -> &mut Self {
        self.is_corrupted = true;
        self
    }

    fn unmark_as_corrupted(&mut self) -> &mut Self {
        self.is_corrupted = false;
        self
    }
}

/// Convert amount to target asset's amount with the same value
///
/// target_amount / target_normalization_factor = amount / source_normalization_factor
/// target_amount = amount * target_normalization_factor / source_normalization_factor
///
/// Since amount unsigned int, we need to round up or down
/// This function gives control to the caller to decide how to round
pub fn convert_amount(
    amount: Uint128,
    source_normalization_factor: Uint128,
    target_normalization_factor: Uint128,
    rounding: &Rounding,
) -> Result<Uint128, ContractError> {
    let amount_by_target_norm = amount.full_mul(target_normalization_factor);
    let quotient: Uint256 =
        amount_by_target_norm.checked_div(Uint256::from(source_normalization_factor))?;

    let has_rem = !amount_by_target_norm
        .checked_rem(Uint256::from(source_normalization_factor))?
        .is_zero();

    if has_rem && rounding == &Rounding::Up {
        Ok(quotient.checked_add(Uint256::one())?.try_into()?)
    } else {
        Ok(quotient.try_into()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmwasm_std::{coin, testing::mock_dependencies_with_balances};

    #[test]
    fn test_convert_amount() {
        // 1 -> 1
        let asset = Asset::unchecked(0u128.into(), "denom1", 1u128.into());
        assert_eq!(
            convert_amount(
                100u128.into(),
                asset.normalization_factor(),
                1u128.into(),
                &Rounding::Up
            )
            .unwrap(),
            Uint128::from(100u128)
        );

        assert_eq!(
            convert_amount(
                100u128.into(),
                asset.normalization_factor(),
                1u128.into(),
                &Rounding::Down
            )
            .unwrap(),
            Uint128::from(100u128)
        );

        // 1 -> 10^12
        let asset = Asset::unchecked(0u128.into(), "denom1", 1u128.into());
        assert_eq!(
            convert_amount(
                100u128.into(),
                asset.normalization_factor(),
                1000000000000u128.into(),
                &Rounding::Up
            )
            .unwrap(),
            Uint128::from(100000000000000u128)
        );

        assert_eq!(
            convert_amount(
                100u128.into(),
                asset.normalization_factor(),
                1000000000000u128.into(),
                &Rounding::Down
            )
            .unwrap(),
            Uint128::from(100000000000000u128)
        );

        // 10^12 -> 1
        let asset = Asset::unchecked(0u128.into(), "denom1", 1000000000000u128.into());
        assert_eq!(
            convert_amount(
                100u128.into(),
                asset.normalization_factor(),
                1u128.into(),
                &Rounding::Up
            )
            .unwrap(),
            Uint128::from(1u128)
        );

        assert_eq!(
            convert_amount(
                100u128.into(),
                asset.normalization_factor(),
                1u128.into(),
                &Rounding::Down
            )
            .unwrap(),
            Uint128::from(0u128)
        );

        // 10^6 -> 10^18
        let asset = Asset::unchecked(0u128.into(), "denom1", 10u128.pow(6).into());

        // y = 10^18 * x / 10^6 = x * 10^12
        assert_eq!(
            convert_amount(
                10u128.pow(2).into(),
                asset.normalization_factor(),
                10u128.pow(18).into(),
                &Rounding::Up
            )
            .unwrap(),
            Uint128::from(10u128.pow(14))
        );

        assert_eq!(
            convert_amount(
                10u128.pow(2).into(),
                asset.normalization_factor(),
                10u128.pow(18).into(),
                &Rounding::Down
            )
            .unwrap(),
            Uint128::from(10u128.pow(14))
        );

        // 2 -> 3
        let asset = Asset::unchecked(0u128.into(), "denom1", 2u128.into());

        // y = 3 * x / 2
        assert_eq!(
            convert_amount(
                3u128.into(),
                asset.normalization_factor(),
                3u128.into(),
                &Rounding::Up
            )
            .unwrap(),
            Uint128::from(5u128)
        );

        assert_eq!(
            convert_amount(
                3u128.into(),
                asset.normalization_factor(),
                3u128.into(),
                &Rounding::Down
            )
            .unwrap(),
            Uint128::from(4u128)
        );
    }

    #[test]
    fn test_checked_init_asset() {
        let deps = mock_dependencies_with_balances(&[
            ("addr1", &[coin(1, "denom1")]),
            ("addr2", &[coin(1, "denom2")]),
        ]);

        // denom1
        // fail to init asset with zero normalization factor
        let asset_config = AssetConfig {
            denom: "denom1".to_string(),
            normalization_factor: Uint128::zero(),
        };
        assert_eq!(
            asset_config.checked_init_asset(deps.as_ref()).unwrap_err(),
            ContractError::NormalizationFactorMustBePositive {}
        );

        // success to init asset with non-zero normalization factor
        let asset_config = AssetConfig::from_denom_str("denom1");
        assert_eq!(
            asset_config.checked_init_asset(deps.as_ref()).unwrap(),
            Asset {
                amount: Uint128::zero(),
                denom: "denom1".to_string(),
                normalization_factor: Uint128::one(),
                is_corrupted: false,
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
                is_corrupted: false,
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

    #[test]
    fn test_set_normalization_factor() {
        let mut asset = Asset {
            amount: Uint128::zero(),
            denom: "denom1".to_string(),
            normalization_factor: Uint128::one(),
            is_corrupted: false,
        };

        assert_eq!(
            asset
                .set_normalization_factor(Uint128::from(1000000u128))
                .unwrap()
                .normalization_factor,
            Uint128::from(1000000u128)
        );

        assert_eq!(
            asset.set_normalization_factor(Uint128::zero()).unwrap_err(),
            ContractError::NormalizationFactorMustBePositive {}
        );
    }

    #[test]
    fn test_mark_as_corrupted() {
        let mut asset = Asset {
            amount: Uint128::zero(),
            denom: "denom1".to_string(),
            normalization_factor: Uint128::one(),
            is_corrupted: false,
        };

        assert!(!asset.is_corrupted());

        assert!(asset.mark_as_corrupted().is_corrupted());
        assert!(asset.is_corrupted());
    }
}
