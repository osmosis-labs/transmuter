use cosmwasm_std::ensure;

use crate::{asset::Asset, ContractError};

use super::TransmuterPool;

impl TransmuterPool {
    pub fn remove_assets(&mut self, removing_assets: &[String]) -> Result<(), ContractError> {
        // check if removing_assets are in the pool_assets
        for removing_asset in removing_assets {
            ensure!(
                self.has_denom(removing_asset),
                ContractError::InvalidPoolAssetDenom {
                    denom: removing_asset.to_string()
                }
            );
        }

        // separate removing_assets from pool_assets
        let removing_assets: Vec<&str> = removing_assets.iter().map(|s| s.as_str()).collect();
        let (removed_assets, remaining_assets): (Vec<Asset>, Vec<Asset>) = self
            .pool_assets
            .drain(..)
            .partition(|asset| removing_assets.contains(&asset.denom()));

        self.pool_assets = remaining_assets;
        self.removed_assets.extend(removed_assets);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::asset::Asset;
    use cosmwasm_std::Coin;

    use super::*;

    #[test]
    fn test_remove_assets() {
        let mut pool = TransmuterPool {
            pool_assets: Asset::unchecked_equal_assets_from_coins(&[
                Coin::new(100000000, "asset1"),
                Coin::new(99999999, "asset2"),
                Coin::new(1, "asset3"),
                Coin::new(0, "asset4"),
            ]),
            removed_assets: vec![],
        };

        // remove asset that is not in the pool
        let err = pool.remove_assets(&["asset5".to_string()]).unwrap_err();
        assert_eq!(
            err,
            ContractError::InvalidPoolAssetDenom {
                denom: "asset5".to_string()
            }
        );

        let err = pool
            .remove_assets(&["asset1".to_string(), "assetx".to_string()])
            .unwrap_err();
        assert_eq!(
            err,
            ContractError::InvalidPoolAssetDenom {
                denom: "assetx".to_string()
            }
        );

        pool.remove_assets(&["asset1".to_string()]).unwrap();
        assert_eq!(
            pool.pool_assets,
            Asset::unchecked_equal_assets_from_coins(&[
                Coin::new(99999999, "asset2"),
                Coin::new(1, "asset3"),
                Coin::new(0, "asset4"),
            ])
        );
        assert_eq!(
            pool.removed_assets,
            Asset::unchecked_equal_assets_from_coins(&[Coin::new(100000000, "asset1")])
        );

        pool.remove_assets(&["asset2".to_string(), "asset3".to_string()])
            .unwrap();

        assert_eq!(
            pool.pool_assets,
            Asset::unchecked_equal_assets_from_coins(&[Coin::new(0, "asset4")])
        );

        assert_eq!(
            pool.removed_assets,
            Asset::unchecked_equal_assets_from_coins(&[
                Coin::new(100000000, "asset1"),
                Coin::new(99999999, "asset2"),
                Coin::new(1, "asset3"),
            ])
        );
    }
}
