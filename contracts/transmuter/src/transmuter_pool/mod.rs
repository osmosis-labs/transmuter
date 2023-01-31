mod supply;

use crate::ContractError;
use cosmwasm_std::Coin;

pub struct TransmuterPool {
    /// incoming coins are stored here
    in_coin: Coin,
    /// reserve of coins for future transmutations
    out_coin_reserve: Coin,
}

impl TransmuterPool {
    pub fn new(in_denom: &str, out_denom: &str) -> Self {
        Self {
            in_coin: Coin::new(0, in_denom),
            out_coin_reserve: Coin::new(0, out_denom),
        }
    }

    pub fn transmute(&mut self, coin: &Coin) -> Result<Coin, ContractError> {
        todo!()
    }

    pub fn withdraw(&mut self, coins: &[Coin]) -> Result<(), ContractError> {
        todo!()
    }
}
