use cosmwasm_std::{Addr, Coin, Deps, StdError, StdResult, Storage, Uint128};
use cw_storage_plus::Item;
use osmosis_std::types::cosmos::bank::v1beta1::BankQuerier;

/// Alloyed asset represents the shares of the pool
/// and since the pool is a 1:1 multi-asset pool, it act
/// as a composite of the underlying assets and assume 1:1
/// value to the underlying assets.
pub struct AlloyedAsset<'a> {
    alloyed_denom: Item<'a, String>,
}

impl<'a> AlloyedAsset<'a> {
    pub const fn new(alloyed_denom_namespace: &'a str) -> Self {
        Self {
            alloyed_denom: Item::new(alloyed_denom_namespace),
        }
    }

    /// get the alloyed denom
    pub fn get_alloyed_denom(&self, store: &dyn Storage) -> StdResult<String> {
        self.alloyed_denom.load(store)
    }

    /// set the alloyed denom
    pub fn set_alloyed_denom(
        &self,
        store: &mut dyn Storage,
        alloyed_denom: &String,
    ) -> StdResult<()> {
        self.alloyed_denom.save(store, alloyed_denom)
    }

    /// get the total supply of alloyed asset
    /// which is the total shares of the pool
    pub fn get_total_supply(&self, deps: Deps) -> StdResult<Uint128> {
        let alloyed_denom = self.get_alloyed_denom(deps.storage)?;
        let bank_querier = BankQuerier::new(&deps.querier);

        bank_querier
            .supply_of(alloyed_denom)?
            .amount
            .ok_or_else(|| StdError::generic_err("No shares"))?
            .amount
            .parse()
    }

    /// get the balance of alloyed asset for a given address
    pub fn get_balance(&self, deps: Deps, address: &Addr) -> StdResult<Uint128> {
        let alloyed_denom = self.get_alloyed_denom(deps.storage)?;
        let bank_querier = BankQuerier::new(&deps.querier);

        bank_querier
            .balance(address.to_string(), alloyed_denom)?
            .balance
            .ok_or_else(|| StdError::generic_err("No alloyed asset"))?
            .amount
            .parse()
    }

    /// calculate the amount of alloyed asset to mint
    pub fn calc_amount_to_mint(tokens: &[Coin]) -> StdResult<Uint128> {
        let mut total = Uint128::zero();
        for coin in tokens {
            total = total.checked_add(coin.amount)?;
        }
        Ok(total)
    }
}
