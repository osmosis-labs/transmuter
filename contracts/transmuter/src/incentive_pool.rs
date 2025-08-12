use cosmwasm_std::{Coin, Storage, Uint128};
use cw_storage_plus::Map;

use crate::ContractError;

/// Incentive pool state management for rebalancing fees and incentives
pub struct IncentivePool {
    /// Track incentive pool balances by denom (exact amounts)
    pool_balances: Map<String, Uint128>,
}

impl IncentivePool {
    pub const fn new(pool_balances_key: &'static str) -> Self {
        Self {
            pool_balances: Map::new(pool_balances_key),
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
}

#[cfg(test)]
mod tests {
    use cosmwasm_std::{coin, testing::MockStorage};
    use rstest::rstest;

    use super::*;

    const POOL_BALANCES_KEY: &str = "pool_balances";

    fn setup_incentive_pool(storage: &mut MockStorage, pool_balances: Vec<Coin>) -> IncentivePool {
        let pool_balances_storage = Map::new(POOL_BALANCES_KEY);
        for coin in pool_balances {
            pool_balances_storage
                .save(storage, &coin.denom, &coin.amount)
                .unwrap();
        }

        IncentivePool::new(POOL_BALANCES_KEY)
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
        let incentive_pool = setup_incentive_pool(&mut storage, pool_balances);
        incentive_pool
            .add_tokens(&mut storage, &additional_token)
            .unwrap();

        let pool_balances = incentive_pool.get_all_pool_balances(&storage).unwrap();
        assert_eq!(pool_balances, expected_pool_balances);
    }

    #[test]
    fn test_add_tokens_overflow() {
        let mut storage = MockStorage::new();
        let incentive_pool = setup_incentive_pool(&mut storage, vec![]);

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
        let incentive_pool = setup_incentive_pool(&mut storage, vec![]);

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
        let incentive_pool = setup_incentive_pool(&mut storage, pool_balances.clone());

        let result = incentive_pool.remove_tokens(&mut storage, &token_to_remove);
        assert_eq!(result, expected);

        let balances = incentive_pool.get_all_pool_balances(&storage).unwrap();
        assert_eq!(balances, expected_balances);
    }
}
