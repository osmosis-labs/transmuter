use std::collections::BTreeMap;

use cosmwasm_schema::cw_serde;
use cosmwasm_std::{coin, Coin, Uint128};

use crate::ContractError;
// TODO: how to handle corrupted denoms in the incentive pool
#[cw_serde]
#[derive(Default)]
pub struct IncentivePool {
    pub balances: BTreeMap<String, Uint128>,
}

impl IncentivePool {
    pub fn new() -> Self {
        Self {
            balances: BTreeMap::new(),
        }
    }

    /// Collects the given coin into the pool.
    pub fn collect(&mut self, coin: Coin) -> Result<&mut Self, ContractError> {
        if let Some(amount) = self.balances.get_mut(&coin.denom) {
            *amount = amount
                .checked_add(coin.amount)
                .map_err(ContractError::from)?;
        } else {
            self.balances.insert(coin.denom, coin.amount);
        }

        Ok(self)
    }

    /// Deducts the given coin from the pool.
    pub fn deduct(
        &mut self,
        coins: impl IntoIterator<Item = Coin>,
    ) -> Result<&mut Self, ContractError> {
        // deduct the coins from the pool
        for c in coins.into_iter() {
            let Some(amount) = self.balances.get_mut(&c.denom) else {
                return Err(ContractError::UnableToDeductFromIncentivePool {
                    required: c.clone(),
                    available: coin(0, &c.denom),
                });
            };

            *amount = amount.checked_sub(c.amount).map_err(|_| {
                ContractError::UnableToDeductFromIncentivePool {
                    required: c.clone(),
                    available: Coin::new(amount.clone(), &c.denom),
                }
            })?;

            // if the amount is zero, remove the coin from the pool
            if amount.is_zero() {
                self.balances.remove(&c.denom);
            }
        }

        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmwasm_std::{coin, Uint128};

    #[test]
    fn test_collect() {
        let mut pool = IncentivePool::new();

        // Collect 100 uusdt
        pool.collect(coin(100, "uusdt")).unwrap();
        assert_eq!(pool.balances.get("uusdt"), Some(&Uint128::new(100)));

        // Collect another 50 uusdt
        pool.collect(coin(50, "uusdt")).unwrap();
        assert_eq!(pool.balances.get("uusdt"), Some(&Uint128::new(150)));

        // Collect 200 uusdc
        pool.collect(coin(200, "uusdc")).unwrap();
        assert_eq!(pool.balances.get("uusdc"), Some(&Uint128::new(200)));

        assert_eq!(
            pool.balances,
            BTreeMap::from([
                ("uusdt".to_string(), Uint128::new(150)),
                ("uusdc".to_string(), Uint128::new(200)),
            ])
        );
    }

    #[test]
    fn test_deduct() {
        let mut pool = IncentivePool::new();

        // Collect some tokens first
        pool.collect(coin(100, "uusdt")).unwrap();
        pool.collect(coin(200, "uusdc")).unwrap();

        // Deduct 50 uusdt
        pool.deduct(vec![coin(50, "uusdt")]).unwrap();
        assert_eq!(pool.balances.get("uusdt"), Some(&Uint128::new(50)));

        // Deduct another 50 uusdt, should be zero now
        pool.deduct(vec![coin(50, "uusdt")]).unwrap();
        assert_eq!(pool.balances.get("uusdt"), None);

        // Deduct 100 uusdc
        pool.deduct(vec![coin(100, "uusdc")]).unwrap();
        assert_eq!(pool.balances.get("uusdc"), Some(&Uint128::new(100)));

        // Try to deduct more than available, should return an error
        let err = pool.deduct(vec![coin(150, "uusdc")]).unwrap_err();
        assert_eq!(
            err,
            ContractError::UnableToDeductFromIncentivePool {
                required: coin(150, "uusdc"),
                available: coin(100, "uusdc"),
            }
        );
    }
}
