use cosmwasm_std::{Coin, StdError};

use crate::ContractError;

use super::TransmuterPool;

impl TransmuterPool {
    pub fn join_pool(&mut self, tokens_in: &[Coin]) -> Result<(), ContractError> {
        self.with_corrupted_scopes_protocol(|pool| pool.unchecked_join_pool(tokens_in))
    }

    fn unchecked_join_pool(&mut self, tokens_in: &[Coin]) -> Result<(), ContractError> {
        tokens_in.iter().try_for_each(|token_in| {
            // check if token_in is in pool_assets
            if let Some(pool_asset) = self
                .pool_assets
                .iter_mut()
                .find(|pool_asset| pool_asset.denom() == token_in.denom)
            {
                // add token_in amount to pool_asset
                pool_asset.update_amount(|amount| {
                    amount
                        .checked_add(token_in.amount)
                        .map_err(StdError::overflow)
                        .map_err(ContractError::Std)
                })?;

                Ok(())
            } else {
                // else return InvalidJoinPoolDenom
                Err(ContractError::InvalidJoinPoolDenom {
                    denom: token_in.denom.clone(),
                    expected_denom: self
                        .pool_assets
                        .iter()
                        .map(|pool_asset| pool_asset.denom().to_string())
                        .collect(),
                })
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use cosmwasm_std::{coin, OverflowError, OverflowOperation};

    use crate::asset::Asset;

    use super::*;
    const ETH_USDC: &str = "ibc/AXLETHUSDC";
    const COSMOS_USDC: &str = "ibc/COSMOSUSDC";

    #[test]
    fn test_join_pool_increasingly() {
        let mut pool =
            TransmuterPool::new(Asset::unchecked_equal_assets(&[ETH_USDC, COSMOS_USDC])).unwrap();

        // join pool
        pool.join_pool(&[coin(1000, COSMOS_USDC)]).unwrap();
        assert_eq!(
            pool.pool_assets,
            Asset::unchecked_equal_assets_from_coins(&[coin(0, ETH_USDC), coin(1000, COSMOS_USDC)])
        );

        // join pool when not empty
        pool.join_pool(&[coin(20000, COSMOS_USDC)]).unwrap();
        assert_eq!(
            pool.pool_assets,
            Asset::unchecked_equal_assets_from_coins(&[
                coin(0, ETH_USDC),
                coin(21000, COSMOS_USDC)
            ])
        );

        // join pool multiple tokens at once
        pool.join_pool(&[coin(1000, ETH_USDC), coin(1000, COSMOS_USDC)])
            .unwrap();
        assert_eq!(
            pool.pool_assets,
            Asset::unchecked_equal_assets_from_coins(&[
                coin(1000, ETH_USDC),
                coin(22000, COSMOS_USDC)
            ])
        );
    }

    #[test]
    fn test_join_pool_error_with_wrong_denom() {
        let mut pool =
            TransmuterPool::new(Asset::unchecked_equal_assets(&[ETH_USDC, COSMOS_USDC])).unwrap();

        assert_eq!(
            pool.join_pool(&[coin(1000, "urandom")]).unwrap_err(),
            ContractError::InvalidJoinPoolDenom {
                denom: "urandom".to_string(),
                expected_denom: vec![ETH_USDC.to_string(), COSMOS_USDC.to_string()]
            },
            "join pool with random denom"
        );

        assert_eq!(
            pool.join_pool(&[coin(1000, "urandom"), coin(10000, COSMOS_USDC)])
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
        let mut pool =
            TransmuterPool::new(Asset::unchecked_equal_assets(&[ETH_USDC, COSMOS_USDC])).unwrap();

        assert_eq!(
            {
                pool.join_pool(&[coin(1, COSMOS_USDC)]).unwrap();
                pool.join_pool(&[coin(u128::MAX, COSMOS_USDC)]).unwrap_err()
            },
            ContractError::Std(StdError::overflow(OverflowError::new(
                OverflowOperation::Add
            ))),
            "join pool overflow"
        );
    }
}
