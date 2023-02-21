mod exit_pool;
mod join_pool;
mod transmute;

use cosmwasm_schema::cw_serde;
use cosmwasm_std::Coin;

#[cw_serde]
pub struct TransmuterPool {
    /// incoming coins are stored here
    pub pool_assets: Vec<Coin>,
}

impl TransmuterPool {
    pub fn new(in_denom: &str, out_denom: &str) -> Self {
        Self {
            pool_assets: vec![Coin::new(0, in_denom), Coin::new(0, out_denom)],
        }
    }
}
