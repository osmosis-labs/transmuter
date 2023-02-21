use cosmwasm_std::{ensure, Coin, StdError};

use crate::ContractError;

use super::TransmuterPool;

impl TransmuterPool {
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
                .find(|pool_asset| pool_asset.denom == denom)
        };

        let token_in_pool_asset = pool_asset_by_denom(&token_in.denom);
        let token_out_pool_asset = pool_asset_by_denom(token_out_denom);

        let pool_asset_denoms = self
            .pool_assets
            .iter()
            .map(|pool_asset| pool_asset.denom.clone())
            .collect();

        ensure!(
            token_in_pool_asset.is_some(),
            ContractError::InvalidTransmuteDenom {
                denom: token_in.denom.clone(),
                expected_denom: pool_asset_denoms
            }
        );

        ensure!(
            token_out_pool_asset.is_some(),
            ContractError::InvalidTransmuteDenom {
                denom: token_out_denom.to_string(),
                expected_denom: pool_asset_denoms
            }
        );

        // ensure there is enough token_out_denom in the pool
        let token_out_pool_asset = token_out_pool_asset.expect("already ensured it exists");
        ensure!(
            token_out_pool_asset.amount >= token_in.amount,
            ContractError::InsufficientFund {
                required: token_out,
                available: token_out_pool_asset.clone()
            }
        );

        for pool_asset in &mut self.pool_assets {
            // increase token in from pool assets
            if token_in.denom == pool_asset.denom {
                pool_asset.amount = pool_asset
                    .amount
                    .checked_add(token_in.amount)
                    .map_err(StdError::overflow)?;
            }

            // decrease token out from pool assets
            if token_out.denom == pool_asset.denom {
                pool_asset.amount = pool_asset
                    .amount
                    .checked_sub(token_in.amount)
                    .map_err(StdError::overflow)?;
            }
        }

        Ok(token_out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const ETH_USDC: &str = "ibc/AXLETHUSDC";
    const COSMOS_USDC: &str = "ibc/COSMOSUSDC";

    #[test]
    fn test_transmute_succeed() {
        let mut pool = TransmuterPool::new(&[ETH_USDC.to_string(), COSMOS_USDC.to_string()]);

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
            vec![Coin::new(170_000, ETH_USDC), Coin::new(0, COSMOS_USDC),]
        );

        assert_eq!(
            pool.transmute(&Coin::new(100_000, COSMOS_USDC), ETH_USDC)
                .unwrap(),
            Coin::new(100_000, ETH_USDC)
        );

        assert_eq!(
            pool.pool_assets,
            vec![Coin::new(70_000, ETH_USDC), Coin::new(100_000, COSMOS_USDC)]
        );
    }

    #[test]
    fn test_transmute_token_out_denom_eq_token_in_denom() {
        let mut pool = TransmuterPool::new(&[ETH_USDC.to_string(), COSMOS_USDC.to_string()]);
        pool.join_pool(&[Coin::new(70_000, COSMOS_USDC)]).unwrap();

        let token_in = Coin::new(70_000, COSMOS_USDC);
        assert_eq!(pool.transmute(&token_in, COSMOS_USDC).unwrap(), token_in);
    }

    #[test]
    fn test_transmute_fail_token_out_not_enough() {
        let mut pool = TransmuterPool::new(&[ETH_USDC.to_string(), COSMOS_USDC.to_string()]);

        pool.join_pool(&[Coin::new(70_000, COSMOS_USDC)]).unwrap();
        assert_eq!(
            pool.transmute(&Coin::new(70_001, ETH_USDC), COSMOS_USDC)
                .unwrap_err(),
            ContractError::InsufficientFund {
                required: Coin::new(70_001, COSMOS_USDC),
                available: Coin::new(70_000, COSMOS_USDC)
            }
        );
    }

    #[test]
    fn test_transmute_fail_token_in_not_allowed() {
        let mut pool = TransmuterPool::new(&[ETH_USDC.to_string(), COSMOS_USDC.to_string()]);

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
        let mut pool = TransmuterPool::new(&[ETH_USDC.to_string(), COSMOS_USDC.to_string()]);

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
