use std::collections::BTreeMap;

use cosmwasm_std::{Decimal, Uint128};

use crate::{
    asset::{convert_amount, Rounding},
    math::lcm_from_iter,
    ContractError,
};

use super::TransmuterPool;

impl TransmuterPool {
    /// All weights of each pool assets. Returns pairs of (denom, weight)
    ///
    /// weights are calculated by:
    /// - finding standard normalization factor via lcm of all normalization factors
    /// - converting each pool asset amount to the standard normalization factor
    /// - calculating ratio of each pool asset amount to the total of normalized pool asset values
    ///
    /// If total pool asset amount is zero, returns None to signify that
    /// it makes no sense to calculate ratios, but not an error.
    pub fn weights(&self) -> Result<Option<Vec<(String, Decimal)>>, ContractError> {
        let std_norm_factor = lcm_from_iter(
            self.pool_assets
                .iter()
                .map(|pool_asset| pool_asset.normalization_factor()),
        )?;

        let normalized_asset_values = self.normalized_asset_values(std_norm_factor)?;

        let total_normalized_pool_value = normalized_asset_values
            .iter()
            .map(|(_, value)| value)
            .try_fold(Uint128::zero(), |acc, value| acc.checked_add(*value))?;

        if total_normalized_pool_value.is_zero() {
            return Ok(None);
        }

        let ratios = normalized_asset_values
            .into_iter()
            .map(|(denom, value)| {
                Ok((
                    denom,
                    Decimal::checked_from_ratio(value, total_normalized_pool_value)?,
                ))
            })
            .collect::<Result<_, ContractError>>()?;

        Ok(Some(ratios))
    }

    pub fn weights_map(&self) -> Result<BTreeMap<String, Decimal>, ContractError> {
        Ok(self.weights()?.unwrap_or_default().into_iter().collect())
    }

    fn normalized_asset_values(
        &self,
        std_norm_factor: Uint128,
    ) -> Result<Vec<(String, Uint128)>, ContractError> {
        self.pool_assets
            .iter()
            .map(|asset| {
                let value = convert_amount(
                    asset.amount(),
                    asset.normalization_factor(),
                    std_norm_factor,
                    &Rounding::Down, // This shouldn't matter since the target is LCM
                )?;

                Ok((asset.denom().to_string(), value))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::asset::Asset;
    use cosmwasm_std::Coin;
    use rstest::rstest;
    use std::str::FromStr;

    use super::*;

    #[rstest]
    // equal normalization factor
    #[case(
        vec![
            Asset::new(6000u128, "axlusdc", 1u128),
            Asset::new(4000u128, "whusdc", 1u128),
        ],
        vec![
            ("axlusdc".to_string(), Decimal::percent(60)),
            ("whusdc".to_string(), Decimal::percent(40))
        ]
    )]
    #[case(
        vec![
            Asset::new(6000u128, "axlusdc", 999u128),
            Asset::new(4000u128, "whusdc", 999u128),
        ],
        vec![
            ("axlusdc".to_string(), Decimal::percent(60)),
            ("whusdc".to_string(), Decimal::percent(40))
        ]
    )]
    #[case(
        vec![
            Asset::new(0u128, "axlusdc", 1u128),
            Asset::new(9999u128, "whusdc", 1u128),
        ],
        vec![
            ("axlusdc".to_string(), Decimal::percent(0)),
            ("whusdc".to_string(), Decimal::percent(100))
        ]
    )]
    #[case(
        vec![
           Asset::new(2u128, "axlusdc", 1u128),
           Asset::new(9999u128, "whusdc", 1u128),
           Asset::new(9999u128, "xusdc", 1u128),
        ],
        vec![
            ("axlusdc".to_string(), Decimal::from_str("0.0001").unwrap()),
            ("whusdc".to_string(), Decimal::from_str("0.49995").unwrap()),
            ("xusdc".to_string(), Decimal::from_str("0.49995").unwrap())
        ]
    )]
    // different normalization factor
    #[case(
        vec![
            Asset::new(6000u128, "a", 100_000_000u128),
            Asset::new(4000u128, "b", 1u128),
        ],
        vec![
            ("a".to_string(), Decimal::from_ratio(6000u128, 400_000_006_000u128)),
            ("b".to_string(), Decimal::from_ratio(400_000_000_000u128, 400_000_006_000u128))
        ]
    )]
    #[case(
        vec![
            Asset::new(6000u128, "a", 100_000_000u128),  // 6000 * 300_000_000 / 100_000_000 = 18_000
            Asset::new(4000u128, "b", 3u128), // 4000 * 300_000_000 / 3 = 400_000_000_000
            // 18_000 + 400_000_000_000 = 400_000_018_000
        ],
        vec![
            ("a".to_string(), Decimal::from_ratio(18_000u128, 400_000_018_000u128)),
            ("b".to_string(), Decimal::from_ratio(400_000_000_000u128, 400_000_018_000u128))
        ]
    )]
    #[case(
        vec![
            Asset::new(6000u128, "a", 100_000_000u128), // 6000 * 100_000_000 / 100_000_000 = 6000
            Asset::new(4000u128, "b", 1u128), // 4000 * 100_000_000 / 1 = 400_000_000_000
            Asset::new(3000u128, "c", 50_000_000u128), // 3000 * 100_000_000 / 50_000_000 = 6000
            // 6000 + 400_000_000_000 + 6000 = 400_000_012_000
        ],
        vec![
            ("a".to_string(), Decimal::from_ratio(6000u128, 400_000_012_000u128)),
            ("b".to_string(), Decimal::from_ratio(400_000_000_000u128, 400_000_012_000u128)),
            ("c".to_string(), Decimal::from_ratio(6000u128, 400_000_012_000u128))
        ]
    )]
    fn test_all_ratios(
        #[case] pool_assets: Vec<Result<Asset, ContractError>>,
        #[case] expected: Vec<(String, Decimal)>,
    ) {
        let pool_assets = pool_assets
            .into_iter()
            .map(|asset| asset.unwrap())
            .collect();
        let pool = TransmuterPool { pool_assets };

        let ratios = pool.weights().unwrap();
        assert_eq!(ratios, Some(expected));
    }

    #[test]
    fn test_all_ratios_when_total_pool_assets_is_zero() {
        let pool = TransmuterPool {
            pool_assets: Asset::unchecked_equal_assets_from_coins(&[
                Coin::new(0, "axlusdc"),
                Coin::new(0, "whusdc"),
            ]),
        };

        let ratios = pool.weights().unwrap();
        assert_eq!(ratios, None);
    }
}
