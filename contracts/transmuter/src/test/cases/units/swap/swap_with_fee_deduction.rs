use cosmwasm_std::{coin, Decimal, Uint128};
use osmosis_std::types::{
    cosmos::bank::v1beta1::QueryBalanceRequest,
    osmosis::poolmanager::v1beta1::{
        MsgSwapExactAmountIn, MsgSwapExactAmountOut, SwapAmountInRoute, SwapAmountOutRoute,
    },
};
use osmosis_test_tube::{Account, Bank, Module, OsmosisTestApp};
use transmuter_math::rebalancing::config::RebalancingConfig;

use crate::test::{modules::cosmwasm_pool::CosmwasmPool, test_env::TestEnv};
use crate::{
    asset::AssetConfig,
    contract::sv::{ExecMsg, QueryMsg},
    contract::{GetIncentivePoolBalancesResponse, GetTotalPoolLiquidityResponse},
    scope::Scope,
    test::test_env::TestEnvBuilder,
};

const DENOM1_INITIAL: u128 = 100_000_000_000;
const DENOM2_INITIAL: u128 = 500_000_000_000;
const DENOM3_INITIAL: u128 = 5_000_000_000_000;

#[test]
fn test_swap_exact_amount_out_with_fee_deduction() {
    let app = OsmosisTestApp::new();
    let t = setup_test_env(&app);
    let cp = CosmwasmPool::new(&app);

    // Test the swap that should require fee
    // This swap makes denom1 weight go from 50% to 60%, triggering fee
    let token_out = coin(200_000_000_000u128, "denom2");
    let amount_in_before_fee = Uint128::from(20_000_000_000u128);
    let expected_fee = Uint128::from(1_000_000_000u128) + Uint128::from(3_000_000_000u128); // group1 + denom1 fees
    let token_in_amount = amount_in_before_fee + expected_fee;

    // Test with insufficient max amount (should fail)
    let insufficient_max = token_in_amount - Uint128::from(1u128);
    let err = cp
        .swap_exact_amount_out(
            MsgSwapExactAmountOut {
                sender: t.accounts["swapper"].address(),
                routes: vec![SwapAmountOutRoute {
                    pool_id: t.contract.pool_id,
                    token_in_denom: "denom1".to_string(),
                }],
                token_out: Some(token_out.clone().into()),
                token_in_max_amount: insufficient_max.to_string(),
            },
            &t.accounts["swapper"],
        )
        .unwrap_err();

    // Should fail due to excessive token in required
    assert!(err.to_string().contains("Excessive token in required"));

    // Test with sufficient max amount (should succeed)
    cp.swap_exact_amount_out(
        MsgSwapExactAmountOut {
            sender: t.accounts["swapper"].address(),
            routes: vec![SwapAmountOutRoute {
                pool_id: t.contract.pool_id,
                token_in_denom: "denom1".to_string(),
            }],
            token_out: Some(token_out.into()),
            token_in_max_amount: token_in_amount.to_string(),
        },
        &t.accounts["swapper"],
    )
    .unwrap();

    // Verify contract balances include pool liquidity + collected fees
    verify_contract_balances(&t, expected_fee, "denom1");
}

#[test]
fn test_swap_exact_amount_in_with_fee_deduction() {
    let app = OsmosisTestApp::new();
    let cp = CosmwasmPool::new(&app);

    let t = setup_test_env(&app);

    // Test the swap that should require fee deduction from output
    let token_in = coin(20_000_000_000u128, "denom1");
    let amount_out_before_fee = Uint128::from(200_000_000_000u128);
    let expected_fee = Uint128::from(10_000_000_000u128) + Uint128::from(30_000_000_000u128); // group1 + denom1 fees
    let token_out_amount = amount_out_before_fee - expected_fee;

    // Test with min amount too high (should fail)
    let excessive_min = token_out_amount + Uint128::from(1u128);
    let err = cp
        .swap_exact_amount_in(
            MsgSwapExactAmountIn {
                sender: t.accounts["swapper"].address(),
                routes: vec![SwapAmountInRoute {
                    pool_id: t.contract.pool_id,
                    token_out_denom: "denom2".to_string(),
                }],
                token_in: Some(token_in.clone().into()),
                token_out_min_amount: excessive_min.to_string(),
            },
            &t.accounts["swapper"],
        )
        .unwrap_err();

    // Should fail due to insufficient token out
    assert!(err.to_string().contains("Insufficient token out"));

    // Test with appropriate min amount (should succeed)
    cp.swap_exact_amount_in(
        MsgSwapExactAmountIn {
            sender: t.accounts["swapper"].address(),
            routes: vec![SwapAmountInRoute {
                pool_id: t.contract.pool_id,
                token_out_denom: "denom2".to_string(),
            }],
            token_in: Some(token_in.into()),
            token_out_min_amount: token_out_amount.to_string(),
        },
        &t.accounts["swapper"],
    )
    .unwrap();

    // Verify contract balances include pool liquidity + collected fees
    verify_contract_balances(&t, expected_fee, "denom2");
}

fn setup_test_env<'a>(app: &'a OsmosisTestApp) -> TestEnv<'a> {
    let admin = app.init_account(&[coin(100_000u128, "uosmo")]).unwrap();

    let t = TestEnvBuilder::new()
        .with_account("admin", vec![])
        .with_account("swapper", vec![coin(1_000_000_000_000, "denom1")])
        .with_account(
            "provider",
            vec![
                coin(DENOM1_INITIAL, "denom1"),
                coin(DENOM2_INITIAL, "denom2"),
                coin(DENOM3_INITIAL, "denom3"),
            ],
        )
        .with_instantiate_msg(crate::contract::sv::InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig {
                    denom: "denom1".to_string(),
                    normalization_factor: Uint128::one(),
                },
                AssetConfig {
                    denom: "denom2".to_string(),
                    normalization_factor: Uint128::new(10),
                },
                AssetConfig {
                    denom: "denom3".to_string(),
                    normalization_factor: Uint128::new(100),
                },
            ],
            alloyed_asset_subdenom: "usd".to_string(),
            alloyed_asset_normalization_factor: Uint128::new(100),
            admin: Some(admin.address()),
            moderator: "osmo1cyyzpxplxdzkeea7kwsydadg87357qnahakaks".to_string(),
        })
        .build(&app);

    // Add initial liquidity to the pool
    t.contract
        .execute(
            &ExecMsg::JoinPool {},
            &[
                coin(DENOM1_INITIAL, "denom1"),
                coin(DENOM2_INITIAL, "denom2"),
                coin(DENOM3_INITIAL, "denom3"),
            ],
            &t.accounts["provider"],
        )
        .unwrap();

    // Create asset group for denom2 and denom3
    t.contract
        .execute(
            &ExecMsg::CreateAssetGroup {
                label: "group1".to_string(),
                denoms: vec!["denom2".to_string(), "denom3".to_string()],
            },
            &[],
            &t.accounts["admin"],
        )
        .unwrap();

    // Add rebalancing config for denom1 (same as unit test)
    t.contract
        .execute(
            &ExecMsg::AddRebalancingConfig {
                scope: Scope::denom("denom1"),
                rebalancing_config: RebalancingConfig::new(
                    Decimal::percent(50),
                    Decimal::percent(45),
                    Decimal::percent(55),
                    Decimal::percent(30),
                    Decimal::percent(65),
                    Decimal::percent(10),
                    Decimal::percent(20),
                )
                .unwrap(),
            },
            &[],
            &t.accounts["admin"],
        )
        .unwrap();

    t.contract
        .execute(
            &ExecMsg::AddRebalancingConfig {
                scope: Scope::asset_group("group1"),
                rebalancing_config: RebalancingConfig::new(
                    Decimal::percent(55),
                    Decimal::percent(45),
                    Decimal::percent(60),
                    Decimal::percent(30),
                    Decimal::percent(65),
                    Decimal::percent(10),
                    Decimal::percent(20),
                )
                .unwrap(),
            },
            &[],
            &t.accounts["admin"],
        )
        .unwrap();

    t
}

/// Helper function to verify that contract balances equal pool liquidity plus collected fees
fn verify_contract_balances(
    t: &crate::test::test_env::TestEnv,
    expected_fee_amount: Uint128,
    fee_denom: &str,
) {
    let bank = Bank::new(t.app);

    // Query the pool liquidity from the contract
    let pool_liquidity: GetTotalPoolLiquidityResponse = t
        .contract
        .query(&QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    // Query the incentive pool balances from the contract
    let incentive_pool_balances: GetIncentivePoolBalancesResponse = t
        .contract
        .query(&QueryMsg::GetIncentivePoolBalances {})
        .unwrap();

    // Verify the incentive pool balances
    for balance in incentive_pool_balances.balances {
        if balance.denom == fee_denom {
            assert_eq!(balance.amount, expected_fee_amount);
        } else {
            assert_eq!(balance.amount, Uint128::zero());
        }
    }

    // For each denom in the pool, verify contract balance
    for pool_coin in &pool_liquidity.total_pool_liquidity {
        let contract_balance = bank
            .query_balance(&QueryBalanceRequest {
                address: t.contract.contract_addr.to_string(),
                denom: pool_coin.denom.clone(),
            })
            .unwrap()
            .balance
            .unwrap();

        let expected_balance = if pool_coin.denom == fee_denom {
            // For the fee denom, expect pool amount + collected fees
            pool_coin.amount + expected_fee_amount
        } else {
            // For other denoms, expect just the pool amount
            pool_coin.amount
        };

        assert_eq!(
            contract_balance.amount.parse::<u128>().unwrap(),
            expected_balance.u128(),
            "Contract balance mismatch for {}: expected {}, got {}",
            pool_coin.denom,
            expected_balance,
            contract_balance.amount
        );
    }
}
