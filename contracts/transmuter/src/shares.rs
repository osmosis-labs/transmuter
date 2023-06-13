use cosmwasm_std::{Addr, Coin, Deps, StdError, StdResult, Storage, Uint128};
use cw_storage_plus::Item;
use osmosis_std::types::cosmos::bank::v1beta1::BankQuerier;

pub struct Shares<'a> {
    share_denom: Item<'a, String>,
}

impl Shares<'_> {
    pub const fn new() -> Self {
        Self {
            share_denom: Item::new("share_denom"),
        }
    }

    /// get the share denom
    pub fn get_share_denom(&self, store: &dyn Storage) -> StdResult<String> {
        self.share_denom.load(store)
    }

    /// set the share denom
    pub fn set_share_denom(&self, store: &mut dyn Storage, share_denom: &String) -> StdResult<()> {
        self.share_denom.save(store, share_denom)
    }

    /// get the total shares
    pub fn get_total_shares(&self, deps: Deps) -> StdResult<Uint128> {
        let share_denom = self.get_share_denom(deps.storage)?;
        let bank_querier = BankQuerier::new(&deps.querier);

        bank_querier
            .supply_of(share_denom)?
            .amount
            .ok_or_else(|| StdError::generic_err("No shares"))?
            .amount
            .parse()
    }

    /// get shares for a given address
    pub fn get_share(&self, deps: Deps, address: &Addr) -> StdResult<Uint128> {
        let share_denom = self.get_share_denom(deps.storage)?;
        let bank_querier = BankQuerier::new(&deps.querier);

        bank_querier
            .balance(address.to_string(), share_denom)?
            .balance
            .ok_or_else(|| StdError::generic_err("No shares"))?
            .amount
            .parse()
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
