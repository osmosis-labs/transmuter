use std::collections::BTreeMap;

use crate::scope::Scope;

use crate::asset::Asset;
use crate::{contract::Transmuter, transmuter_pool::TransmuterPool};
use cosmwasm_std::Decimal;
use cosmwasm_std::{
    coin,
    testing::{MockApi, MockQuerier, MockStorage},
    Addr, OwnedDeps, Uint128,
};
use transmuter_math::rebalancing::config::RebalancingConfig;

pub fn setup_fee_deduction_test() -> (Addr, OwnedDeps<MockStorage, MockApi, MockQuerier>) {
    let sender = Addr::unchecked("sender");
    let mut deps = cosmwasm_std::testing::mock_dependencies_with_balances(&[(
        sender.to_string().as_str(),
        &[coin(20_000_000_000_000u128, "alloyed")],
    )]);

    let transmuter = Transmuter::new();
    transmuter
        .alloyed_asset
        .set_alloyed_denom(&mut deps.storage, &"alloyed".to_string())
        .unwrap();

    transmuter
        .alloyed_asset
        .set_normalization_factor(&mut deps.storage, 100u128.into())
        .unwrap();

    transmuter
        .pool
        .save(
            &mut deps.storage,
            &TransmuterPool {
                pool_assets: vec![
                    Asset::new(Uint128::from(100_000_000_000u128), "denom1", 1u128).unwrap(), // normalized = 10_000_000_000_000
                    Asset::new(Uint128::from(500_000_000_000u128), "denom2", 10u128).unwrap(), // normalized = 5_000_000_000_000
                    Asset::new(Uint128::from(5_000_000_000_000u128), "denom3", 100u128) // normalized = 5_000_000_000_000
                        .unwrap(),
                ],
                asset_groups: BTreeMap::new(),
            },
        )
        .unwrap();

    let mut pool = transmuter.pool.load(&deps.storage).unwrap();
    pool.create_asset_group(
        "group1".to_string(),
        vec!["denom2".to_string(), "denom3".to_string()],
    )
    .unwrap();
    transmuter.pool.save(&mut deps.storage, &pool).unwrap();

    transmuter
        .rebalancer
        .add_config(
            &mut deps.storage,
            Scope::denom("denom1"),
            RebalancingConfig::new(
                Decimal::percent(50),
                Decimal::percent(45),
                Decimal::percent(55),
                Decimal::percent(30),
                Decimal::percent(65),
                Decimal::percent(10),
                Decimal::percent(20),
            )
            .unwrap(),
        )
        .unwrap();

    transmuter
        .rebalancer
        .add_config(
            &mut deps.storage,
            Scope::asset_group("group1"),
            RebalancingConfig::new(
                Decimal::percent(55),
                Decimal::percent(45),
                Decimal::percent(60),
                Decimal::percent(30),
                Decimal::percent(65),
                Decimal::percent(10),
                Decimal::percent(20),
            )
            .unwrap(),
        )
        .unwrap();

    transmuter
        .incentive_pool
        .add_tokens(&mut deps.storage, &coin(100_000_000_000, "denom1"))
        .unwrap();
    transmuter
        .incentive_pool
        .add_tokens(&mut deps.storage, &coin(1_000_000_000_000, "denom2"))
        .unwrap();
    transmuter
        .incentive_pool
        .add_tokens(&mut deps.storage, &coin(10_000_000_000_000, "denom3"))
        .unwrap();

    return (sender, deps);
}
