use cosmwasm_std::ensure;

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
}
