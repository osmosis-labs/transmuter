use cosmwasm_std::{Addr, Coin, Storage, Uint128};
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
    total_outstanding_credits: Item<Uint128>,
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
        let pool_balances: Result<Vec<_>, _> = self
            .pool_balances
            .range(storage, None, None, cosmwasm_std::Order::Ascending)
            .collect();
        let pool_balances = pool_balances?;

        let total_pool_normalized_value = pool_balances
            .into_iter()
            .map(|(denom, exact_amount)| {
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
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .try_fold(Uint128::zero(), |acc, amount| acc.checked_add(amount))?;

        // Get current total outstanding credits from accumulator
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
        let updated_total_credits = current_total_credits.checked_add(actual_credit_amount)?;
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
        let updated_total = current_total.checked_sub(total_normalized_cost)?;
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
    pub fn get_total_credits(&self, storage: &dyn Storage) -> Result<Uint128, ContractError> {
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
