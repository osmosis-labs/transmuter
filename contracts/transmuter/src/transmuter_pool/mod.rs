mod add_new_assets;
mod asset_group;
mod corrupted_assets;
mod exit_pool;
mod has_denom;
mod join_pool;
mod transmute;
mod weight;

use std::collections::{BTreeMap, HashSet};

use cosmwasm_schema::cw_serde;
use cosmwasm_std::{ensure, Coin, Uint128, Uint64};
use itertools::Itertools;

use crate::{
    asset::{convert_amount, Asset, Rounding},
    math::lcm,
    scope::Scope,
    ContractError,
};

pub use asset_group::AssetGroup;
pub use transmute::AmountConstraint;

/// Minimum number of pool assets.
/// If the pool asset count is `1` it can still be transmuted to alloyed asset.
const MIN_POOL_ASSET_DENOMS: Uint64 = Uint64::new(1u64);

/// Maximum number of pool assets. This is required in order to
/// prevent the contract from running out of gas when iterating
const MAX_POOL_ASSET_DENOMS: Uint64 = Uint64::new(20u64);

/// Maximum number of asset groups allowed in a pool.
/// This limit helps prevent excessive gas consumption when iterating over groups.
const MAX_ASSET_GROUPS: Uint64 = Uint64::new(10u64);

#[cw_serde]
pub struct TransmuterPool {
    pub pool_assets: Vec<Asset>,
    pub asset_groups: BTreeMap<String, AssetGroup>,
}

impl TransmuterPool {
    pub fn new(pool_assets: Vec<Asset>) -> Result<Self, ContractError> {
        let pool = Self {
            pool_assets,
            asset_groups: BTreeMap::new(),
        };

        pool.ensure_no_duplicated_denom()?;
        pool.ensure_pool_asset_count_within_range()?;

        Ok(pool)
    }

    fn ensure_pool_asset_count_within_range(&self) -> Result<(), ContractError> {
        let count = Uint64::new(self.pool_assets.len() as u64);
        ensure!(
            count >= MIN_POOL_ASSET_DENOMS && count <= MAX_POOL_ASSET_DENOMS,
            ContractError::PoolAssetDenomCountOutOfRange {
                min: MIN_POOL_ASSET_DENOMS,
                max: MAX_POOL_ASSET_DENOMS,
                actual: count
            }
        );
        Ok(())
    }

    fn ensure_no_duplicated_denom(&self) -> Result<(), ContractError> {
        let mut denoms = HashSet::new();

        for asset in self.pool_assets.iter() {
            let is_new = denoms.insert(asset.denom());
            ensure!(
                is_new,
                ContractError::DuplicatedPoolAssetDenom {
                    denom: asset.denom().to_string()
                }
            );
        }

        Ok(())
    }

    pub fn get_pool_asset_by_denom(&self, denom: &'_ str) -> Result<&'_ Asset, ContractError> {
        self.pool_assets
            .iter()
            .find(|pool_asset| pool_asset.denom() == denom)
            .ok_or_else(|| ContractError::InvalidTransmuteDenom {
                denom: denom.to_string(),
                expected_denom: self
                    .pool_assets
                    .iter()
                    .map(|pool_asset| pool_asset.denom().to_string())
                    .collect(),
            })
    }

    pub fn pair_coins_with_normalization_factor(
        &self,
        coins: &[Coin],
    ) -> Result<Vec<(Coin, Uint128)>, ContractError> {
        coins
            .iter()
            .map(|coin| {
                Ok((
                    coin.clone(),
                    self.get_pool_asset_by_denom(coin.denom.as_str())?
                        .normalization_factor(),
                ))
            })
            .collect::<Result<Vec<_>, ContractError>>()
    }

    pub fn update_normalization_factor<F>(self, update_fn: F) -> Result<Self, ContractError>
    where
        F: Fn(Uint128) -> Result<Uint128, ContractError>,
    {
        let pool_assets = self
            .pool_assets
            .into_iter()
            .map(|mut pool_asset| {
                pool_asset
                    .set_normalization_factor(update_fn(pool_asset.normalization_factor())?)?;
                Ok(pool_asset)
            })
            .collect::<Result<Vec<_>, ContractError>>()?;

        Ok(Self {
            pool_assets,
            ..self
        })
    }

    pub fn scopes(&self) -> Result<Vec<Scope>, ContractError> {
        let denom_scopes = self
            .pool_assets
            .iter()
            .map(|asset| Scope::denom(asset.denom()));

        let asset_group_scopes = self
            .asset_groups
            .iter()
            .map(|(label, _)| Scope::asset_group(label));

        Ok(denom_scopes.chain(asset_group_scopes).collect())
    }

    /// Normalize coins to the standard normalization factor.
    pub fn normalize_coins(
        &self,
        coins: &[Coin],
        alloyed_denom: String,
        alloyed_normalization_factor: Uint128,
    ) -> Result<Vec<(String, Uint128)>, ContractError> {
        let std_norm_factor = lcm(
            self.underlying_assets_norm_factor()?,
            alloyed_normalization_factor,
        )?;
        let normalization_factor_by_denom: BTreeMap<String, Uint128> = self
            .pool_assets
            .clone()
            .into_iter()
            .map(|asset| (asset.denom().to_string(), asset.normalization_factor()))
            .chain(std::iter::once((
                alloyed_denom,
                alloyed_normalization_factor,
            )))
            .collect();

        coins
            .iter()
            .map(|c| {
                let value = convert_amount(
                    c.amount,
                    normalization_factor_by_denom
                        .get(&c.denom.to_string())
                        .copied()
                        .ok_or_else(|| ContractError::InvalidTransmuteDenom {
                            denom: c.denom.to_string(),
                            expected_denom: normalization_factor_by_denom.keys().cloned().collect(),
                        })?,
                    std_norm_factor,
                    &Rounding::Down, // This shouldn't matter since the target is LCM
                )?;

                Ok((c.denom.to_string(), value))
            })
            .collect()
    }

    /// Denormalize std normalized amount to the target denom amount.
    pub fn denormalize_amount(
        &self,
        std_amount: Uint128,
        target_denom: String,
        alloyed_denom: String,
        alloyed_normalization_factor: Uint128,
    ) -> Result<Uint128, ContractError> {
        let std_norm_factor = lcm(
            self.underlying_assets_norm_factor()?,
            alloyed_normalization_factor,
        )?;

        let target_norm_factor = if target_denom == alloyed_denom {
            alloyed_normalization_factor
        } else {
            self.pool_assets
                .iter()
                .find(|asset| asset.denom() == target_denom)
                .ok_or_else(|| ContractError::InvalidTransmuteDenom {
                    denom: target_denom.clone(),
                    expected_denom: self
                        .pool_assets
                        .iter()
                        .map(|asset| asset.denom().to_string())
                        .chain(std::iter::once(alloyed_denom.clone()))
                        .sorted()
                        .collect(),
                })?
                .normalization_factor()
        };

        convert_amount(
            std_amount,
            std_norm_factor,
            target_norm_factor,
            &Rounding::Down, // target is LCM so this shouldn't matter
        )
    }
}

#[cfg(test)]
mod tests {
    use cosmwasm_std::{coin, Uint128};

    use super::*;

    #[test]
    fn test_denom_count_within_range() {
        // 0 denom
        assert_eq!(
            TransmuterPool::new(Asset::unchecked_equal_assets(&[])).unwrap_err(),
            ContractError::PoolAssetDenomCountOutOfRange {
                min: MIN_POOL_ASSET_DENOMS,
                max: MAX_POOL_ASSET_DENOMS,
                actual: Uint64::new(0),
            }
        );

        // 1 denoms
        assert_eq!(
            TransmuterPool::new(Asset::unchecked_equal_assets(&["a"])).unwrap(),
            TransmuterPool {
                pool_assets: Asset::unchecked_equal_assets(&["a"]),
                asset_groups: BTreeMap::new(),
            }
        );

        // 2 denoms
        assert_eq!(
            TransmuterPool::new(Asset::unchecked_equal_assets(&["a", "b"])).unwrap(),
            TransmuterPool {
                pool_assets: Asset::unchecked_equal_assets(&["a", "b"]),
                asset_groups: BTreeMap::new(),
            }
        );

        // 20 denoms
        let assets: Vec<Asset> = (0..20)
            .map(|i| Asset::unchecked(Uint128::zero(), &format!("d{}", i), Uint128::one()))
            .collect();
        assert_eq!(
            TransmuterPool::new(assets.clone()).unwrap(),
            TransmuterPool {
                pool_assets: assets,
                asset_groups: BTreeMap::new(),
            }
        );

        // 21 denoms should fail
        let assets: Vec<Asset> = (0..21)
            .map(|i| Asset::unchecked(Uint128::zero(), &format!("d{}", i), Uint128::one()))
            .collect();
        assert_eq!(
            TransmuterPool::new(assets).unwrap_err(),
            ContractError::PoolAssetDenomCountOutOfRange {
                min: MIN_POOL_ASSET_DENOMS,
                max: MAX_POOL_ASSET_DENOMS,
                actual: Uint64::new(21),
            }
        );
    }

    #[test]
    fn test_duplicated_denom() {
        let assets = Asset::unchecked_equal_assets(&["a", "a"]);
        assert_eq!(
            TransmuterPool::new(assets).unwrap_err(),
            ContractError::DuplicatedPoolAssetDenom {
                denom: "a".to_string(),
            }
        );
    }

    #[test]
    fn test_list_all_scopes() {
        let assets = Asset::unchecked_equal_assets(&["a", "b"]);
        let mut transmuter_pool = TransmuterPool::new(assets).unwrap();

        // Add asset groups
        transmuter_pool
            .asset_groups
            .insert("group1".to_string(), AssetGroup::new(vec!["a".to_string()]));
        transmuter_pool
            .asset_groups
            .insert("group2".to_string(), AssetGroup::new(vec!["b".to_string()]));

        let scopes = transmuter_pool.scopes().unwrap();
        let expected_scopes: Vec<Scope> = vec![
            Scope::denom("a"),
            Scope::denom("b"),
            Scope::asset_group("group1"),
            Scope::asset_group("group2"),
        ];

        assert_eq!(scopes, expected_scopes);
    }

    #[test]
    fn test_normalize_coins() {
        let assets = vec![
            Asset::new(1000u128, "axlusdc", 10u128).unwrap(),
            Asset::new(2000u128, "whusdc", 100u128).unwrap(),
        ];
        let transmuter_pool = TransmuterPool::new(assets).unwrap();

        let coins = vec![coin(500u128, "axlusdc"), coin(1000u128, "whusdc")];

        let normalized_coins = transmuter_pool
            .normalize_coins(&coins, "alloyed".to_string(), Uint128::one())
            .unwrap();
        let expected_normalized_coins = vec![
            ("axlusdc".to_string(), Uint128::new(5000)),
            ("whusdc".to_string(), Uint128::new(1000)),
        ];

        assert_eq!(normalized_coins, expected_normalized_coins);

        // denormalize back
        let denormalized_coins = expected_normalized_coins
            .into_iter()
            .map(|(denom, amount)| {
                let amount = transmuter_pool
                    .denormalize_amount(
                        amount,
                        denom.clone(),
                        "alloyed".to_string(),
                        Uint128::one(),
                    )
                    .unwrap();
                coin(amount.u128(), denom)
            })
            .collect::<Vec<_>>();
        assert_eq!(denormalized_coins, coins);

        // with alloyed denom
        let coins = vec![
            coin(500u128, "axlusdc"),
            coin(1000u128, "whusdc"),
            coin(1000u128, "alloyed"),
        ];
        let normalized_coins = transmuter_pool
            .normalize_coins(&coins, "alloyed".to_string(), Uint128::one())
            .unwrap();
        let expected_normalized_coins = vec![
            ("axlusdc".to_string(), Uint128::new(5000)),
            ("whusdc".to_string(), Uint128::new(1000)),
            ("alloyed".to_string(), Uint128::new(100000)),
        ];

        assert_eq!(normalized_coins, expected_normalized_coins);

        // denormalize back
        let denormalized_coins = expected_normalized_coins
            .into_iter()
            .map(|(denom, amount)| {
                let amount = transmuter_pool
                    .denormalize_amount(
                        amount,
                        denom.clone(),
                        "alloyed".to_string(),
                        Uint128::one(),
                    )
                    .unwrap();
                coin(amount.u128(), denom)
            })
            .collect::<Vec<_>>();
        assert_eq!(denormalized_coins, coins);

        // Test with unknown denom
        let unknown_coins = vec![coin(1000u128, "whusdc"), coin(500u128, "unknown")];
        assert_eq!(
            transmuter_pool
                .normalize_coins(&unknown_coins, "alloyed".to_string(), Uint128::one())
                .unwrap_err(),
            ContractError::InvalidTransmuteDenom {
                denom: "unknown".to_string(),
                expected_denom: vec![
                    "alloyed".to_string(),
                    "axlusdc".to_string(),
                    "whusdc".to_string()
                ],
            }
        );

        // denormalize unknown denom
        assert_eq!(
            transmuter_pool
                .denormalize_amount(
                    Uint128::new(1000),
                    "unknown".to_string(),
                    "alloyed".to_string(),
                    Uint128::one(),
                )
                .unwrap_err(),
            ContractError::InvalidTransmuteDenom {
                denom: "unknown".to_string(),
                expected_denom: vec![
                    "alloyed".to_string(),
                    "axlusdc".to_string(),
                    "whusdc".to_string(),
                ],
            }
        );
    }
}
