use std::collections::BTreeMap;

use cosmwasm_schema::cw_serde;
use cosmwasm_std::{coin, Coin, Uint128};

use crate::ContractError;

#[cw_serde]
pub struct IncentivePool {
    pub pool: BTreeMap<String, Uint128>,
}

impl IncentivePool {
    pub fn new() -> Self {
        Self {
            pool: BTreeMap::new(),
        }
    }

    /// Collects the given coin into the pool.
    pub fn collect(&mut self, coin: Coin) -> Result<&mut Self, ContractError> {
        if let Some(amount) = self.pool.get_mut(&coin.denom) {
            *amount = amount
                .checked_add(coin.amount)
                .map_err(ContractError::from)?;
        } else {
            self.pool.insert(coin.denom, coin.amount);
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
            let Some(amount) = self.pool.get_mut(&c.denom) else {
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
                self.pool.remove(&c.denom);
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

        // Collect 100 uatom
        pool.collect(coin(100, "uatom")).unwrap();
        assert_eq!(pool.pool.get("uatom"), Some(&Uint128::new(100)));

        // Collect another 50 uatom
        pool.collect(coin(50, "uatom")).unwrap();
        assert_eq!(pool.pool.get("uatom"), Some(&Uint128::new(150)));

        // Collect 200 uluna
        pool.collect(coin(200, "uluna")).unwrap();
        assert_eq!(pool.pool.get("uluna"), Some(&Uint128::new(200)));

        assert_eq!(
            pool.pool,
            BTreeMap::from([
                ("uatom".to_string(), Uint128::new(150)),
                ("uluna".to_string(), Uint128::new(200)),
            ])
        );
    }

    #[test]
    fn test_deduct() {
        let mut pool = IncentivePool::new();

        // Collect some tokens first
        pool.collect(coin(100, "uatom")).unwrap();
        pool.collect(coin(200, "uluna")).unwrap();

        // Deduct 50 uatom
        pool.deduct(vec![coin(50, "uatom")]).unwrap();
        assert_eq!(pool.pool.get("uatom"), Some(&Uint128::new(50)));

        // Deduct another 50 uatom, should be zero now
        pool.deduct(vec![coin(50, "uatom")]).unwrap();
        assert_eq!(pool.pool.get("uatom"), None);

        // Deduct 100 uluna
        pool.deduct(vec![coin(100, "uluna")]).unwrap();
        assert_eq!(pool.pool.get("uluna"), Some(&Uint128::new(100)));

        // Try to deduct more than available, should return an error
        let err = pool.deduct(vec![coin(150, "uluna")]).unwrap_err();
        assert_eq!(
            err,
            ContractError::UnableToDeductFromIncentivePool {
                required: coin(150, "uluna"),
                available: coin(100, "uluna"),
            }
        );
    }
}
