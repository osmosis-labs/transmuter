use cosmwasm_std::{coin, Decimal, Uint128};
use osmosis_test_tube::{Account, Module};
use transmuter_math::rebalancing::config::RebalancingConfig;

use crate::test::modules::cosmwasm_pool::CosmwasmPool;
use crate::{
    asset::AssetConfig, contract::sv::ExecMsg, scope::Scope, test::test_env::TestEnvBuilder,
};
use osmosis_std::types::osmosis::poolmanager::v1beta1::{
    MsgSwapExactAmountOut, SwapAmountOutRoute,
};

const REMAINING_DENOM0: u128 = 1_000_000_000_000;
const REMAINING_DENOM1: u128 = 1_000_000_000_000;
const REMAINING_DENOM2: u128 = 1_000_000_000_000;

#[test]
fn test_swap_with_asset_group_limiters() {
    let app = osmosis_test_tube::OsmosisTestApp::new();
    let admin = app.init_account(&[coin(100_000u128, "uosmo")]).unwrap();
    let cp = CosmwasmPool::new(&app);

    let t = TestEnvBuilder::new()
        .with_account("admin", vec![])
        .with_account("user", vec![coin(1_000_000, "denom0")])
        .with_account(
            "provider",
            vec![
                coin(REMAINING_DENOM0, "denom0"),
                coin(REMAINING_DENOM1, "denom1"),
                coin(REMAINING_DENOM2, "denom2"),
            ],
        )
        .with_instantiate_msg(crate::contract::sv::InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig {
                    denom: "denom0".to_string(),
                    normalization_factor: Uint128::one(),
                },
                AssetConfig {
                    denom: "denom1".to_string(),
                    normalization_factor: Uint128::one(),
                },
                AssetConfig {
                    denom: "denom2".to_string(),
                    normalization_factor: Uint128::one(),
                },
            ],
            alloyed_asset_subdenom: "usd".to_string(),
            alloyed_asset_normalization_factor: Uint128::one(),
            admin: Some(admin.address()),
            moderator: "osmo1cyyzpxplxdzkeea7kwsydadg87357qnahakaks".to_string(),
        })
        .build(&app);

    // Add initial liquidity to the pool
    t.contract
        .execute(
            &ExecMsg::JoinPool {},
            &[
                coin(500_000, "denom0"),
                coin(500_000, "denom1"),
                coin(500_000, "denom2"),
            ],
            &t.accounts["provider"],
        )
        .unwrap();

    // Add asset group and static limiter at 67%
    t.contract
        .execute(
            &ExecMsg::CreateAssetGroup {
                label: "group1".to_string(),
                denoms: vec!["denom0".to_string(), "denom1".to_string()],
            },
            &[],
            &t.accounts["admin"],
        )
        .unwrap();

    t.contract
        .execute(
            &&ExecMsg::AddRebalancingConfig {
                scope: Scope::asset_group("group1"),
                rebalancing_config: RebalancingConfig::limit_only(Decimal::percent(67)).unwrap(),
            },
            &[],
            &t.accounts["admin"],
        )
        .unwrap();

    // Swap within group (denom0 -> denom1): should succeed
    cp.swap_exact_amount_out(
        MsgSwapExactAmountOut {
            sender: t.accounts["provider"].address(),
            routes: vec![SwapAmountOutRoute {
                pool_id: t.contract.pool_id,
                token_in_denom: "denom0".to_string(),
            }],
            token_out: Some(coin(1_000, "denom1").into()),
            token_in_max_amount: Uint128::from(10_000u128).to_string(),
        },
        &t.accounts["provider"],
    )
    .unwrap();

    // Join with denom0 (increase group1 weight): should fail because it would push group1 above 67%
    let err = t
        .contract
        .execute(
            &ExecMsg::JoinPool {},
            &[coin(100_000, "denom0")],
            &t.accounts["user"],
        )
        .unwrap_err();
    assert!(err.to_string().contains("Upper limit exceeded"));

    // Swap denom0 to denom2 (decrease group1 weight): should succeed
    cp.swap_exact_amount_out(
        MsgSwapExactAmountOut {
            sender: t.accounts["provider"].address(),
            routes: vec![SwapAmountOutRoute {
                pool_id: t.contract.pool_id,
                token_in_denom: "denom0".to_string(),
            }],
            token_out: Some(coin(1_000, "denom2").into()),
            token_in_max_amount: Uint128::from(10_000u128).to_string(),
        },
        &t.accounts["provider"],
    )
    .unwrap();
}
