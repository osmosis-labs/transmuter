use cosmwasm_std::{Addr, Coin, Storage, Uint128};
use cw_storage_plus::Map;

use crate::ContractError;

/// Incentive pool state management for rebalancing fees and incentives
pub struct IncentivePool {
    /// Track incentive pool balances by denom (exact amounts)
    pool_balances: Map<String, Uint128>,
    /// Track outstanding credits to users (normalized amounts)
    outstanding_credits: Map<Addr, Uint128>,
}

impl IncentivePool {
    pub const fn new(
        pool_balances_key: &'static str,
        outstanding_credits_key: &'static str,
    ) -> Self {
        Self {
            pool_balances: Map::new(pool_balances_key),
            outstanding_credits: Map::new(outstanding_credits_key),
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
        pool_denom_factors: &[(String, Uint128)], // (denom, normalization_factor) pairs
        std_norm_factor: Uint128,
    ) -> Result<Uint128, ContractError> {
        // Calculate total normalized value of all pool tokens
        let pool_balances: Result<Vec<_>, _> = self
            .pool_balances
            .range(storage, None, None, cosmwasm_std::Order::Ascending)
            .collect();
        let pool_balances = pool_balances?;

        let total_pool_normalized_value = pool_balances
            .into_iter()
            .map(|(denom, exact_amount)| {
                // Find the normalization factor for this denom
                let norm_factor = pool_denom_factors
                    .iter()
                    .find(|(d, _)| d == &denom)
                    .map(|(_, factor)| *factor)
                    .ok_or_else(|| ContractError::InvalidPoolAssetDenom {
                        denom: denom.clone(),
                    })?;

                // Calculate normalized value: exact_amount * norm_factor / std_norm_factor
                exact_amount
                    .checked_multiply_ratio(norm_factor, std_norm_factor)
                    .map_err(|e| ContractError::CheckedMultiplyRatioError(e))
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .try_fold(Uint128::zero(), |acc, amount| acc.checked_add(amount))?;

        // Get current total outstanding credits
        let current_total_credits = self.get_total_credits(storage)?;

        // Calculate maximum additional credits we can give
        let max_additional_credits =
            total_pool_normalized_value.saturating_sub(current_total_credits);

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

        let new_user_credit = current_user_credit.checked_add(actual_credit_amount)?;

        if new_user_credit.is_zero() {
            // Remove the entry if the new credit is zero
            self.outstanding_credits.remove(storage, user.clone());
        } else {
            // Save the updated credit
            self.outstanding_credits
                .save(storage, user.clone(), &new_user_credit)?;
        }

        Ok(actual_credit_amount)
    }

    /// Simple credit function for testing (bypasses pool validation)
    /// This is used for tests that don't need the pool validation logic
    pub fn credit_incentive_to_user_unchecked(
        &self,
        storage: &mut dyn Storage,
        user: &Addr,
        normalized_amount: Uint128,
    ) -> Result<(), ContractError> {
        let current_credit = self
            .outstanding_credits
            .may_load(storage, user.clone())?
            .unwrap_or_default();

        let new_credit = current_credit.checked_add(normalized_amount)?;
        self.outstanding_credits
            .save(storage, user.clone(), &new_credit)?;

        Ok(())
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
        let new_credit = current_credit.checked_sub(total_normalized_cost)?;
        if new_credit.is_zero() {
            self.outstanding_credits.remove(storage, user.clone());
        } else {
            self.outstanding_credits
                .save(storage, user.clone(), &new_credit)?;
        }

        // Then, remove tokens from pool
        for (coin, _) in redemptions {
            let current_balance = self
                .pool_balances
                .may_load(storage, coin.denom.clone())?
                .unwrap_or_default();

            let new_balance = current_balance.checked_sub(coin.amount)?;
            if new_balance.is_zero() {
                self.pool_balances.remove(storage, coin.denom.clone());
            } else {
                self.pool_balances
                    .save(storage, coin.denom.clone(), &new_balance)?;
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
    pub fn get_total_credits(&self, storage: &dyn Storage) -> Result<Uint128, ContractError> {
        let all_credits: Result<Vec<_>, _> = self
            .outstanding_credits
            .range(storage, None, None, cosmwasm_std::Order::Ascending)
            .collect();

        let total_credits = all_credits?
            .into_iter()
            .map(|(_, amount)| amount)
            .try_fold(Uint128::zero(), |acc, amount| acc.checked_add(amount))?;

        Ok(total_credits)
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
    use super::*;
    use cosmwasm_std::testing::mock_dependencies;
    use cosmwasm_std::{coin, Addr};
    use rstest::rstest;

    fn setup_incentive_pool() -> IncentivePool {
        IncentivePool::new("pool_balances", "outstanding_credits")
    }

    #[rstest]
    #[case::empty_pool_single_denom(coin(1000, "uosmo"), Uint128::new(1000))]
    #[case::empty_pool_different_denom(coin(2500, "uatom"), Uint128::new(2500))]
    fn test_add_to_pool(#[case] test_coin: Coin, #[case] expected_balance: Uint128) {
        let mut deps = mock_dependencies();
        let incentive_pool = setup_incentive_pool();

        incentive_pool
            .add_tokens(&mut deps.storage, &test_coin)
            .unwrap();

        let balance = incentive_pool
            .get_pool_balance(&deps.storage, &test_coin.denom)
            .unwrap();
        assert_eq!(balance, expected_balance);
    }

    #[test]
    fn test_add_to_pool_accumulates() {
        let mut deps = mock_dependencies();
        let incentive_pool = setup_incentive_pool();

        // Add tokens multiple times
        incentive_pool
            .add_tokens(&mut deps.storage, &coin(1000, "uosmo"))
            .unwrap();
        incentive_pool
            .add_tokens(&mut deps.storage, &coin(500, "uosmo"))
            .unwrap();

        let balance = incentive_pool
            .get_pool_balance(&deps.storage, "uosmo")
            .unwrap();
        assert_eq!(balance, Uint128::new(1500));
    }

    #[rstest]
    #[case::multiple_denoms(
        vec![coin(1000, "uosmo"), coin(2000, "uatom"), coin(3000, "uusdc")],
        vec![("uatom", 2000), ("uosmo", 1000), ("uusdc", 3000)]
    )]
    fn test_add_different_denoms_to_pool(
        #[case] coins_to_add: Vec<Coin>,
        #[case] expected_balances: Vec<(&str, u128)>,
    ) {
        let mut deps = mock_dependencies();
        let incentive_pool = setup_incentive_pool();

        for coin in coins_to_add {
            incentive_pool.add_tokens(&mut deps.storage, &coin).unwrap();
        }

        for (denom, expected_amount) in expected_balances {
            let balance = incentive_pool
                .get_pool_balance(&deps.storage, denom)
                .unwrap();
            assert_eq!(balance, Uint128::new(expected_amount));
        }
    }

    #[rstest]
    #[case::partial_removal(1000, 300, 700)]
    #[case::complete_removal(1000, 1000, 0)]
    fn test_remove_from_pool(
        #[case] initial_amount: u128,
        #[case] remove_amount: u128,
        #[case] expected_remaining: u128,
    ) {
        let mut deps = mock_dependencies();
        let incentive_pool = setup_incentive_pool();

        incentive_pool
            .add_tokens(&mut deps.storage, &coin(initial_amount, "uosmo"))
            .unwrap();
        incentive_pool
            .remove_tokens(&mut deps.storage, &coin(remove_amount, "uosmo"))
            .unwrap();

        let balance = incentive_pool
            .get_pool_balance(&deps.storage, "uosmo")
            .unwrap();
        assert_eq!(balance, Uint128::new(expected_remaining));
    }

    #[rstest]
    #[case::remove_more_than_available(1000, 1500)]
    #[case::remove_from_empty_pool(0, 100)]
    fn test_remove_from_pool_insufficient_balance(
        #[case] initial_amount: u128,
        #[case] remove_amount: u128,
    ) {
        let mut deps = mock_dependencies();
        let incentive_pool = setup_incentive_pool();

        if initial_amount > 0 {
            incentive_pool
                .add_tokens(&mut deps.storage, &coin(initial_amount, "uosmo"))
                .unwrap();
        }

        let result = incentive_pool.remove_tokens(&mut deps.storage, &coin(remove_amount, "uosmo"));
        assert!(matches!(
            result,
            Err(ContractError::InsufficientIncentivePool { .. })
        ));
    }

    #[rstest]
    #[case::single_user_credit(100)]
    #[case::large_credit(1000000)]
    fn test_add_credit_to_user(#[case] credit_amount: u128) {
        let mut deps = mock_dependencies();
        let incentive_pool = setup_incentive_pool();
        let user = Addr::unchecked("user1");

        incentive_pool
            .credit_incentive_to_user_unchecked(
                &mut deps.storage,
                &user,
                Uint128::new(credit_amount),
            )
            .unwrap();

        let credit = incentive_pool
            .get_user_credit(&deps.storage, &user)
            .unwrap();
        assert_eq!(credit, Uint128::new(credit_amount));
    }

    #[test]
    fn test_add_credit_to_multiple_users() {
        let mut deps = mock_dependencies();
        let incentive_pool = setup_incentive_pool();
        let user1 = Addr::unchecked("user1");
        let user2 = Addr::unchecked("user2");

        incentive_pool
            .credit_incentive_to_user_unchecked(&mut deps.storage, &user1, Uint128::new(300))
            .unwrap();
        incentive_pool
            .credit_incentive_to_user_unchecked(&mut deps.storage, &user2, Uint128::new(200))
            .unwrap();

        assert_eq!(
            incentive_pool
                .get_user_credit(&deps.storage, &user1)
                .unwrap(),
            Uint128::new(300)
        );
        assert_eq!(
            incentive_pool
                .get_user_credit(&deps.storage, &user2)
                .unwrap(),
            Uint128::new(200)
        );
        assert_eq!(
            incentive_pool.get_total_credits(&deps.storage).unwrap(),
            Uint128::new(500)
        );
    }

    #[test]
    fn test_add_credit_accumulates() {
        let mut deps = mock_dependencies();
        let incentive_pool = setup_incentive_pool();
        let user = Addr::unchecked("user1");

        incentive_pool
            .credit_incentive_to_user_unchecked(&mut deps.storage, &user, Uint128::new(100))
            .unwrap();
        incentive_pool
            .credit_incentive_to_user_unchecked(&mut deps.storage, &user, Uint128::new(200))
            .unwrap();

        let credit = incentive_pool
            .get_user_credit(&deps.storage, &user)
            .unwrap();
        assert_eq!(credit, Uint128::new(300));
    }

    #[test]
    fn test_redeem_incentive_single_token() {
        let mut deps = mock_dependencies();
        let incentive_pool = setup_incentive_pool();
        let user = Addr::unchecked("user1");

        // Add credit to user
        incentive_pool
            .credit_incentive_to_user_unchecked(&mut deps.storage, &user, Uint128::new(300))
            .unwrap();

        // Add tokens to pool
        incentive_pool
            .add_tokens(&mut deps.storage, &coin(1000, "uosmo"))
            .unwrap();

        // Redeem partial amount
        let redemptions = vec![(coin(500, "uosmo"), Uint128::new(100))];
        incentive_pool
            .redeem_incentive(&mut deps.storage, &user, redemptions)
            .unwrap();

        // Check remaining credit
        let remaining_credit = incentive_pool
            .get_user_credit(&deps.storage, &user)
            .unwrap();
        assert_eq!(remaining_credit, Uint128::new(200));

        // Check remaining pool balance
        let remaining_balance = incentive_pool
            .get_pool_balance(&deps.storage, "uosmo")
            .unwrap();
        assert_eq!(remaining_balance, Uint128::new(500));
    }

    #[test]
    fn test_redeem_incentive_multiple_tokens() {
        let mut deps = mock_dependencies();
        let incentive_pool = setup_incentive_pool();
        let user = Addr::unchecked("user1");

        // Add credit to user
        incentive_pool
            .credit_incentive_to_user_unchecked(&mut deps.storage, &user, Uint128::new(500))
            .unwrap();

        // Add tokens to pool
        incentive_pool
            .add_tokens(&mut deps.storage, &coin(1000, "uosmo"))
            .unwrap();
        incentive_pool
            .add_tokens(&mut deps.storage, &coin(2000, "uatom"))
            .unwrap();

        // Redeem multiple tokens
        let redemptions = vec![
            (coin(500, "uosmo"), Uint128::new(200)),
            (coin(1000, "uatom"), Uint128::new(300)),
        ];
        incentive_pool
            .redeem_incentive(&mut deps.storage, &user, redemptions)
            .unwrap();

        // Check remaining credit (should be zero)
        let remaining_credit = incentive_pool
            .get_user_credit(&deps.storage, &user)
            .unwrap();
        assert_eq!(remaining_credit, Uint128::zero());

        // Check remaining pool balances
        assert_eq!(
            incentive_pool
                .get_pool_balance(&deps.storage, "uosmo")
                .unwrap(),
            Uint128::new(500)
        );
        assert_eq!(
            incentive_pool
                .get_pool_balance(&deps.storage, "uatom")
                .unwrap(),
            Uint128::new(1000)
        );
    }

    #[test]
    fn test_redeem_incentive_insufficient_credit() {
        let mut deps = mock_dependencies();
        let incentive_pool = setup_incentive_pool();
        let user = Addr::unchecked("user1");

        // Add limited credit to user
        incentive_pool
            .credit_incentive_to_user_unchecked(&mut deps.storage, &user, Uint128::new(100))
            .unwrap();

        // Add tokens to pool
        incentive_pool
            .add_tokens(&mut deps.storage, &coin(1000, "uosmo"))
            .unwrap();

        // Try to redeem more than available credit
        let redemptions = vec![(coin(500, "uosmo"), Uint128::new(200))];
        let result = incentive_pool.redeem_incentive(&mut deps.storage, &user, redemptions);

        assert!(matches!(
            result,
            Err(ContractError::InsufficientIncentiveCredit { .. })
        ));
    }

    #[test]
    fn test_redeem_incentive_insufficient_pool_balance() {
        let mut deps = mock_dependencies();
        let incentive_pool = setup_incentive_pool();
        let user = Addr::unchecked("user1");

        // Add credit to user
        incentive_pool
            .credit_incentive_to_user_unchecked(&mut deps.storage, &user, Uint128::new(500))
            .unwrap();

        // Add limited tokens to pool
        incentive_pool
            .add_tokens(&mut deps.storage, &coin(500, "uosmo"))
            .unwrap();

        // Try to redeem more tokens than available in pool
        let redemptions = vec![(coin(1000, "uosmo"), Uint128::new(200))];
        let result = incentive_pool.redeem_incentive(&mut deps.storage, &user, redemptions);

        assert!(matches!(
            result,
            Err(ContractError::InsufficientIncentivePool { .. })
        ));
    }

    #[test]
    fn test_get_all_pool_balances() {
        let mut deps = mock_dependencies();
        let incentive_pool = setup_incentive_pool();

        incentive_pool
            .add_tokens(&mut deps.storage, &coin(1000, "uosmo"))
            .unwrap();
        incentive_pool
            .add_tokens(&mut deps.storage, &coin(2000, "uatom"))
            .unwrap();
        incentive_pool
            .add_tokens(&mut deps.storage, &coin(3000, "uusdc"))
            .unwrap();

        let balances = incentive_pool.get_all_pool_balances(&deps.storage).unwrap();

        // Should be sorted by denom
        assert_eq!(balances.len(), 3);
        assert_eq!(balances[0], coin(2000, "uatom"));
        assert_eq!(balances[1], coin(1000, "uosmo"));
        assert_eq!(balances[2], coin(3000, "uusdc"));
    }

    #[test]
    fn test_get_all_credit_users() {
        let mut deps = mock_dependencies();
        let incentive_pool = setup_incentive_pool();
        let user1 = Addr::unchecked("user1");
        let user2 = Addr::unchecked("user2");

        incentive_pool
            .credit_incentive_to_user_unchecked(&mut deps.storage, &user1, Uint128::new(300))
            .unwrap();
        incentive_pool
            .credit_incentive_to_user_unchecked(&mut deps.storage, &user2, Uint128::new(500))
            .unwrap();

        let credit_users = incentive_pool.get_all_credit_users(&deps.storage).unwrap();

        assert_eq!(credit_users.len(), 2);
        assert_eq!(credit_users[0], (user1, Uint128::new(300)));
        assert_eq!(credit_users[1], (user2, Uint128::new(500)));
    }

    #[test]
    fn test_zero_queries() {
        let deps = mock_dependencies();
        let incentive_pool = setup_incentive_pool();
        let user = Addr::unchecked("user1");

        // All queries should return zero/empty for non-existent data
        assert_eq!(
            incentive_pool
                .get_pool_balance(&deps.storage, "uosmo")
                .unwrap(),
            Uint128::zero()
        );
        assert_eq!(
            incentive_pool.get_total_credits(&deps.storage).unwrap(),
            Uint128::zero()
        );
        assert_eq!(
            incentive_pool
                .get_user_credit(&deps.storage, &user)
                .unwrap(),
            Uint128::zero()
        );
        assert!(incentive_pool
            .get_all_pool_balances(&deps.storage)
            .unwrap()
            .is_empty());
        assert!(incentive_pool
            .get_all_credit_users(&deps.storage)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn test_zero_amount_operations() {
        let mut deps = mock_dependencies();
        let incentive_pool = setup_incentive_pool();
        let user = Addr::unchecked("user1");

        // Test adding zero tokens - should work but not change state
        incentive_pool
            .add_tokens(&mut deps.storage, &coin(0, "uosmo"))
            .unwrap();
        assert_eq!(
            incentive_pool
                .get_pool_balance(&deps.storage, "uosmo")
                .unwrap(),
            Uint128::zero()
        );

        // Test crediting zero amount - should work but not change state
        incentive_pool
            .credit_incentive_to_user_unchecked(&mut deps.storage, &user, Uint128::zero())
            .unwrap();
        assert_eq!(
            incentive_pool
                .get_user_credit(&deps.storage, &user)
                .unwrap(),
            Uint128::zero()
        );

        // Test redemption with zero cost - should work
        incentive_pool
            .add_tokens(&mut deps.storage, &coin(1000, "uosmo"))
            .unwrap();
        let redemptions = vec![(coin(500, "uosmo"), Uint128::zero())];
        incentive_pool
            .redeem_incentive(&mut deps.storage, &user, redemptions)
            .unwrap();

        // Pool should have tokens removed despite zero cost
        assert_eq!(
            incentive_pool
                .get_pool_balance(&deps.storage, "uosmo")
                .unwrap(),
            Uint128::new(500)
        );
    }

    #[test]
    fn test_empty_redemption_list() {
        let mut deps = mock_dependencies();
        let incentive_pool = setup_incentive_pool();
        let user = Addr::unchecked("user1");

        // Add some credit to user
        incentive_pool
            .credit_incentive_to_user_unchecked(&mut deps.storage, &user, Uint128::new(100))
            .unwrap();

        // Empty redemption list should succeed without changes
        let redemptions = vec![];
        incentive_pool
            .redeem_incentive(&mut deps.storage, &user, redemptions)
            .unwrap();

        // Credit should remain unchanged
        assert_eq!(
            incentive_pool
                .get_user_credit(&deps.storage, &user)
                .unwrap(),
            Uint128::new(100)
        );
    }

    #[test]
    fn test_duplicate_tokens_in_redemption() {
        let mut deps = mock_dependencies();
        let incentive_pool = setup_incentive_pool();
        let user = Addr::unchecked("user1");

        // Setup: credit user and add tokens to pool
        incentive_pool
            .credit_incentive_to_user_unchecked(&mut deps.storage, &user, Uint128::new(200))
            .unwrap();
        incentive_pool
            .add_tokens(&mut deps.storage, &coin(1000, "uosmo"))
            .unwrap();

        // Duplicate denoms in redemption list
        let redemptions = vec![
            (coin(100, "uosmo"), Uint128::new(50)),
            (coin(200, "uosmo"), Uint128::new(75)),
        ];
        incentive_pool
            .redeem_incentive(&mut deps.storage, &user, redemptions)
            .unwrap();

        // Should deduct total: 100 + 200 = 300 tokens, 50 + 75 = 125 credits
        assert_eq!(
            incentive_pool
                .get_pool_balance(&deps.storage, "uosmo")
                .unwrap(),
            Uint128::new(700) // 1000 - 300
        );
        assert_eq!(
            incentive_pool
                .get_user_credit(&deps.storage, &user)
                .unwrap(),
            Uint128::new(75) // 200 - 125
        );
    }

    #[test]
    fn test_exact_boundary_conditions() {
        let mut deps = mock_dependencies();
        let incentive_pool = setup_incentive_pool();
        let user = Addr::unchecked("user1");

        // Test exact credit redemption
        incentive_pool
            .credit_incentive_to_user_unchecked(&mut deps.storage, &user, Uint128::new(100))
            .unwrap();
        incentive_pool
            .add_tokens(&mut deps.storage, &coin(500, "uosmo"))
            .unwrap();

        let redemptions = vec![(coin(300, "uosmo"), Uint128::new(100))];
        incentive_pool
            .redeem_incentive(&mut deps.storage, &user, redemptions)
            .unwrap();

        // User should have zero credits, pool should have remaining tokens
        assert_eq!(
            incentive_pool
                .get_user_credit(&deps.storage, &user)
                .unwrap(),
            Uint128::zero()
        );
        assert_eq!(
            incentive_pool
                .get_pool_balance(&deps.storage, "uosmo")
                .unwrap(),
            Uint128::new(200)
        );
    }

    #[test]
    fn test_exact_pool_balance_redemption() {
        let mut deps = mock_dependencies();
        let incentive_pool = setup_incentive_pool();
        let user = Addr::unchecked("user1");

        // Pool has exactly 500 tokens, user redeems exactly 500 tokens
        incentive_pool
            .credit_incentive_to_user_unchecked(&mut deps.storage, &user, Uint128::new(200))
            .unwrap();
        incentive_pool
            .add_tokens(&mut deps.storage, &coin(500, "uosmo"))
            .unwrap();

        let redemptions = vec![(coin(500, "uosmo"), Uint128::new(150))];
        incentive_pool
            .redeem_incentive(&mut deps.storage, &user, redemptions)
            .unwrap();

        // Pool should have zero tokens (and be removed from storage)
        assert_eq!(
            incentive_pool
                .get_pool_balance(&deps.storage, "uosmo")
                .unwrap(),
            Uint128::zero()
        );
        assert_eq!(
            incentive_pool
                .get_user_credit(&deps.storage, &user)
                .unwrap(),
            Uint128::new(50)
        );
    }

    #[test]
    fn test_state_cleanup_after_complete_operations() {
        let mut deps = mock_dependencies();
        let incentive_pool = setup_incentive_pool();
        let user = Addr::unchecked("user1");

        // Add tokens and credits, then remove everything
        incentive_pool
            .add_tokens(&mut deps.storage, &coin(1000, "uosmo"))
            .unwrap();
        incentive_pool
            .credit_incentive_to_user_unchecked(&mut deps.storage, &user, Uint128::new(200))
            .unwrap();

        // Remove all tokens
        incentive_pool
            .remove_tokens(&mut deps.storage, &coin(1000, "uosmo"))
            .unwrap();

        // Redeem all credits (should work even with no tokens since we're testing edge case)
        let redemptions = vec![(coin(0, "uatom"), Uint128::new(200))];
        incentive_pool
            .redeem_incentive(&mut deps.storage, &user, redemptions)
            .unwrap();

        // Both should return to empty state
        assert!(incentive_pool
            .get_all_pool_balances(&deps.storage)
            .unwrap()
            .is_empty());
        assert!(incentive_pool
            .get_all_credit_users(&deps.storage)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn test_overflow_protection() {
        let mut deps = mock_dependencies();
        let incentive_pool = setup_incentive_pool();
        let user = Addr::unchecked("user1");

        // Test adding to existing near-max values
        incentive_pool
            .add_tokens(&mut deps.storage, &coin(u128::MAX - 100, "uosmo"))
            .unwrap();

        // Adding a small amount should work
        incentive_pool
            .add_tokens(&mut deps.storage, &coin(50, "uosmo"))
            .unwrap();

        // Adding too much should cause overflow error
        let result = incentive_pool.add_tokens(&mut deps.storage, &coin(100, "uosmo"));
        assert!(result.is_err());

        // Same for credits
        incentive_pool
            .credit_incentive_to_user_unchecked(
                &mut deps.storage,
                &user,
                Uint128::new(u128::MAX - 100),
            )
            .unwrap();

        let result = incentive_pool.credit_incentive_to_user_unchecked(
            &mut deps.storage,
            &user,
            Uint128::new(200),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_partial_redemption_failure_atomicity() {
        let mut deps = mock_dependencies();
        let incentive_pool = setup_incentive_pool();
        let user = Addr::unchecked("user1");

        // Setup: user has credits, pool has some tokens but not all
        incentive_pool
            .credit_incentive_to_user_unchecked(&mut deps.storage, &user, Uint128::new(500))
            .unwrap();
        incentive_pool
            .add_tokens(&mut deps.storage, &coin(1000, "uosmo"))
            .unwrap();
        // No uatom in pool

        // Try to redeem both uosmo (available) and uatom (not available)
        let redemptions = vec![
            (coin(500, "uosmo"), Uint128::new(200)),
            (coin(500, "uatom"), Uint128::new(200)), // This should fail
        ];

        let result = incentive_pool.redeem_incentive(&mut deps.storage, &user, redemptions);
        assert!(matches!(
            result,
            Err(ContractError::InsufficientIncentivePool { .. })
        ));

        // State should be unchanged (atomic failure)
        assert_eq!(
            incentive_pool
                .get_user_credit(&deps.storage, &user)
                .unwrap(),
            Uint128::new(500) // Unchanged
        );
        assert_eq!(
            incentive_pool
                .get_pool_balance(&deps.storage, "uosmo")
                .unwrap(),
            Uint128::new(1000) // Unchanged
        );
    }

    #[test]
    fn test_multiple_users_competing_workflow() {
        let mut deps = mock_dependencies();
        let incentive_pool = setup_incentive_pool();
        let user1 = Addr::unchecked("user1");
        let user2 = Addr::unchecked("user2");

        // Setup: Limited pool, multiple users with credits
        incentive_pool
            .add_tokens(&mut deps.storage, &coin(1000, "uosmo"))
            .unwrap();
        incentive_pool
            .credit_incentive_to_user_unchecked(&mut deps.storage, &user1, Uint128::new(300))
            .unwrap();
        incentive_pool
            .credit_incentive_to_user_unchecked(&mut deps.storage, &user2, Uint128::new(400))
            .unwrap();

        // User1 redeems first
        let redemptions1 = vec![(coin(600, "uosmo"), Uint128::new(200))];
        incentive_pool
            .redeem_incentive(&mut deps.storage, &user1, redemptions1)
            .unwrap();

        // User2 tries to redeem more than remaining
        let redemptions2 = vec![(coin(500, "uosmo"), Uint128::new(300))];
        let result = incentive_pool.redeem_incentive(&mut deps.storage, &user2, redemptions2);
        assert!(matches!(
            result,
            Err(ContractError::InsufficientIncentivePool { .. })
        ));

        // User2 can redeem what's left
        let redemptions2_valid = vec![(coin(400, "uosmo"), Uint128::new(300))];
        incentive_pool
            .redeem_incentive(&mut deps.storage, &user2, redemptions2_valid)
            .unwrap();

        // Final state check
        assert_eq!(
            incentive_pool
                .get_user_credit(&deps.storage, &user1)
                .unwrap(),
            Uint128::new(100) // 300 - 200
        );
        assert_eq!(
            incentive_pool
                .get_user_credit(&deps.storage, &user2)
                .unwrap(),
            Uint128::new(100) // 400 - 300
        );
        assert_eq!(
            incentive_pool
                .get_pool_balance(&deps.storage, "uosmo")
                .unwrap(),
            Uint128::zero() // 1000 - 600 - 400 = 0
        );
    }

    #[test]
    fn test_full_lifecycle_workflow() {
        let mut deps = mock_dependencies();
        let incentive_pool = setup_incentive_pool();
        let user1 = Addr::unchecked("user1");
        let user2 = Addr::unchecked("user2");

        // Phase 1: Add tokens from fees
        incentive_pool
            .add_tokens(&mut deps.storage, &coin(1000, "uosmo"))
            .unwrap();
        incentive_pool
            .add_tokens(&mut deps.storage, &coin(2000, "uatom"))
            .unwrap();

        // Phase 2: Credit users for their rebalancing work
        incentive_pool
            .credit_incentive_to_user_unchecked(&mut deps.storage, &user1, Uint128::new(400))
            .unwrap();
        incentive_pool
            .credit_incentive_to_user_unchecked(&mut deps.storage, &user2, Uint128::new(600))
            .unwrap();

        // Phase 3: Users redeem their incentives
        let redemptions1 = vec![(coin(500, "uosmo"), Uint128::new(250))];
        incentive_pool
            .redeem_incentive(&mut deps.storage, &user1, redemptions1)
            .unwrap();

        let redemptions2 = vec![
            (coin(300, "uosmo"), Uint128::new(200)),
            (coin(800, "uatom"), Uint128::new(350)),
        ];
        incentive_pool
            .redeem_incentive(&mut deps.storage, &user2, redemptions2)
            .unwrap();

        // Phase 4: Final state verification
        assert_eq!(
            incentive_pool.get_total_credits(&deps.storage).unwrap(),
            Uint128::new(200) // 400 - 250 + 600 - 550 = 200
        );

        let remaining_balances = incentive_pool.get_all_pool_balances(&deps.storage).unwrap();
        assert_eq!(remaining_balances.len(), 2);
        // uatom: 2000 - 800 = 1200, uosmo: 1000 - 500 - 300 = 200
        assert!(remaining_balances.contains(&coin(1200, "uatom")));
        assert!(remaining_balances.contains(&coin(200, "uosmo")));
    }

    #[test]
    fn test_credit_incentive_with_pool_validation() {
        let mut deps = mock_dependencies();
        let incentive_pool = setup_incentive_pool();
        let user1 = Addr::unchecked("user1");
        let user2 = Addr::unchecked("user2");

        // Add tokens to pool with different amounts
        incentive_pool
            .add_tokens(&mut deps.storage, &coin(1000, "uosmo"))
            .unwrap();
        incentive_pool
            .add_tokens(&mut deps.storage, &coin(2000, "uatom"))
            .unwrap();

        // Setup normalization factors: uosmo=1, uatom=2, std_norm=2
        let pool_denom_factors = vec![
            ("uosmo".to_string(), Uint128::new(1)),
            ("uatom".to_string(), Uint128::new(2)),
        ];
        let std_norm_factor = Uint128::new(2);

        // Total normalized pool value = (1000 * 1 / 2) + (2000 * 2 / 2) = 500 + 2000 = 2500

        // Credit user1 with 1000 (should succeed)
        let credited1 = incentive_pool
            .credit_incentive_to_user(
                &mut deps.storage,
                &user1,
                Uint128::new(1000),
                &pool_denom_factors,
                std_norm_factor,
            )
            .unwrap();
        assert_eq!(credited1, Uint128::new(1000));

        // Credit user2 with 2000 (should be capped to 1500 remaining)
        let credited2 = incentive_pool
            .credit_incentive_to_user(
                &mut deps.storage,
                &user2,
                Uint128::new(2000),
                &pool_denom_factors,
                std_norm_factor,
            )
            .unwrap();
        assert_eq!(credited2, Uint128::new(1500)); // Capped to remaining pool value

        // Verify final credits
        assert_eq!(
            incentive_pool
                .get_user_credit(&deps.storage, &user1)
                .unwrap(),
            Uint128::new(1000)
        );
        assert_eq!(
            incentive_pool
                .get_user_credit(&deps.storage, &user2)
                .unwrap(),
            Uint128::new(1500)
        );

        // Total credits should equal total pool normalized value
        assert_eq!(
            incentive_pool.get_total_credits(&deps.storage).unwrap(),
            Uint128::new(2500)
        );

        // Try to credit more (should return 0)
        let credited3 = incentive_pool
            .credit_incentive_to_user(
                &mut deps.storage,
                &user1,
                Uint128::new(100),
                &pool_denom_factors,
                std_norm_factor,
            )
            .unwrap();
        assert_eq!(credited3, Uint128::zero());
    }
}
