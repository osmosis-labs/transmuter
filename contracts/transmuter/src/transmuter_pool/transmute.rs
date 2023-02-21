use cosmwasm_std::{ensure, ensure_eq, Coin, StdError};

use crate::ContractError;

use super::TransmuterPool;

impl TransmuterPool {
    pub fn transmute(&mut self, coin: &Coin) -> Result<Coin, ContractError> {
        let pool_assets = &mut self.pool_assets;

        // out coin has 1:1 amount ratio with in coin
        let out_coin = Coin::new(coin.amount.into(), pool_assets[1].denom.clone());

        // ensure transmute denom is in_coin's denom
        ensure_eq!(
            coin.denom,
            pool_assets[0].denom,
            ContractError::InvalidTransmuteDenom {
                denom: coin.denom.clone(),
                expected_denom: pool_assets[0].denom.clone()
            }
        );

        // ensure there is enough out_coin_reserve
        ensure!(
            pool_assets[1].amount >= coin.amount,
            ContractError::InsufficientFund {
                required: out_coin,
                available: pool_assets[1].clone()
            }
        );

        // increase in_coin
        pool_assets[0].amount = pool_assets[0]
            .amount
            .checked_add(coin.amount)
            .map_err(StdError::overflow)?;

        // deduct out_coin_reserve
        pool_assets[1].amount = pool_assets[1]
            .amount
            .checked_sub(coin.amount)
            .map_err(StdError::overflow)?;

        Ok(out_coin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const ETH_USDC: &str = "ibc/AXLETHUSDC";
    const COSMOS_USDC: &str = "ibc/COSMOSUSDC";

    #[test]
    fn test_transmute_succeed() {
        let mut pool = TransmuterPool::new(ETH_USDC, COSMOS_USDC);

        pool.supply(&Coin::new(70_000, COSMOS_USDC)).unwrap();
        assert_eq!(
            pool.transmute(&Coin::new(70_000, ETH_USDC)).unwrap(),
            Coin::new(70_000, COSMOS_USDC)
        );

        pool.supply(&Coin::new(100_000, COSMOS_USDC)).unwrap();
        assert_eq!(
            pool.transmute(&Coin::new(60_000, ETH_USDC)).unwrap(),
            Coin::new(60_000, COSMOS_USDC)
        );
        assert_eq!(
            pool.transmute(&Coin::new(20_000, ETH_USDC)).unwrap(),
            Coin::new(20_000, COSMOS_USDC)
        );
        assert_eq!(
            pool.transmute(&Coin::new(20_000, ETH_USDC)).unwrap(),
            Coin::new(20_000, COSMOS_USDC)
        );
        assert_eq!(
            pool.transmute(&Coin::new(0, ETH_USDC)).unwrap(),
            Coin::new(0, COSMOS_USDC)
        );
    }

    #[test]
    fn test_transmute_fail_out_coin_not_enough() {
        let mut pool = TransmuterPool::new(ETH_USDC, COSMOS_USDC);

        pool.supply(&Coin::new(70_000, COSMOS_USDC)).unwrap();
        assert_eq!(
            pool.transmute(&Coin::new(70_001, ETH_USDC)).unwrap_err(),
            ContractError::InsufficientFund {
                required: Coin::new(70_001, COSMOS_USDC),
                available: Coin::new(70_000, COSMOS_USDC)
            }
        );
    }

    #[test]
    fn test_transmute_fail_in_coin_not_allowed() {
        let mut pool = TransmuterPool::new(ETH_USDC, COSMOS_USDC);

        pool.supply(&Coin::new(70_000, COSMOS_USDC)).unwrap();
        assert_eq!(
            pool.transmute(&Coin::new(70_000, "ibc/AXLETHUSDT"))
                .unwrap_err(),
            ContractError::InvalidTransmuteDenom {
                denom: "ibc/AXLETHUSDT".to_string(),
                expected_denom: ETH_USDC.to_string()
            }
        );
    }
}
