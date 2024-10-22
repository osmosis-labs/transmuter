use crate::{asset::Asset, ContractError};

use super::TransmuterPool;

impl TransmuterPool {
    pub fn add_new_assets(&mut self, assets: Vec<Asset>) -> Result<(), ContractError> {
        self.pool_assets.extend(assets);

        self.ensure_pool_asset_count_within_range()?;
        self.ensure_no_duplicated_denom()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use cosmwasm_std::{coin, Uint128, Uint64};

    use crate::transmuter_pool::{MAX_POOL_ASSET_DENOMS, MIN_POOL_ASSET_DENOMS};

    use super::*;

    #[test]
    fn test_add_new_assets() {
        let mut pool = TransmuterPool {
            pool_assets: Asset::unchecked_equal_assets_from_coins(&[
                coin(100000000, "asset1"),
                coin(99999999, "asset2"),
            ]),
            asset_groups: BTreeMap::new(),
        };
        let new_assets =
            Asset::unchecked_equal_assets_from_coins(&[coin(0, "asset3"), coin(0, "asset4")]);
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
                coin(100000000, "asset1"),
                coin(99999999, "asset2"),
            ]),
            asset_groups: BTreeMap::new(),
        };
        let new_assets =
            Asset::unchecked_equal_assets_from_coins(&[coin(0, "asset3"), coin(0, "asset4")]);
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
                coin(100000000, "asset1"),
                coin(99999999, "asset2"),
            ]),
            asset_groups: BTreeMap::new(),
        };
        let err = pool
            .add_new_assets(Asset::unchecked_equal_assets_from_coins(&[
                coin(0, "asset5"),
                coin(0, "asset5"),
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
                coin(100000000, "asset1"),
                coin(99999999, "asset2"),
            ]),
            asset_groups: BTreeMap::new(),
        };
        let new_assets = Asset::unchecked_equal_assets_from_coins(&[
            coin(0, "asset3"),
            coin(0, "asset4"),
            coin(0, "asset5"),
            coin(0, "asset6"),
            coin(0, "asset7"),
            coin(0, "asset8"),
            coin(0, "asset9"),
            coin(0, "asset10"),
            coin(0, "asset11"),
            coin(0, "asset12"),
            coin(0, "asset13"),
            coin(0, "asset14"),
            coin(0, "asset15"),
            coin(0, "asset16"),
            coin(0, "asset17"),
            coin(0, "asset18"),
            coin(0, "asset19"),
            coin(0, "asset20"),
            coin(0, "asset21"),
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
