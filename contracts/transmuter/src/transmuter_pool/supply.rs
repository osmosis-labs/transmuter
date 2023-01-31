use cosmwasm_std::{ensure_eq, Coin, StdError};

use crate::ContractError;

use super::TransmuterPool;

impl TransmuterPool {
    pub fn supply(&mut self, coin: &Coin) -> Result<(), ContractError> {
        // ensure supply denom is out_coin_reserve's denom
        ensure_eq!(
            coin.denom,
            self.out_coin_reserve.denom,
            ContractError::InvalidSupplyDenom {
                denom: coin.denom.clone(),
                expected_denom: self.out_coin_reserve.denom.clone()
            }
        );

        // add coin to out_coin_reserve
        self.out_coin_reserve.amount = self
            .out_coin_reserve
            .amount
            .checked_add(coin.amount)
            .map_err(StdError::overflow)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use cosmwasm_std::{OverflowError, OverflowOperation};

    use super::*;
    const ETH_USDC: &str = "ibc/AXLETHUSDC";
    const COSMOS_USDC: &str = "ibc/COSMOSUSDC";

    #[test]
    fn test_supply_increasingly() {
        let mut pool = TransmuterPool::new(ETH_USDC, COSMOS_USDC);

        // supply with out coin denom
        pool.supply(&Coin::new(1000, COSMOS_USDC)).unwrap();
        assert_eq!(pool.out_coin_reserve, Coin::new(1000, COSMOS_USDC));

        // supply with out coin denom when not empty
        pool.supply(&Coin::new(20000, COSMOS_USDC)).unwrap();
        assert_eq!(pool.out_coin_reserve, Coin::new(21000, COSMOS_USDC));
    }

    #[test]
    fn test_supply_error_with_wrong_denom() {
        let mut pool = TransmuterPool::new(ETH_USDC, COSMOS_USDC);

        assert_eq!(
            pool.supply(&Coin::new(1000, ETH_USDC)).unwrap_err(),
            ContractError::InvalidSupplyDenom {
                denom: ETH_USDC.to_string(),
                expected_denom: COSMOS_USDC.to_string()
            },
            "supply with in_coin denom"
        );

        assert_eq!(
            pool.supply(&Coin::new(1000, "urandom")).unwrap_err(),
            ContractError::InvalidSupplyDenom {
                denom: "urandom".to_string(),
                expected_denom: COSMOS_USDC.to_string()
            },
            "supply with random denom"
        );
    }

    #[test]
    fn test_supply_error_with_overflow() {
        let mut pool = TransmuterPool::new(ETH_USDC, COSMOS_USDC);

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
