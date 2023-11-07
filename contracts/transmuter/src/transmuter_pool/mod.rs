mod add_new_assets;
mod exit_pool;
mod has_denom;
mod join_pool;
mod transmute;
mod weight;

use std::collections::HashSet;

use cosmwasm_schema::cw_serde;
use cosmwasm_std::{ensure, Coin, Uint64};

use crate::{denom::Denom, ContractError};

/// Minimum number of pool assets. This is required since if the
/// number of pool assets is less than 2, then the contract will
/// not function as a pool.
const MIN_POOL_ASSET_DENOMS: Uint64 = Uint64::new(2u64);

/// Maximum number of pool assets. This is required in order to
/// prevent the contract from running out of gas when iterating
const MAX_POOL_ASSET_DENOMS: Uint64 = Uint64::new(20u64);

#[cw_serde]
pub struct TransmuterPool {
    /// incoming coins are stored here
    pub pool_assets: Vec<Coin>,
}

impl TransmuterPool {
    pub fn new(pool_asset_denoms: &[Denom]) -> Result<Self, ContractError> {
        let pool = Self {
            pool_assets: pool_asset_denoms
                .iter()
                .map(|denom| Coin::new(0, denom.as_str()))
                .collect(),
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
            let is_new = denoms.insert(asset.denom.as_str());
            ensure!(
                is_new,
                ContractError::DuplicatedPoolAssetDenom {
                    denom: asset.denom.clone(),
                }
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_denom_count_within_range() {
        // 1 denom
        assert_eq!(
            TransmuterPool::new(&[Denom::unchecked("a")]).unwrap_err(),
            ContractError::PoolAssetDenomCountOutOfRange {
                min: MIN_POOL_ASSET_DENOMS,
                max: MAX_POOL_ASSET_DENOMS,
                actual: Uint64::new(1),
            }
        );

        // 2 denoms
        assert_eq!(
            TransmuterPool::new(&[Denom::unchecked("a"), Denom::unchecked("b")]).unwrap(),
            TransmuterPool {
                pool_assets: vec![Coin::new(0, "a"), Coin::new(0, "b"),],
            }
        );

        // 20 denoms
        let denoms: Vec<Denom> = (0..20)
            .map(|i| Denom::unchecked(&format!("d{}", i)))
            .collect();
        assert_eq!(
            TransmuterPool::new(&denoms).unwrap(),
            TransmuterPool {
                pool_assets: denoms
                    .iter()
                    .map(|denom| Coin::new(0, denom.as_str()))
                    .collect(),
            }
        );

        // 21 denoms should fail
        let denoms: Vec<Denom> = (0..21)
            .map(|i| Denom::unchecked(&format!("d{}", i)))
            .collect();
        assert_eq!(
            TransmuterPool::new(&denoms).unwrap_err(),
            ContractError::PoolAssetDenomCountOutOfRange {
                min: MIN_POOL_ASSET_DENOMS,
                max: MAX_POOL_ASSET_DENOMS,
                actual: Uint64::new(21),
            }
        );
    }

    #[test]
    fn test_duplicated_denom() {
        let denoms: Vec<Denom> = vec![Denom::unchecked("a"), Denom::unchecked("a")];
        assert_eq!(
            TransmuterPool::new(&denoms).unwrap_err(),
            ContractError::DuplicatedPoolAssetDenom {
                denom: "a".to_string(),
            }
        );
    }
}
