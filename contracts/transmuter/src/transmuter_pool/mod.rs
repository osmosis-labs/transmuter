mod exit_pool;
mod join_pool;
mod transmute;
mod weight;
mod has_denom;

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
        let count = Uint64::new(pool_asset_denoms.len() as u64);

        ensure!(
            count >= MIN_POOL_ASSET_DENOMS && count <= MAX_POOL_ASSET_DENOMS,
            ContractError::PoolAssetDenomCountOutOfRange {
                min: MIN_POOL_ASSET_DENOMS,
                max: MAX_POOL_ASSET_DENOMS,
                actual: count
            }
        );

        Ok(Self {
            pool_assets: pool_asset_denoms
                .iter()
                .map(|denom| Coin::new(0, denom.as_str()))
                .collect(),
        })
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
}
