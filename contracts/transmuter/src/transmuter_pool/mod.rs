mod supply;
mod transmute;

use crate::ContractError;
use cosmwasm_schema::cw_serde;
use cosmwasm_std::Coin;

#[cw_serde]
pub struct TransmuterPool {
    /// incoming coins are stored here
    pub in_coin: Coin,
    /// reserve of coins for future transmutations
    pub out_coin_reserve: Coin,
}

impl TransmuterPool {
    pub fn new(in_denom: &str, out_denom: &str) -> Self {
        Self {
            in_coin: Coin::new(0, in_denom),
            out_coin_reserve: Coin::new(0, out_denom),
        }
    }

    pub fn withdraw(&mut self, coins: &[Coin]) -> Result<(), ContractError> {
        todo!()
    }
}
