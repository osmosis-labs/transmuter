use cosmwasm_std::{Coin, StdError};

use crate::ContractError;

use super::TransmuterPool;

impl TransmuterPool {
    pub fn join_pool(&mut self, tokens_in: &[Coin]) -> Result<(), ContractError> {
        tokens_in.iter().try_for_each(|token_in| {
            // check if token_in is in pool_assets
            if let Some(pool_asset) = self
                .pool_assets
                .iter_mut()
                .find(|pool_asset| pool_asset.denom == token_in.denom)
            {
                // add token_in amount to pool_asset
                pool_asset.amount = pool_asset
                    .amount
                    .checked_add(token_in.amount)
                    .map_err(StdError::overflow)?;
                Ok(())
            } else {
                // else return InvalidJoinPoolDenom
                Err(ContractError::InvalidJoinPoolDenom {
                    denom: token_in.denom.clone(),
                    expected_denom: self
                        .pool_assets
                        .iter()
                        .map(|pool_asset| pool_asset.denom.clone())
                        .collect(),
                })
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use cosmwasm_std::{OverflowError, OverflowOperation};

    use super::*;
    const ETH_USDC: &str = "ibc/AXLETHUSDC";
    const COSMOS_USDC: &str = "ibc/COSMOSUSDC";

    #[test]
    fn test_join_pool_increasingly() {
        let mut pool = TransmuterPool::new(&[ETH_USDC.to_string(), COSMOS_USDC.to_string()]);

        // join pool
        pool.join_pool(&[Coin::new(1000, COSMOS_USDC)]).unwrap();
        assert_eq!(
            pool.pool_assets,
            vec![Coin::new(0, ETH_USDC), Coin::new(1000, COSMOS_USDC)]
        );

        // join pool when not empty
        pool.join_pool(&[Coin::new(20000, COSMOS_USDC)]).unwrap();
        assert_eq!(
            pool.pool_assets,
            vec![Coin::new(0, ETH_USDC), Coin::new(21000, COSMOS_USDC)]
        );

        // join pool multiple tokens at once
        pool.join_pool(&[Coin::new(1000, ETH_USDC), Coin::new(1000, COSMOS_USDC)])
            .unwrap();
        assert_eq!(
            pool.pool_assets,
            vec![Coin::new(1000, ETH_USDC), Coin::new(22000, COSMOS_USDC)]
        );
    }

    #[test]
    fn test_join_pool_error_with_wrong_denom() {
        let mut pool = TransmuterPool::new(&[ETH_USDC.to_string(), COSMOS_USDC.to_string()]);

        assert_eq!(
            pool.join_pool(&[Coin::new(1000, "urandom")]).unwrap_err(),
            ContractError::InvalidJoinPoolDenom {
                denom: "urandom".to_string(),
                expected_denom: vec![ETH_USDC.to_string(), COSMOS_USDC.to_string()]
            },
            "join pool with random denom"
        );

        assert_eq!(
            pool.join_pool(&[Coin::new(1000, "urandom"), Coin::new(10000, COSMOS_USDC)])
                .unwrap_err(),
            ContractError::InvalidJoinPoolDenom {
                denom: "urandom".to_string(),
                expected_denom: vec![ETH_USDC.to_string(), COSMOS_USDC.to_string()]
            },
            "join pool with random denom"
        );
    }

    #[test]
    fn test_join_pool_error_with_overflow() {
        let mut pool = TransmuterPool::new(&[ETH_USDC.to_string(), COSMOS_USDC.to_string()]);

        assert_eq!(
            {
                pool.join_pool(&[Coin::new(1, COSMOS_USDC)]).unwrap();
                pool.join_pool(&[Coin::new(u128::MAX, COSMOS_USDC)])
                    .unwrap_err()
            },
            ContractError::Std(StdError::Overflow {
                source: OverflowError {
                    operation: OverflowOperation::Add,
                    operand1: 1.to_string(),
                    operand2: u128::MAX.to_string()
                }
            }),
            "join pool overflow"
        );
    }
}
