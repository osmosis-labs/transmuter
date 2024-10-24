use cosmwasm_std::Coin;

use crate::ContractError;

use super::TransmuterPool;

impl TransmuterPool {
    pub fn exit_pool(&mut self, tokens_out: &[Coin]) -> Result<(), ContractError> {
        self.with_corrupted_scopes_protocol(|pool| pool.unchecked_exit_pool(tokens_out))
    }

    pub fn unchecked_exit_pool(&mut self, tokens_out: &[Coin]) -> Result<(), ContractError> {
        for token in tokens_out {
            let token_is_in_pool_assets = self
                .pool_assets
                .iter()
                .any(|pool_asset| pool_asset.denom() == token.denom);

            if token_is_in_pool_assets {
                for pool_asset in &mut self.pool_assets {
                    // deduct token from pool assets
                    if token.denom == pool_asset.denom() {
                        let available: Coin = pool_asset.to_coin();
                        pool_asset.update_amount(|amount| {
                            amount.checked_sub(token.amount).map_err(|_| {
                                ContractError::InsufficientPoolAsset {
                                    required: token.clone(),
                                    available,
                                }
                            })
                        })?;
                    }
                }
            } else {
                return Err(ContractError::InvalidPoolAssetDenom {
                    denom: token.denom.clone(),
                });
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use cosmwasm_std::coin;

    use crate::asset::Asset;

    use super::*;
    const ETH_USDC: &str = "ibc/AXLETHUSDC";
    const COSMOS_USDC: &str = "ibc/COSMOSUSDC";

    #[test]
    fn test_exit_pool_succeed_when_has_enough_coins_in_pool() {
        let mut pool = TransmuterPool {
            pool_assets: Asset::unchecked_equal_assets_from_coins(&[
                coin(100_000, ETH_USDC),
                coin(100_000, COSMOS_USDC),
            ]),
            asset_groups: BTreeMap::new(),
        };

        // exit pool with first token
        pool.exit_pool(&[coin(10_000, ETH_USDC)]).unwrap();
        assert_eq!(
            pool,
            TransmuterPool {
                pool_assets: Asset::unchecked_equal_assets_from_coins(&[
                    coin(90_000, ETH_USDC),
                    coin(100_000, COSMOS_USDC),
                ]),
                asset_groups: BTreeMap::new(),
            }
        );

        // exit pool with second token
        pool.exit_pool(&[coin(10_000, COSMOS_USDC)]).unwrap();
        assert_eq!(
            pool,
            TransmuterPool {
                pool_assets: Asset::unchecked_equal_assets_from_coins(&[
                    coin(90_000, ETH_USDC),
                    coin(90_000, COSMOS_USDC),
                ]),
                asset_groups: BTreeMap::new(),
            }
        );

        // exit pool with both tokens
        pool.exit_pool(&[coin(90_000, ETH_USDC), coin(90_000, COSMOS_USDC)])
            .unwrap();
        assert_eq!(
            pool,
            TransmuterPool {
                pool_assets: Asset::unchecked_equal_assets_from_coins(&[
                    coin(0, ETH_USDC),
                    coin(0, COSMOS_USDC),
                ]),
                asset_groups: BTreeMap::new(),
            }
        );
    }

    #[test]
    fn test_exit_pool_fail_when_token_denom_is_invalid() {
        let mut pool = TransmuterPool {
            pool_assets: Asset::unchecked_equal_assets_from_coins(&[
                coin(100_000, ETH_USDC),
                coin(100_000, COSMOS_USDC),
            ]),
            asset_groups: BTreeMap::new(),
        };

        // exit pool with invalid token
        let err = pool.exit_pool(&[coin(10_000, "invalid")]).unwrap_err();
        assert_eq!(
            err,
            ContractError::InvalidPoolAssetDenom {
                denom: "invalid".to_string()
            }
        );

        // exit pool with both valid and invalid token
        let err = pool
            .exit_pool(&[coin(10_000, ETH_USDC), coin(10_000, "invalid2")])
            .unwrap_err();
        assert_eq!(
            err,
            ContractError::InvalidPoolAssetDenom {
                denom: "invalid2".to_string()
            }
        );
    }

    #[test]
    fn test_exit_pool_fail_when_not_enough_token() {
        let mut pool = TransmuterPool {
            pool_assets: Asset::unchecked_equal_assets_from_coins(&[
                coin(100_000, ETH_USDC),
                coin(100_000, COSMOS_USDC),
            ]),
            asset_groups: BTreeMap::new(),
        };

        let err = pool.exit_pool(&[coin(100_001, ETH_USDC)]).unwrap_err();
        assert_eq!(
            err,
            ContractError::InsufficientPoolAsset {
                required: coin(100_001, ETH_USDC),
                available: coin(100_000, ETH_USDC)
            }
        );

        let err = pool.exit_pool(&[coin(110_000, COSMOS_USDC)]).unwrap_err();

        assert_eq!(
            err,
            ContractError::InsufficientPoolAsset {
                required: coin(110_000, COSMOS_USDC),
                available: coin(100_000, COSMOS_USDC)
            }
        );

        let err = pool
            .exit_pool(&[coin(110_000, ETH_USDC), coin(110_000, COSMOS_USDC)])
            .unwrap_err();

        assert_eq!(
            err,
            ContractError::InsufficientPoolAsset {
                required: coin(110_000, ETH_USDC),
                available: coin(100_000, ETH_USDC)
            }
        );
    }
}
