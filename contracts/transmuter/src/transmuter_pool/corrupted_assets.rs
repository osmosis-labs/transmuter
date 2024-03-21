use std::collections::HashMap;

use cosmwasm_std::{ensure, Decimal};

use crate::{asset::Asset, ContractError};

use super::TransmuterPool;

impl TransmuterPool {
    pub fn mark_corrupted_assets(
        &mut self,
        corrupted_denoms: &[String],
    ) -> Result<(), ContractError> {
        // check if removing_assets are in the pool_assets
        for corrupted_denom in corrupted_denoms {
            ensure!(
                self.has_denom(corrupted_denom),
                ContractError::InvalidPoolAssetDenom {
                    denom: corrupted_denom.to_string()
                }
            );

            self.pool_assets
                .iter_mut()
                .find(|asset| asset.denom() == corrupted_denom)
                .map(|asset| asset.mark_as_corrupted());
        }

        Ok(())
    }

    pub fn corrupted_assets(&self) -> Vec<&Asset> {
        self.pool_assets
            .iter()
            .filter(|asset| asset.is_corrupted())
            .collect()
    }

    /// Enforce corrupted assets protocol on specific action. This will ensure that amount or weight
    /// of corrupted assets will never be increased.
    pub fn with_corrupted_asset_protocol<A, R>(&mut self, action: A) -> Result<R, ContractError>
    where
        A: FnOnce(&mut Self) -> Result<R, ContractError>,
    {
        let pool_asset_pre_action = self.pool_assets.clone();
        let corrupted_assets_pre_action = pool_asset_pre_action
            .iter()
            .filter(|asset| asset.is_corrupted())
            .map(|asset| (asset.denom().to_string(), asset))
            .collect::<HashMap<_, _>>();

        // if total pool value == 0 -> Empty mapping, later unwrap weight will be 0
        let weight_pre_action = self.weights()?.unwrap_or_default();
        let weight_pre_action = weight_pre_action.into_iter().collect::<HashMap<_, _>>();

        let res = action(self)?;

        let corrupted_assets_post_action = self
            .pool_assets
            .clone()
            .into_iter()
            .filter(|asset| asset.is_corrupted());

        // if total pool value == 0 -> Empty mapping, later unwrap weight will be 0
        let weight_post_action = self.weights()?.unwrap_or_default();
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
                ContractError::CorruptedAssetRelativelyIncreased {
                    denom: post_action.denom().to_string()
                }
            );
        }

        Ok(res)
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
        };

        // remove asset that is not in the pool
        let err = pool
            .mark_corrupted_assets(&["asset5".to_string()])
            .unwrap_err();
        assert_eq!(
            err,
            ContractError::InvalidPoolAssetDenom {
                denom: "asset5".to_string()
            }
        );

        let err = pool
            .mark_corrupted_assets(&["asset1".to_string(), "assetx".to_string()])
            .unwrap_err();
        assert_eq!(
            err,
            ContractError::InvalidPoolAssetDenom {
                denom: "assetx".to_string()
            }
        );

        pool.mark_corrupted_assets(&["asset1".to_string()]).unwrap();
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

        pool.mark_corrupted_assets(&["asset2".to_string(), "asset3".to_string()])
            .unwrap();

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
                if vec!["asset1", "asset2", "asset3"].contains(&asset.denom()) {
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
        };

        pool.mark_corrupted_assets(&["asset1".to_string()]).unwrap();

        // increase corrupted asset directly
        let err = pool
            .with_corrupted_asset_protocol(|pool| {
                pool.pool_assets
                    .iter_mut()
                    .find(|asset| asset.denom() == "asset1")
                    .map(|asset| asset.increase_amount(Uint128::new(1)).unwrap())
                    .unwrap();
                Ok(())
            })
            .unwrap_err();

        assert_eq!(
            err,
            ContractError::CorruptedAssetRelativelyIncreased {
                denom: "asset1".to_string()
            }
        );

        // decrease other asset -> increase corrupted asset weight
        let err = pool
            .with_corrupted_asset_protocol(|pool| {
                pool.pool_assets
                    .iter_mut()
                    .find(|asset| asset.denom() == "asset2")
                    .map(|asset| asset.decrease_amount(Uint128::new(1)).unwrap())
                    .unwrap();
                Ok(())
            })
            .unwrap_err();

        assert_eq!(
            err,
            ContractError::CorruptedAssetRelativelyIncreased {
                denom: "asset1".to_string()
            }
        );

        // decrease both corrupted and other asset with different weight
        let err = pool.with_corrupted_asset_protocol(|pool| {
            pool.pool_assets
                .iter_mut()
                .find(|asset| asset.denom() == "asset1")
                .map(|asset| asset.decrease_amount(Uint128::new(1)).unwrap())
                .unwrap();
            pool.pool_assets
                .iter_mut()
                .find(|asset| asset.denom() == "asset2")
                .map(|asset| asset.decrease_amount(Uint128::new(2)).unwrap())
                .unwrap();
            Ok(())
        });

        assert_eq!(
            err.unwrap_err(),
            ContractError::CorruptedAssetRelativelyIncreased {
                denom: "asset1".to_string()
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
        };

        pool.mark_corrupted_assets(&["asset1".to_string()]).unwrap();

        // decrease both corrupted and other asset with slightly more weight on the corrupted asset
        // requires slightly more weight to work due to rounding error
        pool.with_corrupted_asset_protocol(|pool| {
            pool.pool_assets
                .iter_mut()
                .find(|asset| asset.denom() == "asset1")
                .map(|asset| asset.decrease_amount(Uint128::new(2)).unwrap())
                .unwrap();
            pool.pool_assets
                .iter_mut()
                .find(|asset| asset.denom() == "asset2")
                .map(|asset| asset.decrease_amount(Uint128::new(1)).unwrap())
                .unwrap();
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
    }
}
