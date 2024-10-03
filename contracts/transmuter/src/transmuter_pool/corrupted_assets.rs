use std::collections::{BTreeMap, HashMap};

use cosmwasm_std::{ensure, Decimal, Uint128};

use super::{asset_group::AssetGroup, TransmuterPool};
use crate::{asset::Asset, corruptable::Corruptable, scope::Scope, ContractError};

impl TransmuterPool {
    pub fn mark_corrupted_asset(&mut self, corrupted_denom: &str) -> Result<(), ContractError> {
        // check if denom is in the pool_assets
        ensure!(
            self.has_denom(corrupted_denom),
            ContractError::InvalidPoolAssetDenom {
                denom: corrupted_denom.to_string()
            }
        );

        for asset in self.pool_assets.iter_mut() {
            if asset.denom() == corrupted_denom {
                asset.mark_as_corrupted();
                break;
            }
        }

        Ok(())
    }

    pub fn unmark_corrupted_asset(&mut self, uncorrupted_denom: &str) -> Result<(), ContractError> {
        // check if denom is of corrupted asset
        ensure!(
            self.is_corrupted_asset(uncorrupted_denom),
            ContractError::InvalidCorruptedAssetDenom {
                denom: uncorrupted_denom.to_string()
            }
        );

        for asset in self.pool_assets.iter_mut() {
            if asset.denom() == uncorrupted_denom {
                asset.unmark_as_corrupted();
                break;
            }
        }

        Ok(())
    }

    pub fn corrupted_assets(&self) -> Vec<&Asset> {
        self.pool_assets
            .iter()
            .filter(|asset| asset.is_corrupted())
            .collect()
    }

    pub fn is_corrupted_asset(&self, denom: &str) -> bool {
        self.pool_assets
            .iter()
            .any(|asset| asset.denom() == denom && asset.is_corrupted())
    }

    pub fn remove_asset(&mut self, denom: &str) -> Result<(), ContractError> {
        let asset = self.get_pool_asset_by_denom(denom)?;

        // make sure that removing asset has 0 amount
        ensure!(
            asset.amount().is_zero(),
            ContractError::InvalidAssetRemoval {}
        );

        self.pool_assets.retain(|asset| asset.denom() != denom);

        // remove asset from asset groups
        for label in self.asset_groups.clone().keys() {
            let asset_group = self.asset_groups.get_mut(label).unwrap();
            asset_group.remove_denoms(vec![denom.to_string()]);

            if asset_group.denoms().is_empty() {
                self.asset_groups.remove(label);
            }
        }

        Ok(())
    }

    /// Enforce corrupted scopes protocol on specific action. This will ensure that amount or weight
    /// of corrupted scopes will never be increased.
    pub fn with_corrupted_scopes_protocol<A, R>(&mut self, action: A) -> Result<R, ContractError>
    where
        A: FnOnce(&mut Self) -> Result<R, ContractError>,
    {
        let pool_asset_pre_action = self.pool_assets.clone();
        let corrupted_assets_pre_action = pool_asset_pre_action
            .iter()
            .filter(|asset| asset.is_corrupted())
            .map(|asset| (asset.denom().to_string(), asset))
            .collect::<HashMap<_, _>>();

        // early return result without any checks if no corrupted assets
        if corrupted_assets_pre_action.is_empty() {
            return action(self);
        }

        // if total pool value == 0 -> Empty mapping, later unwrap weight will be 0
        let weight_pre_action = self.asset_weights()?.unwrap_or_default();
        let weight_pre_action = weight_pre_action.into_iter().collect::<HashMap<_, _>>();

        let res = action(self)?;

        let corrupted_assets_post_action = self
            .pool_assets
            .clone()
            .into_iter()
            .filter(|asset| asset.is_corrupted());

        // if total pool value == 0 -> Empty mapping, later unwrap weight will be 0
        let weight_post_action = self.asset_weights()?.unwrap_or_default();
        let weight_post_action = weight_post_action.into_iter().collect::<HashMap<_, _>>();

        for post_action in corrupted_assets_post_action {
            let denom = post_action.denom().to_string();
            let pre_action = corrupted_assets_pre_action
                .get(post_action.denom())
                .ok_or(ContractError::Never)?;

            let zero = Decimal::zero();
            let weight_pre_action = weight_pre_action.get(&denom).unwrap_or(&zero);
            let weight_post_action = weight_post_action.get(&denom).unwrap_or(&zero);

            let has_amount_increased = pre_action.amount() < post_action.amount();
            let has_weight_increased = weight_pre_action < weight_post_action;

            ensure!(
                !has_amount_increased && !has_weight_increased,
                ContractError::CorruptedScopeRelativelyIncreased {
                    scope: Scope::denom(post_action.denom())
                }
            );
        }

        Ok(res)
    }

    /// Enforce corrupted scopes protocol on specific action. This will ensure that amount or weight
    /// of corrupted scopes will never be increased.
    pub fn _with_corrupted_scopes_protocol<A, R>(
        &mut self,
        asset_groups: BTreeMap<String, AssetGroup>,
        action: A,
    ) -> Result<R, ContractError>
    where
        A: FnOnce(&mut Self) -> Result<R, ContractError>,
    {
        let corrupted_assets_pre_action = self.get_corrupted_assets();
        let corrupted_asset_groups = self.get_corrupted_asset_groups(asset_groups);

        // early return result without any checks if no corrupted scope
        if corrupted_assets_pre_action.is_empty() && corrupted_asset_groups.is_empty() {
            return action(self);
        }

        let weight_pre_action = self.get_weights()?;
        let corrupted_asset_groups_state_pre_action =
            self.get_corrupted_asset_groups_state(&corrupted_asset_groups, &weight_pre_action)?;

        let res = action(self)?;

        let corrupted_assets_post_action = self.get_corrupted_assets();
        let weight_post_action = self.get_weights()?;
        let corrupted_asset_groups_state_post_action =
            self.get_corrupted_asset_groups_state(&corrupted_asset_groups, &weight_post_action)?;

        self.check_corrupted_assets(
            &corrupted_assets_pre_action,
            &corrupted_assets_post_action,
            &weight_pre_action,
            &weight_post_action,
        )?;

        self.check_corrupted_asset_groups(
            &corrupted_asset_groups_state_pre_action,
            &corrupted_asset_groups_state_post_action,
        )?;

        Ok(res)
    }

    fn get_corrupted_assets(&self) -> HashMap<String, Asset> {
        self.pool_assets
            .iter()
            .filter(|asset| asset.is_corrupted())
            .map(|asset| (asset.denom().to_string(), asset.clone()))
            .collect()
    }

    fn get_corrupted_asset_groups(
        &self,
        asset_groups: BTreeMap<String, AssetGroup>,
    ) -> BTreeMap<String, AssetGroup> {
        asset_groups
            .into_iter()
            .filter(|(_, asset_group)| asset_group.is_corrupted())
            .collect()
    }

    fn get_weights(&self) -> Result<HashMap<String, Decimal>, ContractError> {
        Ok(self
            .asset_weights()?
            .unwrap_or_default()
            .into_iter()
            .collect())
    }

    /// Get the state of corrupted asset groups.
    /// returns map for label -> (amount, weight) for each asset group
    fn get_corrupted_asset_groups_state(
        &self,
        corrupted_asset_groups: &BTreeMap<String, AssetGroup>,
        weights: &HashMap<String, Decimal>,
    ) -> Result<BTreeMap<String, (Uint128, Decimal)>, ContractError> {
        corrupted_asset_groups
            .iter()
            .map(|(label, asset_group)| -> Result<_, ContractError> {
                let (amount, weight) = asset_group.denoms().iter().try_fold(
                    (Uint128::zero(), Decimal::zero()),
                    |(acc_amount, acc_weight), denom| -> Result<_, ContractError> {
                        let asset = self.get_pool_asset_by_denom(denom)?;
                        let amount = acc_amount.checked_add(asset.amount())?;
                        let weight = acc_weight
                            .checked_add(*weights.get(denom).unwrap_or(&Decimal::zero()))?;
                        Ok((amount, weight))
                    },
                )?;
                Ok((label.clone(), (amount, weight)))
            })
            .collect()
    }

    fn check_corrupted_assets(
        &self,
        pre_action: &HashMap<String, Asset>,
        post_action: &HashMap<String, Asset>,
        weight_pre_action: &HashMap<String, Decimal>,
        weight_post_action: &HashMap<String, Decimal>,
    ) -> Result<(), ContractError> {
        let zero_dec = Decimal::zero();
        for (denom, post_asset) in post_action {
            let pre_asset = pre_action.get(denom).ok_or(ContractError::Never)?;
            let weight_pre = weight_pre_action.get(denom).unwrap_or(&zero_dec);
            let weight_post = weight_post_action.get(denom).unwrap_or(&zero_dec);

            let has_amount_increased = pre_asset.amount() < post_asset.amount();
            let has_weight_increased = weight_pre < weight_post;

            ensure!(
                !has_amount_increased && !has_weight_increased,
                ContractError::CorruptedScopeRelativelyIncreased {
                    scope: Scope::denom(post_asset.denom())
                }
            );
        }
        Ok(())
    }

    fn check_corrupted_asset_groups(
        &self,
        pre_action: &BTreeMap<String, (Uint128, Decimal)>,
        post_action: &BTreeMap<String, (Uint128, Decimal)>,
    ) -> Result<(), ContractError> {
        for (label, (pre_amount, pre_weight)) in pre_action {
            let (post_amount, post_weight) = post_action.get(label).ok_or(ContractError::Never)?;

            let has_amount_increased = pre_amount < post_amount;
            let has_weight_increased = pre_weight < post_weight;

            ensure!(
                !has_amount_increased && !has_weight_increased,
                ContractError::CorruptedScopeRelativelyIncreased {
                    scope: Scope::asset_group(label)
                }
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::asset::Asset;
    use cosmwasm_std::{Coin, Uint128};

    use super::*;

    #[test]
    fn test_mark_corrupted_assets() {
        let mut pool = TransmuterPool {
            pool_assets: Asset::unchecked_equal_assets_from_coins(&[
                Coin::new(100000000, "asset1"),
                Coin::new(99999999, "asset2"),
                Coin::new(1, "asset3"),
                Coin::new(0, "asset4"),
            ]),
            asset_groups: BTreeMap::new(),
        };

        // remove asset that is not in the pool
        let err = pool.mark_corrupted_asset("asset5").unwrap_err();
        assert_eq!(
            err,
            ContractError::InvalidPoolAssetDenom {
                denom: "asset5".to_string()
            }
        );

        let err = pool.mark_corrupted_asset("assetx").unwrap_err();
        assert_eq!(
            err,
            ContractError::InvalidPoolAssetDenom {
                denom: "assetx".to_string()
            }
        );

        pool.mark_corrupted_asset("asset1").unwrap();
        assert_eq!(
            pool.pool_assets,
            Asset::unchecked_equal_assets_from_coins(&[
                Coin::new(100000000, "asset1"),
                Coin::new(99999999, "asset2"),
                Coin::new(1, "asset3"),
                Coin::new(0, "asset4"),
            ])
            .into_iter()
            .map(|asset| {
                if asset.denom() == "asset1" {
                    asset.clone().mark_as_corrupted().to_owned()
                } else {
                    asset
                }
            })
            .collect::<Vec<_>>()
        );
        assert_eq!(
            pool.corrupted_assets(),
            vec![
                Asset::unchecked(100000000u128.into(), "asset1", Uint128::one())
                    .mark_as_corrupted()
            ]
        );

        pool.mark_corrupted_asset("asset2").unwrap();
        pool.mark_corrupted_asset("asset3").unwrap();

        assert_eq!(
            pool.pool_assets,
            Asset::unchecked_equal_assets_from_coins(&[
                Coin::new(100000000, "asset1"),
                Coin::new(99999999, "asset2"),
                Coin::new(1, "asset3"),
                Coin::new(0, "asset4"),
            ])
            .into_iter()
            .map(|asset| {
                if ["asset1", "asset2", "asset3"].contains(&asset.denom()) {
                    asset.clone().mark_as_corrupted().to_owned()
                } else {
                    asset
                }
            })
            .collect::<Vec<_>>()
        );

        assert_eq!(
            pool.corrupted_assets(),
            vec![
                Asset::unchecked(100000000u128.into(), "asset1", Uint128::one())
                    .mark_as_corrupted(),
                Asset::unchecked(99999999u128.into(), "asset2", Uint128::one()).mark_as_corrupted(),
                Asset::unchecked(1u128.into(), "asset3", Uint128::one()).mark_as_corrupted()
            ]
        );
    }

    #[test]
    fn test_enforce_corrupted_asset_protocol() {
        let mut pool = TransmuterPool {
            pool_assets: Asset::unchecked_equal_assets_from_coins(&[
                Coin::new(99999999, "asset1"),
                Coin::new(100000000, "asset2"),
                Coin::new(1, "asset3"),
                Coin::new(0, "asset4"),
            ]),
            asset_groups: BTreeMap::new(),
        };

        pool.mark_corrupted_asset("asset1").unwrap();

        // increase corrupted asset directly
        let err = pool
            ._with_corrupted_scopes_protocol(BTreeMap::new(), |pool| {
                for asset in pool.pool_assets.iter_mut() {
                    if asset.denom() == "asset1" {
                        asset.increase_amount(Uint128::new(1)).unwrap();
                    }
                }
                Ok(())
            })
            .unwrap_err();

        assert_eq!(
            err,
            ContractError::CorruptedScopeRelativelyIncreased {
                scope: Scope::denom("asset1")
            }
        );

        // decrease other asset -> increase corrupted asset weight
        let err = pool
            ._with_corrupted_scopes_protocol(BTreeMap::new(), |pool| {
                for asset in pool.pool_assets.iter_mut() {
                    if asset.denom() == "asset2" {
                        asset.decrease_amount(Uint128::new(1)).unwrap();
                    }
                }
                Ok(())
            })
            .unwrap_err();

        assert_eq!(
            err,
            ContractError::CorruptedScopeRelativelyIncreased {
                scope: Scope::denom("asset1")
            }
        );

        // decrease both corrupted and other asset with different weight
        let err = pool
            ._with_corrupted_scopes_protocol(BTreeMap::new(), |pool| {
                for asset in pool.pool_assets.iter_mut() {
                    if asset.denom() == "asset1" {
                        asset.decrease_amount(Uint128::new(1)).unwrap();
                    }

                    if asset.denom() == "asset2" {
                        asset.decrease_amount(Uint128::new(2)).unwrap();
                    }
                }

                Ok(())
            })
            .unwrap_err();

        assert_eq!(
            err,
            ContractError::CorruptedScopeRelativelyIncreased {
                scope: Scope::denom("asset1")
            }
        );

        // reset the pool because pure rust test will not reset state on error
        let mut pool = TransmuterPool {
            pool_assets: Asset::unchecked_equal_assets_from_coins(&[
                Coin::new(99999999, "asset1"),
                Coin::new(100000000, "asset2"),
                Coin::new(1, "asset3"),
                Coin::new(0, "asset4"),
            ]),
            asset_groups: BTreeMap::new(),
        };

        pool.mark_corrupted_asset("asset1").unwrap();

        // decrease both corrupted and other asset with slightly more weight on the corrupted asset
        // requires slightly more weight to work due to rounding error
        pool._with_corrupted_scopes_protocol(BTreeMap::new(), |pool| {
            for asset in pool.pool_assets.iter_mut() {
                if asset.denom() == "asset1" {
                    asset.decrease_amount(Uint128::new(2)).unwrap();
                }

                if asset.denom() == "asset2" {
                    asset.decrease_amount(Uint128::new(1)).unwrap();
                }
            }
            Ok(())
        })
        .unwrap();

        assert_eq!(
            pool.pool_assets,
            Asset::unchecked_equal_assets_from_coins(&[
                Coin::new(99999997, "asset1"),
                Coin::new(99999999, "asset2"),
                Coin::new(1, "asset3"),
                Coin::new(0, "asset4"),
            ])
            .into_iter()
            .map(|asset| {
                if asset.denom() == "asset1" {
                    asset.clone().mark_as_corrupted().to_owned()
                } else {
                    asset
                }
            })
            .collect::<Vec<_>>()
        );

        let mut asset_groups = BTreeMap::from_iter(vec![(
            "group1".to_string(),
            AssetGroup::new(vec!["asset2".to_string(), "asset3".to_string()]),
        )]);

        // increase asset in non-corrupted asset group
        pool._with_corrupted_scopes_protocol(asset_groups.clone(), |pool| {
            for asset in pool.pool_assets.iter_mut() {
                if asset.denom() == "asset2" {
                    asset.increase_amount(Uint128::new(1)).unwrap();
                }
            }

            Ok(())
        })
        .unwrap();

        asset_groups.get_mut("group1").unwrap().mark_as_corrupted();

        // increase asset in corrupted asset group
        let err = pool
            ._with_corrupted_scopes_protocol(asset_groups.clone(), |pool| {
                for asset in pool.pool_assets.iter_mut() {
                    if asset.denom() == "asset2" {
                        asset.increase_amount(Uint128::new(1)).unwrap();
                    }
                }
                Ok(())
            })
            .unwrap_err();

        assert_eq!(
            err,
            ContractError::CorruptedScopeRelativelyIncreased {
                scope: Scope::asset_group("group1")
            }
        );

        // increase all assets except asset1
        let err = pool
            ._with_corrupted_scopes_protocol(asset_groups.clone(), |pool| {
                for asset in pool.pool_assets.iter_mut() {
                    if asset.denom() != "asset1" {
                        asset.increase_amount(Uint128::new(1)).unwrap();
                    }
                }

                Ok(())
            })
            .unwrap_err();

        assert_eq!(
            err,
            ContractError::CorruptedScopeRelativelyIncreased {
                scope: Scope::asset_group("group1")
            }
        );

        pool.unmark_corrupted_asset("asset1").unwrap();

        // decrease asset 2
        pool._with_corrupted_scopes_protocol(asset_groups.clone(), |pool| {
            for asset in pool.pool_assets.iter_mut() {
                if asset.denom() == "asset2" {
                    asset.decrease_amount(Uint128::new(1)).unwrap();
                }
            }
            Ok(())
        })
        .unwrap();

        // decrease asset 3
        pool._with_corrupted_scopes_protocol(asset_groups.clone(), |pool| {
            for asset in pool.pool_assets.iter_mut() {
                if asset.denom() == "asset3" {
                    asset.decrease_amount(Uint128::new(1)).unwrap();
                }
            }
            Ok(())
        })
        .unwrap();

        // decrease asset 2 and 3
        pool._with_corrupted_scopes_protocol(asset_groups.clone(), |pool| {
            for asset in pool.pool_assets.iter_mut() {
                if asset.denom() == "asset2" {
                    asset.decrease_amount(Uint128::new(1)).unwrap();
                }

                if asset.denom() == "asset3" {
                    asset.decrease_amount(Uint128::new(1)).unwrap();
                }
            }
            Ok(())
        })
        .unwrap();

        // decrease asset 4 should fail
        let err = pool
            ._with_corrupted_scopes_protocol(asset_groups.clone(), |pool| {
                for asset in pool.pool_assets.iter_mut() {
                    if asset.denom() == "asset4" {
                        asset.decrease_amount(Uint128::new(1)).unwrap();
                    }
                }
                Ok(())
            })
            .unwrap_err();

        assert_eq!(
            err,
            ContractError::CorruptedScopeRelativelyIncreased {
                scope: Scope::asset_group("group1")
            }
        );
    }

    #[test]
    fn test_remove_corrupted_asset() {
        let mut pool = TransmuterPool {
            pool_assets: Asset::unchecked_equal_assets_from_coins(&[
                Coin::new(100, "asset1"),
                Coin::new(200, "asset2"),
                Coin::new(300, "asset3"),
            ]),
            asset_groups: BTreeMap::from_iter(vec![(
                "group1".to_string(),
                AssetGroup::new(vec!["asset1".to_string(), "asset2".to_string()]),
            )]),
        };

        // Mark asset2 as corrupted
        pool.mark_corrupted_asset("asset2").unwrap();

        // Attempt to remove asset2 with non-zero amount (should fail)
        let err = pool.remove_asset("asset2").unwrap_err();
        assert_eq!(err, ContractError::InvalidAssetRemoval {});

        // Decrease amount of asset2 to zero
        for asset in pool.pool_assets.iter_mut() {
            if asset.denom() == "asset2" {
                asset.decrease_amount(Uint128::new(200)).unwrap();
            }
        }

        // Remove corrupted asset2 (should succeed)
        pool.remove_asset("asset2").unwrap();

        assert_eq!(
            pool.asset_groups,
            BTreeMap::from_iter(vec![(
                "group1".to_string(),
                AssetGroup::new(vec!["asset1".to_string()]),
            )])
        );

        // Verify asset2 is removed
        assert_eq!(
            pool.pool_assets,
            vec![
                Asset::unchecked(Uint128::new(100), "asset1", Uint128::one()),
                Asset::unchecked(Uint128::new(300), "asset3", Uint128::one()),
            ]
        );

        // Attempt to remove non-existent asset (should fail)
        let err = pool.remove_asset("non_existent").unwrap_err();
        assert_eq!(
            err,
            ContractError::InvalidTransmuteDenom {
                denom: "non_existent".to_string(),
                expected_denom: vec!["asset1".to_string(), "asset3".to_string()]
            }
        );

        // Mark asset1 as corrupted
        pool.mark_corrupted_asset("asset1").unwrap();

        // Decrease amount of asset1 to zero
        for asset in pool.pool_assets.iter_mut() {
            if asset.denom() == "asset1" {
                asset.decrease_amount(Uint128::new(100)).unwrap();
            }
        }

        // Remove corrupted asset1
        pool.remove_asset("asset1").unwrap();

        // Verify asset1 is removed
        assert_eq!(
            pool.pool_assets,
            vec![Asset::unchecked(
                Uint128::new(300),
                "asset3",
                Uint128::one()
            ),]
        );

        // Verify asset groups are updated
        assert!(pool.asset_groups.is_empty());
    }
}
