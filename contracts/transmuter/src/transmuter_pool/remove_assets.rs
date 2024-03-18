use crate::{asset::Asset, ContractError};

use super::TransmuterPool;

impl TransmuterPool {
    pub fn remove_assets(&mut self, removing_assets: &[String]) -> Result<(), ContractError> {
        // TODO: check if removing_assets are in the pool_assets

        let removing_assets: Vec<&str> = removing_assets.iter().map(|s| s.as_str()).collect();
        let (removed_assets, remaining_assets): (Vec<Asset>, Vec<Asset>) = self
            .pool_assets
            .drain(..)
            .partition(|asset| removing_assets.contains(&asset.denom()));

        self.pool_assets = remaining_assets;
        self.removed_assets.extend(removed_assets);

        Ok(())
    }
}
