use cosmwasm_std::{ensure, ensure_eq, Coin, StdError};

use crate::ContractError;

use super::TransmuterPool;

impl TransmuterPool {
    pub fn transmute(&mut self, coin: &Coin) -> Result<Coin, ContractError> {
        // out coin has 1:1 amount ratio with in coin
        let out_coin = Coin::new(coin.amount.into(), self.out_coin_reserve.denom.clone());

        // ensure transmute denom is in_coin's denom
        ensure_eq!(
            coin.denom,
            self.in_coin.denom,
            ContractError::InvalidTransmuteDenom {
                denom: coin.denom.clone(),
                expected_denom: self.in_coin.denom.clone()
            }
        );

        // ensure there is enough out_coin_reserve
        ensure!(
            self.out_coin_reserve.amount >= coin.amount,
            ContractError::InsufficientFund {
                required: out_coin,
                available: self.out_coin_reserve.clone()
            }
        );

        // increase in_coin
        self.in_coin.amount = self
            .in_coin
            .amount
            .checked_add(coin.amount)
            .map_err(StdError::overflow)?;

        // deduct out_coin_reserve
        self.out_coin_reserve.amount = self
            .out_coin_reserve
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
