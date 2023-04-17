use cosmwasm_std::{Addr, Coin, StdError, StdResult, Storage, Uint128};
use cw_storage_plus::{Item, Map};

pub struct Shares<'a> {
    shares: Map<'a, &'a Addr, Uint128>,
    total_shares: Item<'a, Uint128>,
}

impl Shares<'_> {
    pub const fn new() -> Self {
        Self {
            shares: Map::new("shares"),
            total_shares: Item::new("total_shares"),
        }
    }

    /// get the total shares
    pub fn get_total_shares(&self, store: &dyn Storage) -> StdResult<Uint128> {
        self.total_shares
            .may_load(store)
            .map(|opt| opt.unwrap_or_default())
    }

    /// get shares for a given address
    pub fn get_share(&self, store: &dyn Storage, address: &Addr) -> StdResult<Uint128> {
        self.shares
            .may_load(store, address)
            .map(|opt| opt.unwrap_or_default())
    }

    /// add shares for a given address
    pub fn add_share(
        &self,
        store: &mut dyn Storage,
        address: &Addr,
        amount: Uint128,
    ) -> StdResult<()> {
        // add the amount to the current shares
        self.shares
            .update(store, address, |current| -> StdResult<_> {
                current
                    .unwrap_or_default()
                    .checked_add(amount)
                    .map_err(StdError::overflow)
            })?;

        // add the amount to the total shares
        let total_shares = self.get_total_shares(store)?;
        self.total_shares.save(
            store,
            &total_shares
                .checked_add(amount)
                .map_err(StdError::overflow)?,
        )?;

        Ok(())
    }

    /// subtract shares for a given address
    pub fn sub_share(
        &self,
        store: &mut dyn Storage,
        address: &Addr,
        amount: Uint128,
    ) -> StdResult<()> {
        // subtract the amount from the current shares
        self.shares
            .update(store, address, |current| -> StdResult<_> {
                current
                    .unwrap_or_default()
                    .checked_sub(amount)
                    .map_err(StdError::overflow)
            })?;

        // subtract the amount from the total shares
        let total_shares = self.get_total_shares(store)?;
        self.total_shares.save(
            store,
            &total_shares
                .checked_sub(amount)
                .map_err(StdError::overflow)?,
        )?;

        Ok(())
    }

    /// calculate the amount of shares for a given amount of tokens
    pub fn calc_shares(tokens: &[Coin]) -> StdResult<Uint128> {
        let mut total = Uint128::zero();
        for coin in tokens {
            total = total.checked_add(coin.amount)?;
        }
        Ok(total)
    }
}
