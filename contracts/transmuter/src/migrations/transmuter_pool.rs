use std::collections::BTreeMap;

use crate::{asset::Asset, contract::key, transmuter_pool::TransmuterPool, ContractError};
use cosmwasm_schema::cw_serde;
use cosmwasm_std::Storage;
use cw_storage_plus::Item;

#[cw_serde]
pub struct TransmuterPoolV3 {
    pub pool_assets: Vec<Asset>,
    // [to-be-added] pub asset_groups: BTreeMap<String, AssetGroup>,
}

pub fn add_asset_groups_to_transmuter_pool(storage: &mut dyn Storage) -> Result<(), ContractError> {
    let pool_v3: TransmuterPoolV3 = Item::<TransmuterPoolV3>::new(key::POOL).load(storage)?;

    let pool_v4 = TransmuterPool {
        pool_assets: pool_v3.pool_assets,
        asset_groups: BTreeMap::new(),
    };

    Item::<TransmuterPool>::new(key::POOL).save(storage, &pool_v4)?;

    Ok(())
}
