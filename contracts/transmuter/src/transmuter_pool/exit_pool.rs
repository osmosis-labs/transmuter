use cosmwasm_std::Coin;

use crate::ContractError;

use super::TransmuterPool;

impl TransmuterPool {
    pub fn exit_pool(&mut self, tokens_out: &[Coin]) -> Result<(), ContractError> {
        for token in tokens_out {
            let token_is_in_pool_assets = self
                .pool_assets
                .iter()
                .any(|pool_asset| pool_asset.denom == token.denom);

            if token_is_in_pool_assets {
                for pool_asset in &mut self.pool_assets {
                    // deduct token from pool assets
                    if token.denom == pool_asset.denom {
                        pool_asset.amount =
                            pool_asset.amount.checked_sub(token.amount).map_err(|_| {
                                ContractError::InsufficientFund {
                                    required: token.clone(),
                                    available: pool_asset.clone(),
                                }
                            })?;
                    }
                }
            } else {
                return Err(ContractError::InsufficientFund {
                    required: token.clone(),
                    available: Coin::new(0, token.denom.clone()),
                });
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const ETH_USDC: &str = "ibc/AXLETHUSDC";
    const COSMOS_USDC: &str = "ibc/COSMOSUSDC";

    #[test]
    fn test_exit_pool_succeed_when_has_enough_coins_in_pool() {
        let mut pool = TransmuterPool {
            pool_assets: vec![
                Coin::new(100_000, ETH_USDC),
                Coin::new(100_000, COSMOS_USDC),
            ],
        };

        // exit pool with first token
        pool.exit_pool(&[Coin::new(10_000, ETH_USDC)]).unwrap();
        assert_eq!(
            pool,
            TransmuterPool {
                pool_assets: vec![Coin::new(90_000, ETH_USDC), Coin::new(100_000, COSMOS_USDC),],
            }
        );

        // exit pool with second token
        pool.exit_pool(&[Coin::new(10_000, COSMOS_USDC)]).unwrap();
        assert_eq!(
            pool,
            TransmuterPool {
                pool_assets: vec![Coin::new(90_000, ETH_USDC), Coin::new(90_000, COSMOS_USDC),],
            }
        );

        // exit pool with both tokens
        pool.exit_pool(&[Coin::new(90_000, ETH_USDC), Coin::new(90_000, COSMOS_USDC)])
            .unwrap();
        assert_eq!(
            pool,
            TransmuterPool {
                pool_assets: vec![Coin::new(0, ETH_USDC), Coin::new(0, COSMOS_USDC),],
            }
        );
    }

    #[test]
    fn test_exit_pool_fail_when_token_denom_is_invalid() {
        let mut pool = TransmuterPool {
            pool_assets: vec![
                Coin::new(100_000, ETH_USDC),
                Coin::new(100_000, COSMOS_USDC),
            ],
        };

        // exit pool with invalid token
        let err = pool.exit_pool(&[Coin::new(10_000, "invalid")]).unwrap_err();
        assert_eq!(
            err,
            ContractError::InsufficientFund {
                required: Coin::new(10_000, "invalid"),
                available: Coin::new(0, "invalid")
            }
        );

        // exit pool with both valid and invalid token
        let err = pool
            .exit_pool(&[Coin::new(10_000, ETH_USDC), Coin::new(10_000, "invalid")])
            .unwrap_err();
        assert_eq!(
            err,
            ContractError::InsufficientFund {
                required: Coin::new(10_000, "invalid"),
                available: Coin::new(0, "invalid")
            }
        );
    }

    #[test]
    fn test_exit_pool_fail_when_not_enough_token() {
        let mut pool = TransmuterPool {
            pool_assets: vec![
                Coin::new(100_000, ETH_USDC),
                Coin::new(100_000, COSMOS_USDC),
            ],
        };

        let err = pool.exit_pool(&[Coin::new(100_001, ETH_USDC)]).unwrap_err();
        assert_eq!(
            err,
            ContractError::InsufficientFund {
                required: Coin::new(100_001, ETH_USDC),
                available: Coin::new(100_000, ETH_USDC)
            }
        );

        let err = pool
            .exit_pool(&[Coin::new(110_000, COSMOS_USDC)])
            .unwrap_err();

        assert_eq!(
            err,
            ContractError::InsufficientFund {
                required: Coin::new(110_000, COSMOS_USDC),
                available: Coin::new(100_000, COSMOS_USDC)
            }
        );

        let err = pool
            .exit_pool(&[
                Coin::new(110_000, ETH_USDC),
                Coin::new(110_000, COSMOS_USDC),
            ])
            .unwrap_err();

        assert_eq!(
            err,
            ContractError::InsufficientFund {
                required: Coin::new(110_000, ETH_USDC),
                available: Coin::new(100_000, ETH_USDC)
            }
        );
    }
}
