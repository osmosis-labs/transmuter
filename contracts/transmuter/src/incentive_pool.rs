use cosmwasm_std::{Addr, Coin, Storage, Uint128, Uint256};
use cw_storage_plus::{Bound, Item, Map};
use std::collections::BTreeMap;

use crate::{
    asset::{convert_amount, Rounding},
    math::lcm_from_iter,
    ContractError,
};

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
    pub fn credit_incentive(
        &self,
        storage: &mut dyn Storage,
        beneficiary: &Addr,
        requested_amount: Uint128,
        pool_denom_factors: &BTreeMap<String, Uint128>, // (denom, normalization_factor) pairs
    ) -> Result<Uint128, ContractError> {
        let std_norm_factor = lcm_from_iter(pool_denom_factors.values().copied())?;

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

                // Convert exact amount to normalized amount, rounding down because we want to cap the amount of credits under what's available
                convert_amount(exact_amount, *norm_factor, std_norm_factor, &Rounding::Down)
            })
            .try_fold(Uint256::zero(), |acc, amount| {
                amount.and_then(|amount| {
                    acc.checked_add(Uint256::from(amount))
                        .map_err(|e| ContractError::OverflowError(e))
                })
            })?;

        // Get current total outstanding credits from accumulator
        let current_total_credits = self.get_total_incentive_credits(storage)?;

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
            .may_load(storage, beneficiary.clone())?
            .unwrap_or_default();

        let updated_user_credit = current_user_credit.checked_add(actual_credit_amount)?;

        if updated_user_credit.is_zero() {
            // Remove the entry if the new credit is zero
            self.outstanding_credits
                .remove(storage, beneficiary.clone());
        } else {
            // Save the updated credit
            self.outstanding_credits
                .save(storage, beneficiary.clone(), &updated_user_credit)?;
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
        redemptions: Vec<Coin>,
        pool_denom_factors: &BTreeMap<String, Uint128>, // (denom, normalization_factor) pairs
    ) -> Result<(), ContractError> {
        let std_norm_factor = lcm_from_iter(pool_denom_factors.values().copied())?;

        // Calculate total normalized cost
        let total_normalized_cost = redemptions
            .iter()
            .map(|coin| {
                let norm_factor = pool_denom_factors.get(&coin.denom).ok_or_else(|| {
                    ContractError::InvalidPoolAssetDenom {
                        denom: coin.denom.clone(),
                    }
                })?;

                // Convert exact amount to normalized amount, rounding up to prevent deducting less than the actual cost
                convert_amount(coin.amount, *norm_factor, std_norm_factor, &Rounding::Up)
            })
            .try_fold(Uint128::zero(), |acc, cost| {
                cost.and_then(|cost| {
                    acc.checked_add(cost)
                        .map_err(|e| ContractError::OverflowError(e))
                })
            })?;

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
        for coin in &redemptions {
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
        let current_total = self.get_total_incentive_credits(storage)?;
        let updated_total = current_total.checked_sub(Uint256::from(total_normalized_cost))?;
        self.total_outstanding_credits
            .save(storage, &updated_total)?;

        // Then, remove tokens from pool
        for coin in redemptions {
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
    pub fn get_total_incentive_credits(
        &self,
        storage: &dyn Storage,
    ) -> Result<Uint256, ContractError> {
        Ok(self
            .total_outstanding_credits
            .may_load(storage)?
            .unwrap_or_default())
    }

    /// Get credits for a user by address in normalized amount
    pub fn get_incentive_credit_by_address(
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
    pub fn get_all_incentive_credits(
        &self,
        storage: &dyn Storage,
        min: Option<Bound<Addr>>,
        max: Option<Bound<Addr>>,
    ) -> Result<Vec<(Addr, Uint128)>, ContractError> {
        let all_credits: Result<Vec<_>, _> = self
            .outstanding_credits
            .range(storage, min, max, cosmwasm_std::Order::Ascending)
            .collect();

        Ok(all_credits?)
    }
}

#[cfg(test)]
mod tests {
    use cosmwasm_std::{coin, testing::MockStorage, Addr, Uint256};
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

    // Standard test denoms with different normalization factors
    const TEST_DENOMS: &[(&str, u128)] = &[
        ("denom_6d", 1_000_000),                  // 6 decimals, factor 1M
        ("denom_6d_2", 1_000_000),                // 6 decimals, factor 1M
        ("denom_9d", 1_000_000_000),              // 9 decimals, factor 1B
        ("denom_18d", 1_000_000_000_000_000_000), // 18 decimals, factor 1E18
        ("denom_0d", 1),                          // 0 decimals, factor 1
        ("denom_3d", 1_000),                      // 3 decimals, factor 1K
    ];

    // Helper function to create normalization map
    fn create_norm_factors(denoms: &[&str]) -> BTreeMap<String, Uint128> {
        denoms
            .iter()
            .map(|denom| {
                let factor = TEST_DENOMS
                    .iter()
                    .find(|(d, _)| d == denom)
                    .map(|(_, f)| *f)
                    .unwrap_or(1_000_000); // default to 1M if not found
                (denom.to_string(), Uint128::new(factor))
            })
            .collect()
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

    #[rstest]
    #[case::credit_single_denom_basic(
        vec![coin(1000, "denom_6d")],
        vec!["denom_6d"],
        vec![],
        "user1",
        Uint128::new(500),
        Uint128::new(500),
        Uint128::new(500),
        Uint256::from(500u128),
    )]
    #[case::credit_multiple_denoms_basic(
        vec![coin(1000, "denom_6d"), coin(2000, "denom_9d")],
        vec!["denom_6d", "denom_9d"],
        vec![],
        "user1",
        Uint128::new(500),
        Uint128::new(500),
        Uint128::new(500),
        Uint256::from(500u128),
    )]
    #[case::credit_multiple_denoms_with_existing_credit(
        vec![coin(1000, "denom_6d"), coin(2000, "denom_9d")],
        vec!["denom_6d", "denom_9d"],
        vec![("user1", Uint128::new(500))],
        "user1",
        Uint128::new(500),
        Uint128::new(500),
        Uint128::new(1000),
        Uint256::from(1000u128),
    )]
    #[case::credit_multiple_denoms_with_existing_credit_with_different_user(
        vec![coin(1000, "denom_6d"), coin(2000, "denom_9d")],
        vec!["denom_6d", "denom_9d"],
        vec![("user1", Uint128::new(500))],
        "user2",
        Uint128::new(500),
        Uint128::new(500),
        Uint128::new(500),
        Uint256::from(1000u128),
    )]
    #[case::credit_multiple_denoms_with_existing_credit_with_same_user_at_max_creditable(
        vec![coin(1000, "denom_6d"), coin(2000, "denom_9d")], // total pool normalized value = 1002000
        vec!["denom_6d", "denom_9d"],
        vec![("user1", Uint128::new(1000000))],
        "user1",
        Uint128::new(2000),
        Uint128::new(2000),
        Uint128::new(1002000u128),
        Uint256::from(1002000u128),
    )]
    #[case::credit_multiple_denoms_with_existing_credit_with_same_user_exceeding_creditable(
        vec![coin(1000, "denom_6d"), coin(2000, "denom_9d")], // total pool normalized value = 1002000
        vec!["denom_6d", "denom_9d"],
        vec![("user1", Uint128::new(1000000))],
        "user1",
        Uint128::new(2001),
        Uint128::new(2000),
        Uint128::new(1002000u128),
        Uint256::from(1002000u128),
    )]
    #[case::credit_multiple_denoms_with_existing_credit_with_different_user_at_max_creditable(
        vec![coin(1000, "denom_6d"), coin(2000, "denom_9d")], // total pool normalized value = 1002000
        vec!["denom_6d", "denom_9d"],
        vec![("user1", Uint128::new(1000000))],
        "user2",
        Uint128::new(2000),
        Uint128::new(2000),
        Uint128::new(2000),
        Uint256::from(1002000u128),
    )]
    #[case::credit_multiple_denoms_with_existing_credit_with_different_user_exceeding_creditable(
        vec![coin(1000, "denom_6d"), coin(2000, "denom_9d")], // total pool normalized value = 1002000
        vec!["denom_6d", "denom_9d"],
        vec![("user1", Uint128::new(1000000))],
        "user2",
        Uint128::new(2001),
        Uint128::new(2000),
        Uint128::new(2000),
        Uint256::from(1002000u128),
    )]
    #[case::credit_multiple_denoms_with_existing_credit_with_same_and_different_user_exceeding_creditable(
        vec![coin(1000, "denom_6d"), coin(2000, "denom_9d")], // total pool normalized value = 1002000
        vec!["denom_6d", "denom_9d"],
        vec![("user1", Uint128::new(600000)), ("user2", Uint128::new(400000))],
        "user2",
        Uint128::new(2001),
        Uint128::new(2000),
        Uint128::new(402000),
        Uint256::from(1002000u128),
    )]
    #[case::credit_total_credits_exceeding_u128_max(
        vec![coin(u128::MAX, "denom_18d"), coin(1000, "denom_6d")],
        vec!["denom_18d", "denom_6d"],
        vec![("user1", Uint128::new(u128::MAX))],
        "user2",
        Uint128::new(1000),
        Uint128::new(1000),
        Uint128::new(1000),
        Uint256::from(u128::MAX) + Uint256::from(1000u128),
    )]

    fn test_credit_incentive(
        #[case] pool_balances: Vec<Coin>,
        #[case] pool_denoms: Vec<&str>,
        #[case] existing_credit: Vec<(&str, Uint128)>,
        #[case] user: &str,
        #[case] requested_amount: Uint128,
        #[case] expected_credited: Uint128,
        #[case] expected_user_credit: Uint128,
        #[case] expected_total_credits: Uint256,
    ) {
        let mut storage = MockStorage::new();
        let incentive_pool = setup_incentive_pool(&mut storage, pool_balances.clone(), vec![]);
        let user = Addr::unchecked(user);

        // Set up user's existing credit
        for (user, credit) in &existing_credit {
            incentive_pool
                .outstanding_credits
                .save(&mut storage, Addr::unchecked(*user), credit)
                .unwrap();
        }

        // Initialize total outstanding credits to match existing credits
        let total_existing_credits: Uint256 = existing_credit
            .iter()
            .map(|(_, credit)| Uint256::from(*credit))
            .fold(Uint256::zero(), |acc, credit| acc + credit);
        incentive_pool
            .total_outstanding_credits
            .save(&mut storage, &total_existing_credits)
            .unwrap();

        let norm_factors = create_norm_factors(&pool_denoms);

        let result =
            incentive_pool.credit_incentive(&mut storage, &user, requested_amount, &norm_factors);

        assert_eq!(result, Ok(expected_credited));

        // Verify user's credit
        assert_eq!(
            incentive_pool
                .get_incentive_credit_by_address(&storage, &user)
                .unwrap(),
            expected_user_credit
        );

        // Verify total outstanding credits
        let total_credits = incentive_pool
            .get_total_incentive_credits(&storage)
            .unwrap();
        assert_eq!(total_credits, expected_total_credits);

        // Verify pool balances are unchanged
        let final_balances = incentive_pool.get_all_pool_balances(&storage).unwrap();
        assert_eq!(final_balances, pool_balances.clone());
    }

    #[test]
    fn test_credit_incentive_invalid_denom() {
        let mut storage = MockStorage::new();
        let incentive_pool =
            setup_incentive_pool(&mut storage, vec![coin(1000, "invalid_denom")], vec![]);

        let user = Addr::unchecked("user1");
        let norm_factors = create_norm_factors(&["denom_a"]); // Only denom_a in normalization map

        let result =
            incentive_pool.credit_incentive(&mut storage, &user, Uint128::new(500), &norm_factors);

        assert_eq!(
            result,
            Err(ContractError::InvalidPoolAssetDenom {
                denom: "invalid_denom".to_string(),
            })
        );
    }

    #[test]
    fn test_credit_incentive_overflow() {
        let mut storage = MockStorage::new();
        let incentive_pool =
            setup_incentive_pool(&mut storage, vec![coin(u128::MAX, "denom_a")], vec![]);

        let user = Addr::unchecked("user1");
        let norm_factors = create_norm_factors(&["denom_a"]);

        // This should succeed but test the overflow handling
        let result =
            incentive_pool.credit_incentive(&mut storage, &user, Uint128::new(1000), &norm_factors);

        assert_eq!(result, Ok(Uint128::new(1000)));
    }

    #[rstest]
    #[case::redeem_single_denom_basic(
        vec![coin(1000, "denom_6d")],
        vec!["denom_6d"],
        vec![("user1", Uint128::new(1000))],
        "user1",
        vec![coin(500, "denom_6d")],
        Ok(()),
        vec![coin(500, "denom_6d")],
        vec![("user1", Uint128::new(500))],
        Uint256::from(500u128),
    )]
    #[case::redeem_single_denom_all(
        vec![coin(1000, "denom_6d")],
        vec!["denom_6d"],
        vec![("user1", Uint128::new(1000))],
        "user1",
        vec![coin(1000, "denom_6d")],
        Ok(()),
        vec![],
        vec![],
        Uint256::zero(),
    )]
    #[case::redeem_multiple_denoms_basic(
        vec![coin(1000, "denom_6d"), coin(2000, "denom_9d")],
        vec!["denom_6d", "denom_9d"],
        vec![("user1", Uint128::new(1000000))],
        "user1",
        vec![coin(500, "denom_6d"), coin(1000, "denom_9d")],
        Ok(()),
        vec![coin(500, "denom_6d"), coin(1000, "denom_9d")],
        vec![("user1", Uint128::new(499000))],
        Uint256::from(499000u128),
    )]
    #[case::redeem_multiple_denoms_at_max_credit(
        vec![coin(1000, "denom_6d"), coin(2000, "denom_9d")],
        vec!["denom_6d", "denom_9d"],
        vec![("user1", Uint128::new(501000))],
        "user1",
        vec![coin(500, "denom_6d"), coin(1000, "denom_9d")],
        Ok(()),
        vec![coin(500, "denom_6d"), coin(1000, "denom_9d")],
        vec![],
        Uint256::zero(),
    )]
    #[case::redeem_multiple_denoms_exceeding_credit(
        vec![coin(1000, "denom_6d"), coin(2000, "denom_9d")],
        vec!["denom_6d", "denom_9d"],
        vec![("user1", Uint128::new(500999))],
        "user1",
        vec![coin(500, "denom_6d"), coin(1000, "denom_9d")],
        Err(ContractError::InsufficientIncentiveCredit {
            user: Addr::unchecked("user1"),
            available: Uint128::new(500999),
            requested: Uint128::new(501000),
        }),
        vec![coin(1000, "denom_6d"), coin(2000, "denom_9d")],
        vec![("user1", Uint128::new(500999))],
        Uint256::from(500999u128),
    )]
    #[case::redeem_multiple_users_basic(
        vec![coin(1000, "denom_6d"), coin(2000, "denom_9d")],
        vec!["denom_6d", "denom_9d"],
        vec![("user1", Uint128::new(1000000)), ("user2", Uint128::new(1000000))],
        "user1",
        vec![coin(500, "denom_6d"), coin(1000, "denom_9d")],
        Ok(()),
        vec![coin(500, "denom_6d"), coin(1000, "denom_9d")],
        vec![("user1", Uint128::new(499000)), ("user2", Uint128::new(1000000))],
        Uint256::from(1499000u128),
    )]
    #[case::redeem_multiple_users_at_max_credit(
        vec![coin(1000, "denom_6d"), coin(2000, "denom_9d")],
        vec!["denom_6d", "denom_9d"],
        vec![("user1", Uint128::new(501000)), ("user2", Uint128::new(501000))],
        "user1",
        vec![coin(500, "denom_6d"), coin(1000, "denom_9d")],
        Ok(()),
        vec![coin(500, "denom_6d"), coin(1000, "denom_9d")],
        vec![("user2", Uint128::new(501000))],
        Uint256::from(501000u128),
    )]
    #[case::redeem_multiple_users_exceeding_credit(
        vec![coin(1000, "denom_6d"), coin(2000, "denom_9d")],
        vec!["denom_6d", "denom_9d"],
        vec![("user1", Uint128::new(500999)), ("user2", Uint128::new(500999))],
        "user1",
        vec![coin(500, "denom_6d"), coin(1000, "denom_9d")],
        Err(ContractError::InsufficientIncentiveCredit {
            user: Addr::unchecked("user1"),
            available: Uint128::new(500999),
            requested: Uint128::new(501000),
        }),
        vec![coin(1000, "denom_6d"), coin(2000, "denom_9d")],
        vec![("user1", Uint128::new(500999)), ("user2", Uint128::new(500999))],
        Uint256::from(1001998u128),
    )]
    #[case::redeem_multiple_users_exceeding_balance(
        vec![coin(1000, "denom_6d"), coin(2000, "denom_9d")],
        vec!["denom_6d", "denom_9d"],
        vec![("user1", Uint128::new(1003000)), ("user2", Uint128::new(501000))],
        "user1",
        vec![coin(1001, "denom_6d"), coin(2000, "denom_9d")],
        Err(ContractError::InsufficientIncentivePool {
            denom: "denom_6d".to_string(),
            available: Uint128::new(1000),
            requested: Uint128::new(1001),
        }),
        vec![coin(1000, "denom_6d"), coin(2000, "denom_9d")],
        vec![("user1", Uint128::new(1003000)), ("user2", Uint128::new(501000))],
        Uint256::from(1504000u128),
    )]
    fn test_redeem_incentive(
        #[case] incentive_pool_balances: Vec<Coin>,
        #[case] pool_denoms: Vec<&str>,
        #[case] existing_credit: Vec<(&str, Uint128)>,
        #[case] user: &str,
        #[case] redemptions: Vec<Coin>,
        #[case] expected_result: Result<(), ContractError>,
        #[case] expected_balances: Vec<Coin>,
        #[case] expected_user_credits: Vec<(&str, Uint128)>,
        #[case] expected_total_credits: Uint256,
    ) {
        let mut storage = MockStorage::new();
        let incentive_pool =
            setup_incentive_pool(&mut storage, incentive_pool_balances.clone(), vec![]);
        let user = Addr::unchecked(user);

        let mut total_credits = Uint256::zero();
        for (user, credit) in &existing_credit {
            incentive_pool
                .outstanding_credits
                .save(&mut storage, Addr::unchecked(*user), credit)
                .unwrap();

            total_credits += Uint256::from(*credit);
        }

        incentive_pool
            .total_outstanding_credits
            .save(&mut storage, &total_credits)
            .unwrap();

        let norm_factors = create_norm_factors(&pool_denoms);

        let result =
            incentive_pool.redeem_incentive(&mut storage, &user, redemptions, &norm_factors);

        assert_eq!(result, expected_result, "result mismatch");

        let final_balances = incentive_pool.get_all_pool_balances(&storage).unwrap();
        assert_eq!(final_balances, expected_balances, "balances mismatch");

        let user_credits = incentive_pool
            .get_all_incentive_credits(&storage, None, None)
            .unwrap();
        assert_eq!(
            user_credits,
            expected_user_credits
                .into_iter()
                .map(|(user, credit)| (Addr::unchecked(user), credit))
                .collect::<Vec<_>>()
        );

        let total_credits = incentive_pool
            .get_total_incentive_credits(&storage)
            .unwrap();
        assert_eq!(
            total_credits, expected_total_credits,
            "total credits mismatch"
        );
    }
}
