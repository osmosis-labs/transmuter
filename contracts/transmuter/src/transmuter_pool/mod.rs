mod exit_pool;
mod join_pool;
mod transmute;
mod weight;

use cosmwasm_schema::cw_serde;
use cosmwasm_std::Coin;

#[cw_serde]
pub struct TransmuterPool {
    /// incoming coins are stored here
    pub pool_assets: Vec<Coin>,
}

impl TransmuterPool {
    pub fn new(pool_asset_denoms: &[String]) -> Self {
        assert!(pool_asset_denoms.len() >= 2);

        Self {
            pool_assets: pool_asset_denoms
                .iter()
                .map(|denom| Coin::new(0, denom))
                .collect(),
        }
    }
}
