use std::collections::BTreeMap;

use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Coin, Decimal, Decimal256, OverflowError, Uint128, Uint256, Uint512};

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
    pub historical_lambda: Decimal,
    pub historical_lambda_collected_balance: Uint256,
    pub current_lambda_collected_balance: Uint256,
}

impl IncentivePoolBalance {
    pub fn new(amount: impl Into<Uint256>) -> Self {
        Self {
            historical_lambda: Decimal::zero(),
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

    /// migrate balance on $\lambda$ update.
    /// When the $p_{hist}$ hasn't run out, update the following:
    ///
    /// $$
    /// \lambda_{hist} := \frac{(\lambda_{hist} \cdot p_{hist}) + (\lambda \cdot p)}{p_{hist} + p}
    /// $$
    ///
    /// $$
    /// p_{hist} := p_{hist} + p
    /// $$
    ///
    /// So that we keep remembering past $\lambda$  without storing all of the history.
    fn migrate_balance_on_lambda_updated(
        &mut self,
        lambda_before_update: Decimal,
    ) -> Result<&mut Self, ContractError> {
        // p_{hist} + p
        let total_prev_collected_balance = Uint512::from(self.historical_lambda_collected_balance)
            .checked_add(self.current_lambda_collected_balance.into())?;

        // \lambda_{hist} \cdot p_{hist} + \lambda \cdot p
        // Doing math on atomics as there is no Decimal512 implementation
        // Maxed at 2 * (2^128-1 * 2^256-1) < 2^512 which is safe for Uint512
        let weighted_lambda_sum_atomics = Uint512::from(self.historical_lambda.atomics())
            .checked_mul(self.historical_lambda_collected_balance.into())?
            .checked_add(
                Uint512::from(lambda_before_update.atomics())
                    .checked_mul(self.current_lambda_collected_balance.into())?,
            )?;

        // weighted_lambda_sum_atomics max is 2 * (2^128-1 * 2^256-1)
        // total_prev_collected_balance max is 2^256 - 1
        // DECIMAL_PLACES is 18 so it's eseentially div 10^`18 which is around 2^60 (~ 2^log2(10^18))
        // so historical_lambda max is 2 * (2^128 - 1) / 2^60 < 2^128 so converting to Decimal (128 bits) type is safe
        //
        // It is an integer division, so it is floored, which prevents the pool from over-incentivize.
        self.historical_lambda = Decimal256::from_atomics(
            Uint256::try_from(
                weighted_lambda_sum_atomics.checked_div(total_prev_collected_balance)?,
            )?,
            Decimal256::DECIMAL_PLACES,
        )?
        .try_into()?;

        // p_{hist} := p_{hist} + p
        // This couldn't overflow as any coin total supply is stored as Uint256
        self.historical_lambda_collected_balance = self
            .historical_lambda_collected_balance
            .checked_add(self.current_lambda_collected_balance)?;

        // reset current lambda collected balance
        self.current_lambda_collected_balance = Uint256::zero();

        Ok(self)
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
    pub fn deduct(&mut self, coin: Coin) -> Result<&mut Self, ContractError> {
        // deduct the coin from the pool
        let Some(collected_fees) = self.balances.get_mut(&coin.denom) else {
            return Err(ContractError::UnableToDeductFromIncentivePool {
                required: coin.clone().into(),
                available: Coin256::zero(&coin.denom),
            });
        };

        // try to deduct the coin from the pool
        if let Err(NotEnoughBalanceError { available }) = collected_fees.deduct(coin.amount.into())
        {
            return Err(ContractError::UnableToDeductFromIncentivePool {
                required: coin.clone().into(),
                available: Coin256::new(available, &coin.denom),
            });
        }

        // if the collected fees is zero, remove the coin from the pool
        if collected_fees.is_zero() {
            self.balances.remove(&coin.denom);
        }

        Ok(self)
    }

    /// Updates the historical lambda for all the coins in the pool when lambda is updated.
    pub fn migrate_balances_on_lambda_updated(
        &mut self,
        lambda_before_update: Decimal,
    ) -> Result<&mut Self, ContractError> {
        self.balances
            .iter_mut()
            .try_for_each(|(_, balance)| -> Result<(), ContractError> {
                balance.migrate_balance_on_lambda_updated(lambda_before_update)?;
                Ok(())
            })?;
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use crate::coin256::coin256;

    use super::*;
    use cosmwasm_std::coin;
    use rstest::rstest;

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
        pool.deduct(coin(50, "uusdt")).unwrap();
        assert_eq!(
            pool.balances.get("uusdt"),
            Some(&IncentivePoolBalance::new(50u128))
        );

        // Deduct another 50 uusdt, should be zero now
        pool.deduct(coin(50, "uusdt")).unwrap();
        assert_eq!(pool.balances.get("uusdt"), None);

        // Deduct 100 uusdc
        pool.deduct(coin(100, "uusdc")).unwrap();
        assert_eq!(
            pool.balances.get("uusdc"),
            Some(&IncentivePoolBalance::new(100u128))
        );

        // Try to deduct more than available, should return an error
        let err = pool.deduct(coin(150, "uusdc")).unwrap_err();
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
                        historical_lambda: Decimal::zero(),
                        historical_lambda_collected_balance: 100u128.into(),
                        current_lambda_collected_balance: 200u128.into(),
                    },
                ),
                (
                    "uusdc".to_string(),
                    IncentivePoolBalance {
                        historical_lambda: Decimal::zero(),
                        historical_lambda_collected_balance: 200u128.into(),
                        current_lambda_collected_balance: 300u128.into(),
                    },
                ),
            ]
            .into_iter()
            .collect(),
        };

        // Deduct below historical balance
        pool.deduct(coin(50, "uusdt")).unwrap();
        assert_eq!(
            pool.balances.get("uusdt"),
            Some(&IncentivePoolBalance {
                historical_lambda: Decimal::zero(),
                historical_lambda_collected_balance: 50u128.into(),
                current_lambda_collected_balance: 200u128.into(),
            })
        );

        // Deduct over historical balance
        pool.deduct(coin(150, "uusdt")).unwrap();
        assert_eq!(
            pool.balances.get("uusdt"),
            Some(&IncentivePoolBalance {
                historical_lambda: Decimal::zero(),
                historical_lambda_collected_balance: 0u128.into(),
                current_lambda_collected_balance: 100u128.into(),
            })
        );

        // usdc should remain unchanged
        assert_eq!(
            pool.balances.get("uusdc"),
            Some(&IncentivePoolBalance {
                historical_lambda: Decimal::zero(),
                historical_lambda_collected_balance: 200u128.into(),
                current_lambda_collected_balance: 300u128.into(),
            })
        );
    }

    #[test]
    fn test_migrate_balances_on_lambda_updated() {
        let mut pool = IncentivePool::new();

        pool.collect(coin(100, "uusdt")).unwrap();
        pool.collect(coin(200, "uusdc")).unwrap();

        assert_eq!(
            pool.balances.get("uusdt"),
            Some(&IncentivePoolBalance {
                historical_lambda: Decimal::zero(),
                historical_lambda_collected_balance: 0u128.into(),
                current_lambda_collected_balance: 100u128.into(),
            })
        );
        assert_eq!(
            pool.balances.get("uusdc"),
            Some(&IncentivePoolBalance {
                historical_lambda: Decimal::zero(),
                historical_lambda_collected_balance: 0u128.into(),
                current_lambda_collected_balance: 200u128.into(),
            })
        );

        pool.migrate_balances_on_lambda_updated(Decimal::percent(10))
            .unwrap();

        assert_eq!(
            pool.balances.get("uusdt"),
            Some(&IncentivePoolBalance {
                historical_lambda: Decimal::percent(10),
                historical_lambda_collected_balance: 100u128.into(),
                current_lambda_collected_balance: 0u128.into(),
            })
        );

        assert_eq!(
            pool.balances.get("uusdc"),
            Some(&IncentivePoolBalance {
                historical_lambda: Decimal::percent(10),
                historical_lambda_collected_balance: 200u128.into(),
                current_lambda_collected_balance: 0u128.into(),
            })
        );

        // collect some more uusdt
        pool.collect(coin(50, "uusdt")).unwrap();

        // This migrate should perform weighted average for new lambda in
        // - usdt pool = ((0.1 * 100) + (0.2 * 50)) / 150 = 0.133333333333333333
        // - usdc pool = ((0.1 * 200) + (0.2 * 0)) / 200 = 0.1
        pool.migrate_balances_on_lambda_updated(Decimal::percent(20))
            .unwrap();

        assert_eq!(
            pool.balances.get("uusdt"),
            Some(&IncentivePoolBalance {
                historical_lambda: Decimal::raw(133333333333333333),
                historical_lambda_collected_balance: 150u128.into(),
                current_lambda_collected_balance: 0u128.into(),
            })
        );

        assert_eq!(
            pool.balances.get("uusdc"),
            Some(&IncentivePoolBalance {
                historical_lambda: Decimal::percent(10),
                historical_lambda_collected_balance: 200u128.into(),
                current_lambda_collected_balance: 0u128.into(),
            })
        );
    }

    #[rstest]
    #[case::zero(
        IncentivePoolBalance {
            historical_lambda: Decimal::zero(),
            historical_lambda_collected_balance: 0u128.into(),
            current_lambda_collected_balance: 100u128.into(),
        },
        Decimal::percent(10),
        IncentivePoolBalance {
            historical_lambda: Decimal::percent(10),
            historical_lambda_collected_balance: 100u128.into(),
            current_lambda_collected_balance: 0u128.into(),
        },
    )]
    #[case::half(
        IncentivePoolBalance {
            historical_lambda: Decimal::percent(50),
            historical_lambda_collected_balance: 100u128.into(),
            current_lambda_collected_balance: 200u128.into(),
        },
        Decimal::percent(20),
        IncentivePoolBalance {
            historical_lambda: Decimal::percent(30), // ((0.5 * 100) + (0.2 * 200)) / 300 = 0.3
            historical_lambda_collected_balance: 300u128.into(),
            current_lambda_collected_balance: 0u128.into(),
        },
    )]
    #[case::round_down(
        IncentivePoolBalance {
            historical_lambda: Decimal::percent(50),
            historical_lambda_collected_balance: 300u128.into(),
            current_lambda_collected_balance: 600u128.into(),
        },
        Decimal::percent(30),
        IncentivePoolBalance {
            historical_lambda: Decimal::raw(366666666666666666), // ((0.5 * 300) + (0.3 * 600)) / 900 = 0.366666666666666666
            historical_lambda_collected_balance: 900u128.into(),
            current_lambda_collected_balance: 0u128.into(),
        },
    )]
    #[case::increase(
        IncentivePoolBalance {
            historical_lambda: Decimal::percent(50),
            historical_lambda_collected_balance: 100u128.into(),
            current_lambda_collected_balance: 200u128.into(),
        },
        Decimal::percent(60),
        IncentivePoolBalance {
            historical_lambda: Decimal::raw(566666666666666666), // ((0.5 * 100) + (0.6 * 200)) / 300 = 0.566666666666666666
            historical_lambda_collected_balance: 300u128.into(),
            current_lambda_collected_balance: 0u128.into(),
        },
    )]
    #[case::close_to_edge(
        IncentivePoolBalance {
            historical_lambda: Decimal::percent(50),
            historical_lambda_collected_balance: Uint256::one(),
            current_lambda_collected_balance: Uint256::MAX - Uint256::one(),
        },
        Decimal::percent(30),
        IncentivePoolBalance {
            historical_lambda: Decimal::percent(30), // ((0.5 * 1) + (0.3 * (2^256 - 1))) / (1 + (2^256 - 1)) = 0.3
            historical_lambda_collected_balance: Uint256::MAX,
            current_lambda_collected_balance: 0u128.into(),
        },
    )]
    #[case::max(
        IncentivePoolBalance {
            historical_lambda: Decimal::percent(100),
            historical_lambda_collected_balance: Uint256::MAX,
            current_lambda_collected_balance: Uint256::zero(),
        },
        Decimal::percent(10),
        IncentivePoolBalance {
            historical_lambda: Decimal::percent(100),
            historical_lambda_collected_balance: Uint256::MAX,
            current_lambda_collected_balance: Uint256::zero(),
        },
    )]
    fn test_migrate_balance_on_lambda_updated(
        #[case] mut incentive_pool_balance: IncentivePoolBalance,
        #[case] updated_lambda: Decimal,
        #[case] expected_incentive_pool_balance: IncentivePoolBalance,
    ) {
        incentive_pool_balance
            .migrate_balance_on_lambda_updated(updated_lambda)
            .unwrap();

        assert_eq!(incentive_pool_balance, expected_incentive_pool_balance);
    }
}
