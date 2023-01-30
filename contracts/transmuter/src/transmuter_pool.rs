use cosmwasm_std::{ensure_eq, Coin, StdError};

use crate::ContractError;

pub struct TransmuterPool {
    /// incoming coins are stored here
    in_coin: Coin,
    /// reserve of coins for future transmutations
    out_coin_reserve: Coin,
}

impl TransmuterPool {
    pub fn new(in_denom: &str, out_denom: &str) -> Self {
        Self {
            in_coin: Coin::new(0, in_denom),
            out_coin_reserve: Coin::new(0, out_denom),
        }
    }

    pub fn supply(&mut self, coin: &Coin) -> Result<(), ContractError> {
        // ensure supply denom is out_coin_reserve's denom
        ensure_eq!(
            coin.denom,
            self.out_coin_reserve.denom,
            ContractError::UnableToSupply {
                denom: coin.denom.clone(),
                expected_denom: self.out_coin_reserve.denom.clone()
            }
        );

        self.out_coin_reserve.amount = self
            .out_coin_reserve
            .amount
            .checked_add(coin.amount)
            .map_err(StdError::overflow)?;

        Ok(())
    }

    pub fn transmute(&mut self, coin: &Coin) -> Result<Coin, ContractError> {
        todo!()
    }

    pub fn withdraw(&mut self, coins: &[Coin]) -> Result<(), ContractError> {
        todo!()
    }
}

#[cfg(test)]
mod test {
    use cosmwasm_std::{OverflowError, OverflowOperation};

    use super::*;
    const ETH_USDC: &str = "ibc/AXLETHUSDC";
    const COSMOS_USDC: &str = "ibc/COSMOSUSDC";

    #[test]
    fn test_supply_ok() {
        let mut pool = TransmuterPool::new(ETH_USDC, COSMOS_USDC);

        // supply with out coin denom
        pool.supply(&Coin::new(1000, COSMOS_USDC)).unwrap();
        assert_eq!(pool.out_coin_reserve, Coin::new(1000, COSMOS_USDC));

        // supply with out coin denom when not empty
        pool.supply(&Coin::new(20000, COSMOS_USDC)).unwrap();
        assert_eq!(pool.out_coin_reserve, Coin::new(21000, COSMOS_USDC));
    }

    #[test]
    fn test_supply_error() {
        let mut pool = TransmuterPool::new(ETH_USDC, COSMOS_USDC);

        assert_eq!(
            pool.supply(&Coin::new(1000, ETH_USDC)).unwrap_err(),
            ContractError::UnableToSupply {
                denom: ETH_USDC.to_string(),
                expected_denom: COSMOS_USDC.to_string()
            },
            "supply with in_coin denom"
        );

        assert_eq!(
            pool.supply(&Coin::new(1000, "urandom")).unwrap_err(),
            ContractError::UnableToSupply {
                denom: "urandom".to_string(),
                expected_denom: COSMOS_USDC.to_string()
            },
            "supply with random denom"
        );

        assert_eq!(
            {
                pool.supply(&Coin::new(1, COSMOS_USDC)).unwrap();
                pool.supply(&Coin::new(u128::MAX, COSMOS_USDC)).unwrap_err()
            },
            ContractError::Std(StdError::Overflow {
                source: OverflowError {
                    operation: OverflowOperation::Add,
                    operand1: 1.to_string(),
                    operand2: u128::MAX.to_string()
                }
            }),
            "supply overflow"
        );
    }
}
