use std::collections::BTreeMap;

use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Coin, OverflowError, Uint128, Uint256};

use crate::{coin256::Coin256, ContractError};

#[derive(Debug, thiserror::Error)]
#[error("Not enough balance to deduct: available {available}")]
struct NotEnoughBalanceError {
    available: Uint256,
}

/// Incentive pool balance is the balance of collected fees from the transmuter for a given denom.
///
/// Split into two parts `historical_lambda_collected_balance` and `current_lambda_collected_balance`
/// to track the fees collected with different lambda values since it effects incentive calculation.
#[cw_serde]
#[derive(Default)]
pub struct IncentivePoolBalance {
    pub historical_lambda_collected_balance: Uint256,
    pub current_lambda_collected_balance: Uint256,
}

impl IncentivePoolBalance {
    pub fn new(amount: impl Into<Uint256>) -> Self {
        Self {
            historical_lambda_collected_balance: Uint256::zero(),
            current_lambda_collected_balance: amount.into(),
        }
    }

    /// Collects the given amount to the current lambda pool.
    fn collect(&mut self, amount: Uint128) -> Result<&mut Self, OverflowError> {
        self.current_lambda_collected_balance = self
            .current_lambda_collected_balance
            .checked_add(amount.into())?;
        Ok(self)
    }

    /// Deducts the given amount from the current lambda pool.
    fn deduct(&mut self, amount: Uint128) -> Result<&mut Self, NotEnoughBalanceError> {
        let available_balance = self
            .historical_lambda_collected_balance
            .saturating_add(self.current_lambda_collected_balance);

        // Deduct from historical lambda collected balance until it's zero first
        let amount_after_deducted_from_historical_balance =
            Uint256::from(amount).saturating_sub(self.historical_lambda_collected_balance);

        self.historical_lambda_collected_balance = self
            .historical_lambda_collected_balance
            .saturating_sub(amount.into());

        // If there is still amount to deduct, deduct from current lambda collected balance
        if amount_after_deducted_from_historical_balance > Uint256::zero() {
            self.current_lambda_collected_balance = self
                .current_lambda_collected_balance
                .checked_sub(amount_after_deducted_from_historical_balance)
                .map_err(|_| NotEnoughBalanceError {
                    available: available_balance,
                })?;
        }
        Ok(self)
    }

    fn is_zero(&self) -> bool {
        self.historical_lambda_collected_balance.is_zero()
            && self.current_lambda_collected_balance.is_zero()
    }
}

/// Incentive pool is the pool of collected rebalancing fees from the transmuter
/// and used for distributing incentives to actors who shift the balances to the favorable range.
#[cw_serde]
#[derive(Default)]
pub struct IncentivePool {
    pub balances: BTreeMap<String, IncentivePoolBalance>,
}

impl IncentivePool {
    pub fn new() -> Self {
        Self {
            balances: BTreeMap::new(),
        }
    }

    /// Collects the given coin into the pool.
    pub fn collect(&mut self, coin: Coin) -> Result<&mut Self, ContractError> {
        if let Some(collected_fees) = self.balances.get_mut(&coin.denom) {
            collected_fees.collect(coin.amount.into())?;
        } else {
            let mut new_collected_fees = IncentivePoolBalance::default();
            new_collected_fees.collect(coin.amount.into())?;
            self.balances.insert(coin.denom, new_collected_fees);
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
            let Some(collected_fees) = self.balances.get_mut(&c.denom) else {
                return Err(ContractError::UnableToDeductFromIncentivePool {
                    required: c.clone().into(),
                    available: Coin256::zero(&c.denom),
                });
            };

            // try to deduct the coin from the pool
            if let Err(NotEnoughBalanceError { available }) = collected_fees.deduct(c.amount.into())
            {
                return Err(ContractError::UnableToDeductFromIncentivePool {
                    required: c.clone().into(),
                    available: Coin256::new(available, &c.denom),
                });
            }

            // if the collected fees is zero, remove the coin from the pool
            if collected_fees.is_zero() {
                self.balances.remove(&c.denom);
            }
        }

        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use crate::coin256::coin256;

    use super::*;
    use cosmwasm_std::coin;

    #[test]
    fn test_collect() {
        let mut pool = IncentivePool::new();

        // Collect 100 uusdt
        pool.collect(coin(100, "uusdt")).unwrap();
        assert_eq!(
            pool.balances.get("uusdt"),
            Some(&IncentivePoolBalance::new(100u128))
        );

        // Collect another 50 uusdt
        pool.collect(coin(50, "uusdt")).unwrap();
        assert_eq!(
            pool.balances.get("uusdt"),
            Some(&IncentivePoolBalance::new(150u128))
        );

        // Collect 200 uusdc
        pool.collect(coin(200, "uusdc")).unwrap();
        assert_eq!(
            pool.balances.get("uusdc"),
            Some(&IncentivePoolBalance::new(200u128))
        );

        assert_eq!(
            pool.balances,
            BTreeMap::from([
                ("uusdt".to_string(), IncentivePoolBalance::new(150u128)),
                ("uusdc".to_string(), IncentivePoolBalance::new(200u128)),
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
        assert_eq!(
            pool.balances.get("uusdt"),
            Some(&IncentivePoolBalance::new(50u128))
        );

        // Deduct another 50 uusdt, should be zero now
        pool.deduct(vec![coin(50, "uusdt")]).unwrap();
        assert_eq!(pool.balances.get("uusdt"), None);

        // Deduct 100 uusdc
        pool.deduct(vec![coin(100, "uusdc")]).unwrap();
        assert_eq!(
            pool.balances.get("uusdc"),
            Some(&IncentivePoolBalance::new(100u128))
        );

        // Try to deduct more than available, should return an error
        let err = pool.deduct(vec![coin(150, "uusdc")]).unwrap_err();
        assert_eq!(
            err,
            ContractError::UnableToDeductFromIncentivePool {
                required: coin256(150, "uusdc"),
                available: coin256(100, "uusdc"),
            }
        );
    }

    #[test]
    fn test_deduct_with_historical_fees() {
        let mut pool = IncentivePool {
            balances: vec![
                (
                    "uusdt".to_string(),
                    IncentivePoolBalance {
                        historical_lambda_collected_balance: 100u128.into(),
                        current_lambda_collected_balance: 200u128.into(),
                    },
                ),
                (
                    "uusdc".to_string(),
                    IncentivePoolBalance {
                        historical_lambda_collected_balance: 200u128.into(),
                        current_lambda_collected_balance: 300u128.into(),
                    },
                ),
            ]
            .into_iter()
            .collect(),
        };

        // Deduct below historical balance
        pool.deduct(vec![coin(50, "uusdt")]).unwrap();
        assert_eq!(
            pool.balances.get("uusdt"),
            Some(&IncentivePoolBalance {
                historical_lambda_collected_balance: 50u128.into(),
                current_lambda_collected_balance: 200u128.into(),
            })
        );

        // Deduct over historical balance
        pool.deduct(vec![coin(150, "uusdt")]).unwrap();
        assert_eq!(
            pool.balances.get("uusdt"),
            Some(&IncentivePoolBalance {
                historical_lambda_collected_balance: 0u128.into(),
                current_lambda_collected_balance: 100u128.into(),
            })
        );
    }
}
