use cosmwasm_std::{Coin, Uint128};

use crate::ContractError;

use super::TransmuterPool;

impl TransmuterPool {
    pub fn withdraw(&mut self, coins: &[Coin]) -> Result<(), ContractError> {
        for coin in coins {
            if coin.denom == self.in_coin.denom {
                self.in_coin.amount =
                    self.in_coin.amount.checked_sub(coin.amount).map_err(|_| {
                        ContractError::InsufficientFund {
                            required: coin.clone(),
                            available: self.in_coin.clone(),
                        }
                    })?;
            } else if coin.denom == self.out_coin_reserve.denom {
                self.out_coin_reserve.amount = self
                    .out_coin_reserve
                    .amount
                    .checked_sub(coin.amount)
                    .map_err(|_| ContractError::InsufficientFund {
                        required: coin.clone(),
                        available: self.out_coin_reserve.clone(),
                    })?;
            } else {
                return Err(ContractError::InsufficientFund {
                    required: coin.clone(),
                    available: Coin::new(0, coin.denom.clone()),
                });
            }
        }

        Ok(())
    }

    /// Calculate the amount of coins to withdraw from the pool based on the number of shares.
    pub fn calc_withdrawing_coins(&self, shares: Uint128) -> Result<Vec<Coin>, ContractError> {
        // withdraw nothing if shares is zero
        if shares.is_zero() {
            Ok(vec![])

        // withdraw from in_coin first
        } else if shares <= self.in_coin.amount {
            Ok(vec![Coin::new(shares.u128(), self.in_coin.denom.clone())])

        // withdraw from out_coin_reserve when in_coin is drained
        } else if self.in_coin.amount.is_zero() {
            Ok(vec![Coin::new(
                shares.u128(),
                self.out_coin_reserve.denom.clone(),
            )])

        // withdraw from both in_coin and out_coin_reserve if shares is larger than in_coin
        } else {
            let in_coin = Coin::new(self.in_coin.amount.u128(), self.in_coin.denom.clone());
            let out_coin_reserve = Coin::new(
                (shares - self.in_coin.amount).u128(),
                self.out_coin_reserve.denom.clone(),
            );
            Ok(vec![in_coin, out_coin_reserve])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const ETH_USDC: &str = "ibc/AXLETHUSDC";
    const COSMOS_USDC: &str = "ibc/COSMOSUSDC";

    #[test]
    fn test_withdraw_succeed_when_has_enough_coins_in_pool() {
        let mut pool = TransmuterPool {
            in_coin: Coin::new(100_000, ETH_USDC),
            out_coin_reserve: Coin::new(100_000, COSMOS_USDC),
        };

        // withdraw in_coin
        pool.withdraw(&[Coin::new(10_000, ETH_USDC)]).unwrap();
        assert_eq!(
            pool,
            TransmuterPool {
                in_coin: Coin::new(90_000, ETH_USDC),
                out_coin_reserve: Coin::new(100_000, COSMOS_USDC),
            }
        );

        // withdraw out_coin
        pool.withdraw(&[Coin::new(10_000, COSMOS_USDC)]).unwrap();
        assert_eq!(
            pool,
            TransmuterPool {
                in_coin: Coin::new(90_000, ETH_USDC),
                out_coin_reserve: Coin::new(90_000, COSMOS_USDC),
            }
        );

        // withdraw both
        pool.withdraw(&[Coin::new(90_000, ETH_USDC), Coin::new(90_000, COSMOS_USDC)])
            .unwrap();
        assert_eq!(
            pool,
            TransmuterPool {
                in_coin: Coin::new(0, ETH_USDC),
                out_coin_reserve: Coin::new(0, COSMOS_USDC),
            }
        );
    }

    #[test]
    fn test_withdraw_fail_when_coin_denom_is_invalid() {
        let mut pool = TransmuterPool {
            in_coin: Coin::new(100_000, ETH_USDC),
            out_coin_reserve: Coin::new(100_000, COSMOS_USDC),
        };

        // withdraw invalid coin
        let err = pool.withdraw(&[Coin::new(10_000, "invalid")]).unwrap_err();
        assert_eq!(
            err,
            ContractError::InsufficientFund {
                required: Coin::new(10_000, "invalid"),
                available: Coin::new(0, "invalid")
            }
        );

        // withdraw both valid and invalid coin
        let err = pool
            .withdraw(&[Coin::new(10_000, ETH_USDC), Coin::new(10_000, "invalid")])
            .unwrap_err();
        assert_eq!(
            err,
            ContractError::InsufficientFund {
                required: Coin::new(10_000, "invalid"),
                available: Coin::new(0, "invalid")
            }
        );
    }

    #[test]
    fn test_withdraw_fail_when_not_enough_coin() {
        let mut pool = TransmuterPool {
            in_coin: Coin::new(100_000, ETH_USDC),
            out_coin_reserve: Coin::new(100_000, COSMOS_USDC),
        };

        // withdraw in_coin
        let err = pool.withdraw(&[Coin::new(100_001, ETH_USDC)]).unwrap_err();
        assert_eq!(
            err,
            ContractError::InsufficientFund {
                required: Coin::new(100_001, ETH_USDC),
                available: Coin::new(100_000, ETH_USDC)
            }
        );

        // withdraw out_coin
        let err = pool
            .withdraw(&[Coin::new(110_000, COSMOS_USDC)])
            .unwrap_err();

        assert_eq!(
            err,
            ContractError::InsufficientFund {
                required: Coin::new(110_000, COSMOS_USDC),
                available: Coin::new(100_000, COSMOS_USDC)
            }
        );

        // withdraw both
        let err = pool
            .withdraw(&[
                Coin::new(110_000, ETH_USDC),
                Coin::new(110_000, COSMOS_USDC),
            ])
            .unwrap_err();

        assert_eq!(
            err,
            ContractError::InsufficientFund {
                required: Coin::new(110_000, ETH_USDC),
                available: Coin::new(100_000, ETH_USDC)
            }
        );
    }

    #[test]
    fn test_calc_withdrawing_coins() {
        // withdraw 0
        let pool = TransmuterPool {
            in_coin: Coin::new(100_000, ETH_USDC),
            out_coin_reserve: Coin::new(100_000, COSMOS_USDC),
        };
        let coins = pool.calc_withdrawing_coins(Uint128::zero()).unwrap();
        assert_eq!(coins, vec![]);

        // withdraw in_coin
        let pool = TransmuterPool {
            in_coin: Coin::new(100_000, ETH_USDC),
            out_coin_reserve: Coin::new(100_000, COSMOS_USDC),
        };
        let coins = pool.calc_withdrawing_coins(Uint128::new(10_000)).unwrap();
        assert_eq!(coins, vec![Coin::new(10_000, ETH_USDC)]);

        // withdraw out_coin
        let pool = TransmuterPool {
            in_coin: Coin::new(0, ETH_USDC),
            out_coin_reserve: Coin::new(100_000, COSMOS_USDC),
        };
        let coins = pool.calc_withdrawing_coins(Uint128::new(10_000)).unwrap();
        assert_eq!(coins, vec![Coin::new(10_000, COSMOS_USDC)]);

        // withdraw both
        let pool = TransmuterPool {
            in_coin: Coin::new(100_000, ETH_USDC),
            out_coin_reserve: Coin::new(100_000, COSMOS_USDC),
        };
        let coins = pool.calc_withdrawing_coins(Uint128::new(100_001)).unwrap();
        assert_eq!(
            coins,
            vec![Coin::new(100_000, ETH_USDC), Coin::new(1, COSMOS_USDC)]
        );

        // withdraw all
        let pool = TransmuterPool {
            in_coin: Coin::new(100_000, ETH_USDC),
            out_coin_reserve: Coin::new(100_000, COSMOS_USDC),
        };
        let coins = pool.calc_withdrawing_coins(Uint128::new(200_000)).unwrap();
        assert_eq!(
            coins,
            vec![
                Coin::new(100_000, ETH_USDC),
                Coin::new(100_000, COSMOS_USDC)
            ]
        );
    }
}
