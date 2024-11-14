use std::collections::BTreeMap;

use cosmwasm_std::{Decimal, Decimal256, Uint128, Uint256};

use crate::{
    asset::{convert_amount, Rounding},
    math::lcm_from_iter,
    scope::Scope,
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
    pub fn asset_weights(&self) -> Result<Option<BTreeMap<String, Decimal>>, ContractError> {
        let normalized_asset_values = self.normalized_asset_values::<Vec<(String, Uint128)>>()?;

        let total_normalized_pool_value = normalized_asset_values
            .iter()
            .map(|(_, value)| Uint128::from(*value))
            .try_fold(Uint128::zero(), Uint128::checked_add)?;

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

    pub fn asset_group_weights(&self) -> Result<Option<BTreeMap<String, Decimal>>, ContractError> {
        let normalized_asset_values =
            self.normalized_asset_values::<BTreeMap<String, Uint128>>()?;

        let total_normalized_pool_value = normalized_asset_values
            .values()
            .copied()
            .map(Uint256::from)
            .try_fold(Uint256::zero(), Uint256::checked_add)?;

        if total_normalized_pool_value.is_zero() {
            return Ok(None);
        }

        let mut weights = BTreeMap::new();
        for (label, asset_group) in &self.asset_groups {
            let mut group_normalized_value = Uint256::zero();
            for denom in asset_group.denoms() {
                let denom_normalized_value = normalized_asset_values
                    .get(denom)
                    .copied()
                    .map(Uint256::from)
                    .unwrap_or_else(Uint256::zero);

                group_normalized_value =
                    group_normalized_value.checked_add(denom_normalized_value)?;
            }

            weights.insert(
                label.to_string(),
                Decimal256::checked_from_ratio(
                    group_normalized_value,
                    total_normalized_pool_value,
                )?
                // This is safe since weights are always less than 1, downcasting from Decimal256 to Decimal should never fail
                .try_into()?,
            );
        }

        Ok(Some(weights))
    }

    pub fn weights(&self) -> Result<Option<BTreeMap<Scope, Decimal>>, ContractError> {
        let Some(asset_weights) = self.asset_weights()? else {
            return Ok(None);
        };
        let Some(asset_group_weights) = self.asset_group_weights()? else {
            return Ok(None);
        };

        let asset_weights = asset_weights
            .into_iter()
            .map(|(denom, weight)| (Scope::Denom(denom), weight));

        let asset_group_weights = asset_group_weights
            .into_iter()
            .map(|(label, weight)| (Scope::AssetGroup(label), weight));

        Ok(Some(asset_weights.chain(asset_group_weights).collect()))
    }

    pub(crate) fn normalized_asset_values<T>(&self) -> Result<T, ContractError>
    where
        T: FromIterator<(String, Uint128)>,
    {
        let underlying_assets_norm_factor = self.underlying_assets_norm_factor()?;

        self.pool_assets
            .iter()
            .map(|asset| {
                let value = convert_amount(
                    asset.amount(),
                    asset.normalization_factor(),
                    underlying_assets_norm_factor,
                    &Rounding::Down, // This shouldn't matter since the target is LCM
                )?;

                Ok((asset.denom().to_string(), value))
            })
            .collect()
    }

    /// Calculate the underlying assets normalization factor for the pool.
    ///
    /// The underlying assets normalization factor is the least common multiple of all
    /// normalization factors in the pool (excluding the alloyed asset).
    pub fn underlying_assets_norm_factor(&self) -> Result<Uint128, ContractError> {
        Ok(lcm_from_iter(
            self.pool_assets
                .iter()
                .map(|pool_asset| pool_asset.normalization_factor()),
        )?)
    }
}

#[cfg(test)]
mod tests {
    use crate::asset::Asset;
    use cosmwasm_std::coin;
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
        let pool = TransmuterPool {
            pool_assets,
            asset_groups: BTreeMap::new(),
        };

        let ratios = pool.asset_weights().unwrap();
        assert_eq!(ratios, Some(expected.into_iter().collect()));
    }

    #[test]
    fn test_all_ratios_when_total_pool_assets_is_zero() {
        let pool = TransmuterPool {
            pool_assets: Asset::unchecked_equal_assets_from_coins(&[
                coin(0, "axlusdc"),
                coin(0, "whusdc"),
            ]),
            asset_groups: BTreeMap::new(),
        };

        let ratios = pool.asset_weights().unwrap();
        assert_eq!(ratios, None);
    }

    #[test]
    fn test_asset_group_weights() {
        let mut pool = TransmuterPool::new(vec![
            Asset::new(Uint128::new(200), "denom1", Uint128::new(2)).unwrap(),
            Asset::new(Uint128::new(300), "denom2", Uint128::new(3)).unwrap(),
            Asset::new(Uint128::new(500), "denom3", Uint128::new(5)).unwrap(),
        ])
        .unwrap();

        // Test with empty pool
        let weights = pool.asset_group_weights().unwrap().unwrap_or_default();
        assert!(weights.is_empty());

        pool.create_asset_group(
            "group1".to_string(),
            vec!["denom1".to_string(), "denom2".to_string()],
        )
        .unwrap();

        pool.create_asset_group("group2".to_string(), vec!["denom3".to_string()])
            .unwrap();

        let weights = pool.asset_group_weights().unwrap().unwrap();
        assert_eq!(weights.len(), 2);
        assert_eq!(
            weights.get("group1").unwrap(),
            &Decimal::raw(666666666666666666)
        );
        assert_eq!(
            weights.get("group2").unwrap(),
            &Decimal::raw(333333333333333333)
        );
    }

    #[test]
    fn test_asset_group_weights_with_potential_decimal_precision_loss() {
        let mut pool = TransmuterPool::new(vec![
            Asset::new(Uint128::new(100), "denom1", Uint128::new(1)).unwrap(),
            Asset::new(Uint128::new(200), "denom2", Uint128::new(1)).unwrap(),
            Asset::new(Uint128::new(0), "denom3", Uint128::new(1)).unwrap(),
        ])
        .unwrap();

        pool.create_asset_group(
            "group1".to_string(),
            vec!["denom1".to_string(), "denom2".to_string()],
        )
        .unwrap();

        let weights = pool.asset_group_weights().unwrap().unwrap_or_default();

        assert_eq!(weights.get("group1").unwrap(), &Decimal::percent(100));
    }

    #[test]
    fn test_weights() {
        let mut pool = TransmuterPool::new(vec![
            Asset::new(Uint128::new(100), "denom1", Uint128::new(1)).unwrap(),
            Asset::new(Uint128::new(200), "denom2", Uint128::new(1)).unwrap(),
            Asset::new(Uint128::new(300), "denom3", Uint128::new(1)).unwrap(),
        ])
        .unwrap();

        // Test with empty pool
        let weights = pool.weights().unwrap().unwrap_or_default();
        assert_eq!(
            weights,
            vec![
                (
                    Scope::Denom("denom1".to_string()),
                    Decimal::from_str("0.166666666666666666").unwrap()
                ),
                (
                    Scope::Denom("denom2".to_string()),
                    Decimal::from_str("0.333333333333333333").unwrap()
                ),
                (
                    Scope::Denom("denom3".to_string()),
                    Decimal::from_str("0.5").unwrap()
                ),
            ]
            .into_iter()
            .collect()
        );

        pool.create_asset_group(
            "group1".to_string(),
            vec!["denom1".to_string(), "denom2".to_string()],
        )
        .unwrap();

        pool.create_asset_group("group2".to_string(), vec!["denom3".to_string()])
            .unwrap();

        let weights = pool.weights().unwrap().unwrap();
        assert_eq!(
            weights,
            vec![
                (
                    Scope::Denom("denom1".to_string()),
                    Decimal::from_str("0.166666666666666666").unwrap()
                ),
                (
                    Scope::Denom("denom2".to_string()),
                    Decimal::from_str("0.333333333333333333").unwrap()
                ),
                (
                    Scope::Denom("denom3".to_string()),
                    Decimal::from_str("0.5").unwrap()
                ),
                (
                    Scope::AssetGroup("group1".to_string()),
                    Decimal::from_str("0.5").unwrap()
                ),
                (
                    Scope::AssetGroup("group2".to_string()),
                    Decimal::from_str("0.5").unwrap()
                ),
            ]
            .into_iter()
            .collect()
        );
    }
}
