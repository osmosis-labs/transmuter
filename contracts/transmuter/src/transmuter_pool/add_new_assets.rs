use cosmwasm_std::Coin;

use crate::{denom::Denom, ContractError};

use super::TransmuterPool;

impl TransmuterPool {
    pub fn add_new_assets(&mut self, denoms: &[Denom]) -> Result<(), ContractError> {
        self.pool_assets
            .extend(denoms.into_iter().map(|denom| Coin::new(0, denom.as_str())));

        self.ensure_no_duplicated_denom()?;
        self.ensure_pool_asset_count_within_range()
    }
}

#[cfg(test)]
mod tests {
    use cosmwasm_std::Uint64;

    use crate::transmuter_pool::{MAX_POOL_ASSET_DENOMS, MIN_POOL_ASSET_DENOMS};

    use super::*;

    #[test]
    fn test_add_new_assets() {
        let mut pool = TransmuterPool {
            pool_assets: vec![
                Coin::new(100000000, "asset1"),
                Coin::new(99999999, "asset2"),
            ],
        };
        let new_assets = vec![Denom::unchecked("asset3"), Denom::unchecked("asset4")];
        pool.add_new_assets(&new_assets).unwrap();
        assert_eq!(
            pool.pool_assets,
            vec![
                Coin::new(100000000, "asset1"),
                Coin::new(99999999, "asset2"),
                Coin::new(0, "asset3"),
                Coin::new(0, "asset4"),
            ]
        );
    }

    #[test]
    fn test_add_duplicated_assets() {
        let mut pool = TransmuterPool {
            pool_assets: vec![
                Coin::new(100000000, "asset1"),
                Coin::new(99999999, "asset2"),
            ],
        };
        let new_assets = vec![Denom::unchecked("asset3"), Denom::unchecked("asset4")];
        pool.add_new_assets(&new_assets).unwrap();
        let err = pool.add_new_assets(&new_assets).unwrap_err();
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
            pool_assets: vec![
                Coin::new(100000000, "asset1"),
                Coin::new(99999999, "asset2"),
            ],
        };
        let err = pool
            .add_new_assets(&[Denom::unchecked("asset5"), Denom::unchecked("asset5")])
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
            pool_assets: vec![
                Coin::new(100000000, "asset1"),
                Coin::new(99999999, "asset2"),
            ],
        };
        let new_assets = vec![
            Denom::unchecked("asset3"),
            Denom::unchecked("asset4"),
            Denom::unchecked("asset5"),
            Denom::unchecked("asset6"),
            Denom::unchecked("asset7"),
            Denom::unchecked("asset8"),
            Denom::unchecked("asset9"),
            Denom::unchecked("asset10"),
            Denom::unchecked("asset11"),
            Denom::unchecked("asset12"),
            Denom::unchecked("asset13"),
            Denom::unchecked("asset14"),
            Denom::unchecked("asset15"),
            Denom::unchecked("asset16"),
            Denom::unchecked("asset17"),
            Denom::unchecked("asset18"),
            Denom::unchecked("asset19"),
            Denom::unchecked("asset20"),
            Denom::unchecked("asset21"),
        ];
        let err = pool.add_new_assets(&new_assets).unwrap_err();
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
