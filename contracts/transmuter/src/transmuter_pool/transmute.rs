use cosmwasm_std::{ensure, Coin, StdError};

use crate::ContractError;

use super::TransmuterPool;

impl TransmuterPool {
    // TODO: take normalization factor into account to how much the resulted token_out will be
    pub fn transmute(
        &mut self,
        token_in: &Coin,
        token_out_denom: &str,
    ) -> Result<Coin, ContractError> {
        // token out has 1:1 amount ratio with token in
        let token_out = Coin::new(token_in.amount.into(), token_out_denom);

        // ensure transmuting denom is one of the pool assets
        let pool_asset_by_denom = |denom: &str| {
            self.pool_assets
                .iter()
                .find(|pool_asset| pool_asset.denom() == denom)
        };

        // get all pool asset denoms
        let pool_asset_denoms: Vec<String> = self
            .pool_assets
            .iter()
            .map(|pool_asset| pool_asset.denom().to_string())
            .collect();

        // check if token_in is in pool_assets
        let _token_in_pool_asset = pool_asset_by_denom(&token_in.denom).ok_or_else(|| {
            ContractError::InvalidTransmuteDenom {
                denom: token_in.denom.clone(),
                expected_denom: pool_asset_denoms.clone(),
            }
        })?;

        // check if token_out_denom is in pool_assets
        let token_out_pool_asset = pool_asset_by_denom(token_out_denom).ok_or_else(|| {
            ContractError::InvalidTransmuteDenom {
                denom: token_out_denom.to_string(),
                expected_denom: pool_asset_denoms,
            }
        })?;

        // ensure there is enough token_out_denom in the pool
        ensure!(
            token_out_pool_asset.amount() >= token_in.amount,
            ContractError::InsufficientPoolAsset {
                required: token_out,
                available: token_out_pool_asset.to_coin()
            }
        );

        for pool_asset in &mut self.pool_assets {
            // increase token in from pool assets
            if token_in.denom == pool_asset.denom() {
                pool_asset.update_amount(|amount| {
                    amount
                        .checked_add(token_in.amount)
                        .map_err(StdError::overflow)
                        .map_err(ContractError::Std)
                })?;
            }

            // decrease token out from pool assets
            if token_out.denom == pool_asset.denom() {
                pool_asset.update_amount(|amount| {
                    amount
                        .checked_sub(token_in.amount)
                        .map_err(StdError::overflow)
                        .map_err(ContractError::Std)
                })?;
            }
        }

        Ok(token_out)
    }
}

#[cfg(test)]
mod tests {
    use crate::asset::Asset;

    use super::*;
    const ETH_USDC: &str = "ibc/AXLETHUSDC";
    const COSMOS_USDC: &str = "ibc/COSMOSUSDC";

    #[test]
    fn test_transmute_succeed() {
        let mut pool =
            TransmuterPool::new(Asset::unchecked_equal_assets(&[ETH_USDC, COSMOS_USDC])).unwrap();

        pool.join_pool(&[Coin::new(70_000, COSMOS_USDC)]).unwrap();
        assert_eq!(
            pool.transmute(&Coin::new(70_000, ETH_USDC), COSMOS_USDC)
                .unwrap(),
            Coin::new(70_000, COSMOS_USDC)
        );

        pool.join_pool(&[Coin::new(100_000, COSMOS_USDC)]).unwrap();
        assert_eq!(
            pool.transmute(&Coin::new(60_000, ETH_USDC), COSMOS_USDC)
                .unwrap(),
            Coin::new(60_000, COSMOS_USDC)
        );
        assert_eq!(
            pool.transmute(&Coin::new(20_000, ETH_USDC), COSMOS_USDC)
                .unwrap(),
            Coin::new(20_000, COSMOS_USDC)
        );
        assert_eq!(
            pool.transmute(&Coin::new(20_000, ETH_USDC), COSMOS_USDC)
                .unwrap(),
            Coin::new(20_000, COSMOS_USDC)
        );
        assert_eq!(
            pool.transmute(&Coin::new(0, ETH_USDC), COSMOS_USDC)
                .unwrap(),
            Coin::new(0, COSMOS_USDC)
        );

        assert_eq!(
            pool.pool_assets,
            Asset::unchecked_equal_assets_from_coins(&[
                Coin::new(170_000, ETH_USDC),
                Coin::new(0, COSMOS_USDC),
            ])
        );

        assert_eq!(
            pool.transmute(&Coin::new(100_000, COSMOS_USDC), ETH_USDC)
                .unwrap(),
            Coin::new(100_000, ETH_USDC)
        );

        assert_eq!(
            pool.pool_assets,
            Asset::unchecked_equal_assets_from_coins(&[
                Coin::new(70_000, ETH_USDC),
                Coin::new(100_000, COSMOS_USDC)
            ])
        );
    }

    #[test]
    fn test_transmute_token_out_denom_eq_token_in_denom() {
        let mut pool =
            TransmuterPool::new(Asset::unchecked_equal_assets(&[ETH_USDC, COSMOS_USDC])).unwrap();

        pool.join_pool(&[Coin::new(70_000, COSMOS_USDC)]).unwrap();

        let token_in = Coin::new(70_000, COSMOS_USDC);
        assert_eq!(pool.transmute(&token_in, COSMOS_USDC).unwrap(), token_in);
    }

    #[test]
    fn test_transmute_fail_token_out_not_enough() {
        let mut pool =
            TransmuterPool::new(Asset::unchecked_equal_assets(&[ETH_USDC, COSMOS_USDC])).unwrap();

        pool.join_pool(&[Coin::new(70_000, COSMOS_USDC)]).unwrap();
        assert_eq!(
            pool.transmute(&Coin::new(70_001, ETH_USDC), COSMOS_USDC)
                .unwrap_err(),
            ContractError::InsufficientPoolAsset {
                required: Coin::new(70_001, COSMOS_USDC),
                available: Coin::new(70_000, COSMOS_USDC)
            }
        );
    }

    #[test]
    fn test_transmute_fail_token_in_not_allowed() {
        let mut pool =
            TransmuterPool::new(Asset::unchecked_equal_assets(&[ETH_USDC, COSMOS_USDC])).unwrap();

        pool.join_pool(&[Coin::new(70_000, COSMOS_USDC)]).unwrap();
        assert_eq!(
            pool.transmute(&Coin::new(70_000, "urandom"), COSMOS_USDC)
                .unwrap_err(),
            ContractError::InvalidTransmuteDenom {
                denom: "urandom".to_string(),
                expected_denom: vec![ETH_USDC.to_string(), COSMOS_USDC.to_string()]
            }
        );
    }

    #[test]
    fn test_transmute_fail_token_out_denom_not_allowed() {
        let mut pool =
            TransmuterPool::new(Asset::unchecked_equal_assets(&[ETH_USDC, COSMOS_USDC])).unwrap();

        pool.join_pool(&[Coin::new(70_000, COSMOS_USDC)]).unwrap();
        assert_eq!(
            pool.transmute(&Coin::new(70_000, COSMOS_USDC), "urandom2")
                .unwrap_err(),
            ContractError::InvalidTransmuteDenom {
                denom: "urandom2".to_string(),
                expected_denom: vec![ETH_USDC.to_string(), COSMOS_USDC.to_string()]
            }
        );
    }
}
