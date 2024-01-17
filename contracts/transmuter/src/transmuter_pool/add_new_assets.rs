use crate::{asset::Asset, ContractError};

use super::TransmuterPool;

impl TransmuterPool {
    pub fn add_new_assets(&mut self, assets: Vec<Asset>) -> Result<(), ContractError> {
        self.pool_assets.extend(assets);

        self.ensure_no_duplicated_denom()?;
        self.ensure_pool_asset_count_within_range()
    }
}

#[cfg(test)]
mod tests {
    use cosmwasm_std::{Coin, Uint128, Uint64};

    use crate::transmuter_pool::{MAX_POOL_ASSET_DENOMS, MIN_POOL_ASSET_DENOMS};

    use super::*;

    #[test]
    fn test_add_new_assets() {
        let mut pool = TransmuterPool {
            pool_assets: Asset::unchecked_equal_assets_from_coins(&[
                Coin::new(100000000, "asset1"),
                Coin::new(99999999, "asset2"),
            ]),
        };
        let new_assets = Asset::unchecked_equal_assets_from_coins(&[
            Coin::new(0, "asset3"),
            Coin::new(0, "asset4"),
        ]);
        pool.add_new_assets(new_assets).unwrap();
        assert_eq!(
            pool.pool_assets,
            vec![
                Asset::unchecked(Uint128::new(100000000), "asset1", Uint128::one()),
                Asset::unchecked(Uint128::new(99999999), "asset2", Uint128::one()),
                Asset::unchecked(Uint128::zero(), "asset3", Uint128::one()),
                Asset::unchecked(Uint128::zero(), "asset4", Uint128::one()),
            ]
        );
    }

    #[test]
    fn test_add_duplicated_assets() {
        let mut pool = TransmuterPool {
            pool_assets: Asset::unchecked_equal_assets_from_coins(&[
                Coin::new(100000000, "asset1"),
                Coin::new(99999999, "asset2"),
            ]),
        };
        let new_assets = Asset::unchecked_equal_assets_from_coins(&[
            Coin::new(0, "asset3"),
            Coin::new(0, "asset4"),
        ]);
        pool.add_new_assets(new_assets.clone()).unwrap();
        let err = pool.add_new_assets(new_assets).unwrap_err();
        assert_eq!(
            err,
            ContractError::DuplicatedPoolAssetDenom {
                denom: "asset3".to_string()
            }
        );
    }

    #[test]
    fn test_add_same_new_assets() {
        let mut pool = TransmuterPool {
            pool_assets: Asset::unchecked_equal_assets_from_coins(&[
                Coin::new(100000000, "asset1"),
                Coin::new(99999999, "asset2"),
            ]),
        };
        let err = pool
            .add_new_assets(Asset::unchecked_equal_assets_from_coins(&[
                Coin::new(0, "asset5"),
                Coin::new(0, "asset5"),
            ]))
            .unwrap_err();
        assert_eq!(
            err,
            ContractError::DuplicatedPoolAssetDenom {
                denom: "asset5".to_string()
            }
        );
    }

    #[test]
    fn test_pool_asset_out_of_range() {
        let mut pool = TransmuterPool {
            pool_assets: Asset::unchecked_equal_assets_from_coins(&[
                Coin::new(100000000, "asset1"),
                Coin::new(99999999, "asset2"),
            ]),
        };
        let new_assets = Asset::unchecked_equal_assets_from_coins(&[
            Coin::new(0, "asset3"),
            Coin::new(0, "asset4"),
            Coin::new(0, "asset5"),
            Coin::new(0, "asset6"),
            Coin::new(0, "asset7"),
            Coin::new(0, "asset8"),
            Coin::new(0, "asset9"),
            Coin::new(0, "asset10"),
            Coin::new(0, "asset11"),
            Coin::new(0, "asset12"),
            Coin::new(0, "asset13"),
            Coin::new(0, "asset14"),
            Coin::new(0, "asset15"),
            Coin::new(0, "asset16"),
            Coin::new(0, "asset17"),
            Coin::new(0, "asset18"),
            Coin::new(0, "asset19"),
            Coin::new(0, "asset20"),
            Coin::new(0, "asset21"),
        ]);
        let err = pool.add_new_assets(new_assets).unwrap_err();
        assert_eq!(
            err,
            ContractError::PoolAssetDenomCountOutOfRange {
                min: MIN_POOL_ASSET_DENOMS,
                max: MAX_POOL_ASSET_DENOMS,
                actual: Uint64::new(21u64)
            }
        );
    }
}
