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
    pub fn new(pool_asset_denoms: &[String]) -> Self {
        Self {
            pool_assets: pool_asset_denoms
                .into_iter()
                .map(|denom| Coin::new(0, denom))
                .collect(),
        }
    }
}
