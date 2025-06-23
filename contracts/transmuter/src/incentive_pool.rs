use cosmwasm_std::{Addr, Coin, Storage, Uint128, Uint256};
use cw_storage_plus::{Item, Map};
use std::collections::BTreeMap;

use crate::ContractError;

/// Incentive pool state management for rebalancing fees and incentives
pub struct IncentivePool {
    /// Track incentive pool balances by denom (exact amounts)
    pool_balances: Map<String, Uint128>,
    /// Track outstanding credits to users (normalized amounts)
    outstanding_credits: Map<Addr, Uint128>,
    /// Accumulator for total outstanding credits across all users
    total_outstanding_credits: Item<Uint256>,
}

impl IncentivePool {
    pub const fn new(
        pool_balances_key: &'static str,
        outstanding_credits_key: &'static str,
        total_outstanding_credits_key: &'static str,
    ) -> Self {
        Self {
            pool_balances: Map::new(pool_balances_key),
            outstanding_credits: Map::new(outstanding_credits_key),
            total_outstanding_credits: Item::new(total_outstanding_credits_key),
        }
    }

    /// Add exact tokens to the incentive pool (from collected fees)
    pub fn add_tokens(&self, storage: &mut dyn Storage, coin: &Coin) -> Result<(), ContractError> {
        let current_balance = self
            .pool_balances
            .may_load(storage, coin.denom.clone())?
            .unwrap_or_default();

        let new_balance = current_balance.checked_add(coin.amount)?;
        self.pool_balances
            .save(storage, coin.denom.clone(), &new_balance)?;

        Ok(())
    }

    /// Remove exact tokens from the incentive pool (for paying out incentives)
    pub fn remove_tokens(
        &self,
        storage: &mut dyn Storage,
        coin: &Coin,
    ) -> Result<(), ContractError> {
        let current_balance = self
            .pool_balances
            .may_load(storage, coin.denom.clone())?
            .unwrap_or_default();

        if current_balance < coin.amount {
            return Err(ContractError::InsufficientIncentivePool {
                denom: coin.denom.clone(),
                available: current_balance,
                requested: coin.amount,
            });
        }

        let new_balance = current_balance.checked_sub(coin.amount)?;
        if new_balance.is_zero() {
            self.pool_balances.remove(storage, coin.denom.clone());
        } else {
            self.pool_balances
                .save(storage, coin.denom.clone(), &new_balance)?;
        }

        Ok(())
    }

    /// Credit normalized incentive amount to user (when they should receive incentives)
    /// Validates that total credits don't exceed normalized pool value and caps if necessary
    /// Returns the actual amount credited (may be less than requested if capped)
    pub fn credit_incentive_to_user(
        &self,
        storage: &mut dyn Storage,
        user: &Addr,
        requested_amount: Uint128,
        pool_denom_factors: &BTreeMap<String, Uint128>, // (denom, normalization_factor) pairs
        std_norm_factor: Uint128,
    ) -> Result<Uint128, ContractError> {
        // Calculate total normalized value of all pool tokens
        let total_pool_normalized_value = self
            .pool_balances
            .range(storage, None, None, cosmwasm_std::Order::Ascending)
            .map(|result| {
                let (denom, exact_amount) = result?;

                // Find the normalization factor for this denom
                let norm_factor = pool_denom_factors.get(&denom).ok_or_else(|| {
                    ContractError::InvalidPoolAssetDenom {
                        denom: denom.clone(),
                    }
                })?;

                // Calculate normalized value: exact_amount * norm_factor / std_norm_factor
                exact_amount
                    .checked_multiply_ratio(*norm_factor, std_norm_factor)
                    .map_err(|e| ContractError::CheckedMultiplyRatioError(e))
            })
            .try_fold(Uint256::zero(), |acc, amount| {
                amount.and_then(|amount| {
                    acc.checked_add(Uint256::from(amount))
                        .map_err(|e| ContractError::OverflowError(e))
                })
            })?;

        // Get current total outstanding credits from accumulator
        let current_total_credits = self.get_total_credits(storage)?;

        // Calculate maximum additional credits we can give
        let max_additional_credits: Uint128 = total_pool_normalized_value
            .saturating_sub(current_total_credits)
            .try_into()
            .unwrap_or(Uint128::MAX);

        // Cap the requested amount to what's available
        let actual_credit_amount = requested_amount.min(max_additional_credits);

        // Early return if no credits are credited, no need to update the user's credit
        if actual_credit_amount.is_zero() {
            return Ok(actual_credit_amount);
        }

        let current_user_credit = self
            .outstanding_credits
            .may_load(storage, user.clone())?
            .unwrap_or_default();

        let updated_user_credit = current_user_credit.checked_add(actual_credit_amount)?;

        if updated_user_credit.is_zero() {
            // Remove the entry if the new credit is zero
            self.outstanding_credits.remove(storage, user.clone());
        } else {
            // Save the updated credit
            self.outstanding_credits
                .save(storage, user.clone(), &updated_user_credit)?;
        }

        // Update the total outstanding credits accumulator
        let updated_total_credits =
            current_total_credits.checked_add(Uint256::from(actual_credit_amount))?;
        self.total_outstanding_credits
            .save(storage, &updated_total_credits)?;

        Ok(actual_credit_amount)
    }

    /// Redeem incentive tokens using user's normalized credits
    /// Input: Vec<(Coin, Uint128)> where Coin is exact token to redeem, Uint128 is normalized cost
    /// Checks both pool balance and user credit availability
    pub fn redeem_incentive(
        &self,
        storage: &mut dyn Storage,
        user: &Addr,
        redemptions: Vec<(Coin, Uint128)>,
    ) -> Result<(), ContractError> {
        // Calculate total normalized cost
        let total_normalized_cost = redemptions
            .iter()
            .map(|(_, normalized_cost)| *normalized_cost)
            .try_fold(Uint128::zero(), |acc, cost| acc.checked_add(cost))?;

        // Check user has enough credits
        let current_credit = self
            .outstanding_credits
            .may_load(storage, user.clone())?
            .unwrap_or_default();

        if current_credit < total_normalized_cost {
            return Err(ContractError::InsufficientIncentiveCredit {
                user: user.clone(),
                available: current_credit,
                requested: total_normalized_cost,
            });
        }

        // Check pool has enough of each token
        for (coin, _) in &redemptions {
            let available_balance = self
                .pool_balances
                .may_load(storage, coin.denom.clone())?
                .unwrap_or_default();

            if available_balance < coin.amount {
                return Err(ContractError::InsufficientIncentivePool {
                    denom: coin.denom.clone(),
                    available: available_balance,
                    requested: coin.amount,
                });
            }
        }

        // All checks passed, perform the redemption
        // First, deduct from user's credits
        let updated_credit = current_credit.checked_sub(total_normalized_cost)?;
        if updated_credit.is_zero() {
            self.outstanding_credits.remove(storage, user.clone());
        } else {
            self.outstanding_credits
                .save(storage, user.clone(), &updated_credit)?;
        }

        // Update the total outstanding credits accumulator
        let current_total = self.get_total_credits(storage)?;
        let updated_total = current_total.checked_sub(Uint256::from(total_normalized_cost))?;
        self.total_outstanding_credits
            .save(storage, &updated_total)?;

        // Then, remove tokens from pool
        for (coin, _) in redemptions {
            let current_balance = self
                .pool_balances
                .may_load(storage, coin.denom.clone())?
                .unwrap_or_default();

            let updated_balance = current_balance.checked_sub(coin.amount)?;
            if updated_balance.is_zero() {
                self.pool_balances.remove(storage, coin.denom.clone());
            } else {
                self.pool_balances
                    .save(storage, coin.denom.clone(), &updated_balance)?;
            }
        }

        Ok(())
    }

    /// Get the exact pool balance for a specific denom
    pub fn get_pool_balance(
        &self,
        storage: &dyn Storage,
        denom: &str,
    ) -> Result<Uint128, ContractError> {
        Ok(self
            .pool_balances
            .may_load(storage, denom.to_string())?
            .unwrap_or_default())
    }

    /// Get the total outstanding credits to all users in normalized amount
    pub fn get_total_credits(&self, storage: &dyn Storage) -> Result<Uint256, ContractError> {
        Ok(self
            .total_outstanding_credits
            .may_load(storage)?
            .unwrap_or_default())
    }

    /// Get credits for a specific user in normalized amount
    pub fn get_user_credit(
        &self,
        storage: &dyn Storage,
        user: &Addr,
    ) -> Result<Uint128, ContractError> {
        Ok(self
            .outstanding_credits
            .may_load(storage, user.clone())?
            .unwrap_or_default())
    }

    /// Get all pool balances (exact amounts)
    pub fn get_all_pool_balances(&self, storage: &dyn Storage) -> Result<Vec<Coin>, ContractError> {
        let balances: Result<Vec<_>, _> = self
            .pool_balances
            .range(storage, None, None, cosmwasm_std::Order::Ascending)
            .collect();

        Ok(balances?
            .into_iter()
            .map(|(denom, amount)| Coin::new(amount.u128(), denom))
            .collect())
    }

    /// Get all users with outstanding credits
    pub fn get_all_credit_users(
        &self,
        storage: &dyn Storage,
    ) -> Result<Vec<(Addr, Uint128)>, ContractError> {
        let all_credits: Result<Vec<_>, _> = self
            .outstanding_credits
            .range(storage, None, None, cosmwasm_std::Order::Ascending)
            .collect();

        Ok(all_credits?)
    }
}

#[cfg(test)]
mod tests {
    use cosmwasm_std::{coin, testing::MockStorage};
    use rstest::rstest;

    use super::*;

    const POOL_BALANCES_KEY: &str = "pool_balances";
    const OUTSTANDING_CREDITS_KEY: &str = "outstanding_credits";
    const TOTAL_OUTSTANDING_CREDITS_KEY: &str = "total_outstanding_credits";

    fn setup_incentive_pool(
        storage: &mut MockStorage,
        pool_balances: Vec<Coin>,
        outstanding_credits: Vec<Coin>,
    ) -> IncentivePool {
        let pool_balances_storage = Map::new(POOL_BALANCES_KEY);
        for coin in pool_balances {
            pool_balances_storage
                .save(storage, &coin.denom, &coin.amount)
                .unwrap();
        }
        let outstanding_credits_storage = Map::new(OUTSTANDING_CREDITS_KEY);
        for coin in outstanding_credits {
            outstanding_credits_storage
                .save(storage, &coin.denom, &coin.amount)
                .unwrap();
        }

        let total_outstanding_credits_storage = Item::new(TOTAL_OUTSTANDING_CREDITS_KEY);
        total_outstanding_credits_storage
            .save(storage, &Uint256::zero())
            .unwrap();

        IncentivePool::new(
            POOL_BALANCES_KEY,
            OUTSTANDING_CREDITS_KEY,
            TOTAL_OUTSTANDING_CREDITS_KEY,
        )
    }

    #[rstest]
    #[case::add_to_empty(vec![], coin(100, "uatom"), vec![coin(100, "uatom")])]
    #[case::add_zero_amount(vec![], coin(0, "uosmo"), vec![coin(0, "uosmo")])]
    #[case::add_to_existing(vec![coin(50, "uatom")], coin(100, "uatom"), vec![coin(150, "uatom")])]
    #[case::add_different_denom(vec![coin(50, "uatom")], coin(100, "uosmo"), vec![coin(50, "uatom"), coin(100, "uosmo")])]
    #[case::add_large_amount(vec![], coin(999999999, "uion"), vec![coin(999999999, "uion")])]
    #[case::add_multiple_denoms_existing(vec![coin(10, "uatom"), coin(20, "uosmo")], coin(30, "uion"), vec![coin(10, "uatom"), coin(30, "uion"), coin(20, "uosmo")])]
    #[case::add_same_denom_multiple_times(vec![coin(100, "uatom")], coin(100, "uatom"), vec![coin(200, "uatom")])]
    #[case::add_to_empty_with_special_chars(vec![], coin(42, "denom-with-dashes"), vec![coin(42, "denom-with-dashes")])]
    #[case::add_max_u128(vec![], coin(u128::MAX, "max_denom"), vec![coin(u128::MAX, "max_denom")])]
    #[case::add_small_amount(vec![], coin(1, "small"), vec![coin(1, "small")])]
    fn test_add_tokens(
        #[case] pool_balances: Vec<Coin>,
        #[case] additional_token: Coin,
        #[case] expected_pool_balances: Vec<Coin>,
    ) {
        let mut storage = MockStorage::new();
        let incentive_pool = setup_incentive_pool(&mut storage, pool_balances, vec![]);
        incentive_pool
            .add_tokens(&mut storage, &additional_token)
            .unwrap();

        let pool_balances = incentive_pool.get_all_pool_balances(&storage).unwrap();
        assert_eq!(pool_balances, expected_pool_balances);
    }

    #[test]
    fn test_add_tokens_overflow() {
        let mut storage = MockStorage::new();
        let incentive_pool = setup_incentive_pool(&mut storage, vec![], vec![]);

        // Add maximum amount first
        let max_coin = coin(u128::MAX, "overflow_denom");
        incentive_pool.add_tokens(&mut storage, &max_coin).unwrap();

        // Try to add 1 more (should overflow)
        let overflow_coin = coin(1, "overflow_denom");
        let result = incentive_pool.add_tokens(&mut storage, &overflow_coin);
        assert!(result.is_err(), "Adding to max amount should overflow");
    }

    #[test]
    fn test_get_pool_balance_nonexistent_denom() {
        let mut storage = MockStorage::new();
        let incentive_pool = setup_incentive_pool(&mut storage, vec![], vec![]);

        // Should return zero for non-existent denoms
        let balance = incentive_pool
            .get_pool_balance(&storage, "nonexistent")
            .unwrap();
        assert_eq!(balance, Uint128::zero());
    }

    #[rstest]
    #[case::remove_from_empty_pool(
        vec![],
        coin(100, "uatom"),
        Err(ContractError::InsufficientIncentivePool {
            denom: "uatom".to_string(),
            available: Uint128::zero(),
            requested: Uint128::new(100),
        }),
        vec![]
    )]
    #[case::remove_zero_amount(
        vec![coin(50, "uatom")],
        coin(0, "uatom"),
        Ok(()),
        vec![coin(50, "uatom")]
    )]
    #[case::remove_exact_amount(
        vec![coin(100, "uatom")],
        coin(100, "uatom"),
        Ok(()),
        vec![]
    )]
    #[case::remove_partial_amount(
        vec![coin(150, "uatom")],
        coin(100, "uatom"),
        Ok(()),
        vec![coin(50, "uatom")]
    )]
    #[case::remove_more_than_available(
        vec![coin(50, "uatom")],
        coin(100, "uatom"),
        Err(ContractError::InsufficientIncentivePool {
            denom: "uatom".to_string(),
            available: Uint128::new(50),
            requested: Uint128::new(100),
        }),
        vec![coin(50, "uatom")]
    )]
    #[case::remove_from_multiple_denoms(
        vec![coin(100, "uatom"), coin(200, "uosmo")],
        coin(50, "uatom"),
        Ok(()),
        vec![coin(50, "uatom"), coin(200, "uosmo")]
    )]
    #[case::remove_different_denom_than_exists(
        vec![coin(100, "uatom")],
        coin(50, "uosmo"),
        Err(ContractError::InsufficientIncentivePool {
            denom: "uosmo".to_string(),
            available: Uint128::zero(),
            requested: Uint128::new(50),
        }),
        vec![coin(100, "uatom")]
    )]
    #[case::remove_large_amount(
        vec![coin(999999999, "uion")],
        coin(999999999, "uion"),
        Ok(()),
        vec![]
    )]
    #[case::remove_small_amount(
        vec![coin(1000, "small")],
        coin(1, "small"),
        Ok(()),
        vec![coin(999, "small")]
    )]
    #[case::remove_max_u128(
        vec![coin(u128::MAX, "max_denom")],
        coin(u128::MAX, "max_denom"),
        Ok(()),
        vec![]
    )]
    #[case::remove_to_zero_balance(
        vec![coin(100, "zero_denom")],
        coin(100, "zero_denom"),
        Ok(()),
        vec![]
    )]
    fn test_remove_tokens(
        #[case] pool_balances: Vec<Coin>,
        #[case] token_to_remove: Coin,
        #[case] expected: Result<(), ContractError>,
        #[case] expected_balances: Vec<Coin>,
    ) {
        let mut storage = MockStorage::new();
        let incentive_pool = setup_incentive_pool(&mut storage, pool_balances.clone(), vec![]);

        let result = incentive_pool.remove_tokens(&mut storage, &token_to_remove);
        assert_eq!(result, expected);

        let balances = incentive_pool.get_all_pool_balances(&storage).unwrap();
        assert_eq!(balances, expected_balances);
    }
}
