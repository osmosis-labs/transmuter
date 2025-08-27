use cosmwasm_std::{coin, Coin, Coins, Decimal, Uint128};
use osmosis_std::types::{
    cosmos::bank::v1beta1::{MsgSend, QueryAllBalancesRequest},
    osmosis::poolmanager::v1beta1::{
        MsgSwapExactAmountIn, MsgSwapExactAmountOut, SwapAmountInRoute, SwapAmountOutRoute,
    },
};
use osmosis_test_tube::{Account, Bank, Module, OsmosisTestApp};
use transmuter_math::rebalancing::config::RebalancingConfig;

use crate::{
    asset::AssetConfig,
    contract::{
        sv::{ExecMsg, QueryMsg},
        GetIncentivePoolBalancesResponse, GetShareDenomResponse, GetTotalPoolLiquidityResponse,
    },
    scope::Scope,
    test::test_env::TestEnvBuilder,
};
use crate::{
    contract::GetTotalSharesResponse,
    test::{modules::cosmwasm_pool::CosmwasmPool, test_env::TestEnv},
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
    let expected_fee = Uint128::from(4_000_000_000u128); // group1 + denom1 fees

    let fee_token = coin(expected_fee.u128(), "denom1");
    let token_in_without_fee = coin(amount_in_before_fee.u128(), "denom1");
    let token_in_with_fee = coin((amount_in_before_fee + expected_fee).u128(), "denom1");

    let swapper_address = t.accounts["swapper"].address().to_string();

    let mut swapper_balances = get_balances(&t, swapper_address.clone());
    let mut pool_liquidity = get_pool_liquidity(&t);
    let mut incentive_pool_balances = get_incentive_pool_balances(&t);

    // Test with insufficient max amount (should fail)
    let insufficient_max = token_in_with_fee.amount - Uint128::from(1u128);
    let err = cp
        .swap_exact_amount_out(
            MsgSwapExactAmountOut {
                sender: t.accounts["swapper"].address(),
                routes: vec![SwapAmountOutRoute {
                    pool_id: t.contract.pool_id,
                    token_in_denom: token_in_with_fee.denom.to_string(),
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
    let result = cp
        .swap_exact_amount_out(
            MsgSwapExactAmountOut {
                sender: t.accounts["swapper"].address(),
                routes: vec![SwapAmountOutRoute {
                    pool_id: t.contract.pool_id,
                    token_in_denom: "denom1".to_string(),
                }],
                token_out: Some(token_out.clone().into()),
                token_in_max_amount: token_in_with_fee.amount.to_string(),
            },
            &t.accounts["swapper"],
        )
        .unwrap();

    // assert swapper balances changes
    swapper_balances.sub(get_tx_fee(&result)).unwrap();
    swapper_balances.sub(token_in_with_fee).unwrap();
    swapper_balances.add(token_out.clone()).unwrap();
    assert_eq!(swapper_balances, get_balances(&t, swapper_address.clone()));

    // assert pool liquidity changes
    pool_liquidity.add(token_in_without_fee).unwrap();
    pool_liquidity.sub(token_out.clone()).unwrap();
    assert_eq!(pool_liquidity, get_pool_liquidity(&t));

    // assert incentive pool changes
    incentive_pool_balances.add(fee_token).unwrap();
    assert_eq!(incentive_pool_balances, get_incentive_pool_balances(&t));

    assert_accounting_invariant(&t);
}

#[test]
fn test_swap_exact_amount_in_incentive() {
    // ----- setup an unbalanced pool state with incentive pool filled -----
    let app = OsmosisTestApp::new();
    let t = setup_test_env(&app);
    let cp = CosmwasmPool::new(&app);

    let token_in = coin(20_000_000_000u128, "denom1");
    let amount_out_before_fee = Uint128::from(200_000_000_000u128);
    let expected_fee = Uint128::from(40_000_000_000u128); // group1 + denom1 fees
    let token_out_amount = amount_out_before_fee - expected_fee;

    let fee_token = coin(expected_fee.u128(), "denom2");

    let swapper_address = t.accounts["swapper"].address().to_string();

    // Setup: Use exact amount in to create unbalanced pool and fill incentive pool
    cp.swap_exact_amount_in(
        MsgSwapExactAmountIn {
            sender: t.accounts["swapper"].address(),
            routes: vec![SwapAmountInRoute {
                pool_id: t.contract.pool_id,
                token_out_denom: "denom2".to_string(),
            }],
            token_in: Some(token_in.clone().into()),
            token_out_min_amount: token_out_amount.to_string(),
        },
        &t.accounts["swapper"],
    )
    .unwrap();

    assert_eq!(get_incentive_pool_balances(&t), Coins::from(fee_token));
    assert_accounting_invariant(&t);
    // ----- end setup -----

    let mut swapper_balances = get_balances(&t, swapper_address.clone());
    let mut pool_liquidity = get_pool_liquidity(&t);
    let mut incentive_pool_balances = get_incentive_pool_balances(&t);

    let token_out_without_incentive = token_in;
    let token_in = coin(amount_out_before_fee.u128(), "denom2");

    let incentive_token = coin(
        4_000_000_000, // 40_000_000_000u128, // - 4_000_000_000u128,
        token_out_without_incentive.denom.clone(),
    );
    let token_out_with_incentive = coin(
        (token_out_without_incentive.amount + incentive_token.amount).u128(),
        token_out_without_incentive.denom.clone(),
    );

    let result = cp
        .swap_exact_amount_in(
            MsgSwapExactAmountIn {
                sender: t.accounts["swapper"].address(),
                routes: vec![SwapAmountInRoute {
                    pool_id: t.contract.pool_id,
                    token_out_denom: token_out_with_incentive.denom.clone(),
                }],
                token_in: Some(token_in.clone().into()),
                token_out_min_amount: token_out_with_incentive.amount.to_string(),
            },
            &t.accounts["swapper"],
        )
        .unwrap();

    // assert swapper balances changes
    swapper_balances.sub(get_tx_fee(&result)).unwrap();
    swapper_balances.sub(token_in.clone()).unwrap();
    swapper_balances
        .add(token_out_with_incentive.clone())
        .unwrap();
    assert_eq!(swapper_balances, get_balances(&t, swapper_address.clone()));

    // assert pool liquidity changes
    pool_liquidity.add(token_in.clone()).unwrap();
    pool_liquidity
        .sub(token_out_without_incentive.clone())
        .unwrap();
    // internal swap
    let internal_token_in = coin(40_000_000_000, "denom2");
    let internal_token_out = coin(4_000_000_000, "denom1");
    pool_liquidity.add(internal_token_in.clone()).unwrap();
    pool_liquidity.sub(internal_token_out.clone()).unwrap();
    assert_eq!(pool_liquidity, get_pool_liquidity(&t));

    // assert incentive pool changes
    incentive_pool_balances.sub(internal_token_in).unwrap();
    assert_eq!(incentive_pool_balances, get_incentive_pool_balances(&t));

    assert_accounting_invariant(&t);
}

#[test]
fn test_swap_exact_amount_in_with_fee_deduction() {
    let app = OsmosisTestApp::new();
    let t = setup_test_env(&app);
    let cp = CosmwasmPool::new(&app);

    // Test the swap that should require fee deduction from output
    // This swap makes denom1 weight go from 50% to 60%, triggering fee
    let token_in = coin(20_000_000_000u128, "denom1");
    let amount_out_before_fee = Uint128::from(200_000_000_000u128);
    let expected_fee = Uint128::from(40_000_000_000u128); // group1 + denom1 fees
    let token_out_amount = amount_out_before_fee - expected_fee;

    let fee_token = coin(expected_fee.u128(), "denom2");
    let token_out_before_fee = coin(amount_out_before_fee.u128(), "denom2");
    let token_out_after_fee = coin(token_out_amount.u128(), "denom2");

    let swapper_address = t.accounts["swapper"].address().to_string();

    let mut swapper_balances = get_balances(&t, swapper_address.clone());
    let mut pool_liquidity = get_pool_liquidity(&t);
    let mut incentive_pool_balances = get_incentive_pool_balances(&t);

    // Test with min amount too high (should fail)
    let excessive_min = token_out_amount + Uint128::from(1u128);
    let err = cp
        .swap_exact_amount_in(
            MsgSwapExactAmountIn {
                sender: t.accounts["swapper"].address(),
                routes: vec![SwapAmountInRoute {
                    pool_id: t.contract.pool_id,
                    token_out_denom: token_out_before_fee.denom.to_string(),
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
    let result = cp
        .swap_exact_amount_in(
            MsgSwapExactAmountIn {
                sender: t.accounts["swapper"].address(),
                routes: vec![SwapAmountInRoute {
                    pool_id: t.contract.pool_id,
                    token_out_denom: "denom2".to_string(),
                }],
                token_in: Some(token_in.clone().into()),
                token_out_min_amount: token_out_amount.to_string(),
            },
            &t.accounts["swapper"],
        )
        .unwrap();

    // assert swapper balances changes
    swapper_balances.sub(get_tx_fee(&result)).unwrap();
    swapper_balances.sub(token_in.clone()).unwrap();
    swapper_balances.add(token_out_after_fee.clone()).unwrap();
    assert_eq!(swapper_balances, get_balances(&t, swapper_address.clone()));

    // assert pool liquidity changes
    pool_liquidity.add(token_in).unwrap();
    pool_liquidity.sub(token_out_before_fee).unwrap();
    assert_eq!(pool_liquidity, get_pool_liquidity(&t));

    // assert incentive pool changes
    incentive_pool_balances.add(fee_token).unwrap();
    assert_eq!(incentive_pool_balances, get_incentive_pool_balances(&t));

    assert_accounting_invariant(&t);
}

#[test]
fn test_swap_exact_amount_out_incentive() {
    // ----- setup an unbalanced pool state with incentive pool filled -----
    let app = OsmosisTestApp::new();
    let t = setup_test_env(&app);
    let cp = CosmwasmPool::new(&app);

    let token_out = coin(200_000_000_000u128, "denom2");
    let amount_in_before_fee = Uint128::from(20_000_000_000u128);
    let expected_fee = Uint128::from(4_000_000_000u128);

    let fee_token = coin(expected_fee.u128(), "denom1");
    let token_in_without_fee = coin(amount_in_before_fee.u128(), "denom1");
    let token_in_with_fee = coin((amount_in_before_fee + expected_fee).u128(), "denom1");

    let swapper_address = t.accounts["swapper"].address().to_string();

    cp.swap_exact_amount_out(
        MsgSwapExactAmountOut {
            sender: t.accounts["swapper"].address(),
            routes: vec![SwapAmountOutRoute {
                pool_id: t.contract.pool_id,
                token_in_denom: "denom1".to_string(),
            }],
            token_out: Some(token_out.clone().into()),
            token_in_max_amount: token_in_with_fee.amount.to_string(),
        },
        &t.accounts["swapper"],
    )
    .unwrap();

    assert_eq!(get_incentive_pool_balances(&t), Coins::from(fee_token));
    assert_accounting_invariant(&t);
    // ----- end setup -----

    // swap the same amount with exact out
    let token_in_before_incentive_rebate = token_out;
    // this is reduced from what is collected in the first swap, excess fee coming from unhealhy incentive pool healing.
    let incentive_token = coin(
        40_000_000_000u128 - 4_000_000_000u128,
        token_in_before_incentive_rebate.denom.clone(),
    );
    let token_in_after_incentive_rebate = coin(
        (token_in_before_incentive_rebate.amount - incentive_token.amount).u128(),
        token_in_before_incentive_rebate.denom.clone(),
    );

    let token_out = token_in_without_fee;

    let mut swapper_balances = get_balances(&t, swapper_address.clone());
    let mut pool_liquidity = get_pool_liquidity(&t);
    let mut incentive_pool_balances = get_incentive_pool_balances(&t);

    let result = cp
        .swap_exact_amount_out(
            MsgSwapExactAmountOut {
                sender: t.accounts["swapper"].address(),
                routes: vec![SwapAmountOutRoute {
                    pool_id: t.contract.pool_id,
                    token_in_denom: token_in_after_incentive_rebate.denom.to_string(),
                }],
                token_out: Some(token_out.clone().into()),
                token_in_max_amount: token_in_after_incentive_rebate.amount.to_string(),
            },
            &t.accounts["swapper"],
        )
        .unwrap();

    // assert swapper balances changes
    swapper_balances.sub(get_tx_fee(&result)).unwrap();
    swapper_balances
        .sub(token_in_after_incentive_rebate.clone())
        .unwrap();
    swapper_balances.add(token_out.clone()).unwrap();
    assert_eq!(swapper_balances, get_balances(&t, swapper_address.clone()));

    // assert pool liquidity changes

    // internal incentive swap
    let internal_token_in = coin(4_000_000_000, "denom1");
    let internal_token_out = coin(40_000_000_000, "denom2");
    pool_liquidity.add(internal_token_in.clone()).unwrap();
    pool_liquidity.sub(internal_token_out.clone()).unwrap();

    pool_liquidity
        .add(token_in_before_incentive_rebate)
        .unwrap();
    pool_liquidity.sub(token_out.clone()).unwrap();
    assert_eq!(pool_liquidity, get_pool_liquidity(&t));

    // assert incentive pool changes
    incentive_pool_balances.sub(internal_token_in).unwrap();
    incentive_pool_balances.add(internal_token_out).unwrap();
    incentive_pool_balances.sub(incentive_token).unwrap();
    assert_eq!(incentive_pool_balances, get_incentive_pool_balances(&t));

    assert_accounting_invariant(&t);
}

#[test]
fn test_swap_exact_amount_in_incentive_with_internal_alloyed_swap() {
    let app = OsmosisTestApp::new();
    let t = setup_test_env(&app);
    let cp = CosmwasmPool::new(&app);

    // ---- setup with only alloyed in the incentive pool ---
    t.contract
        .execute(
            &ExecMsg::JoinPool {},
            &[
                coin(10_000_000_000u128, "denom1"),
                coin(200_000_000_000u128, "denom2"),
                coin(2_000_000_000_000u128, "denom3"),
            ],
            &t.accounts["swapper"],
        )
        .unwrap();

    assert_accounting_invariant(&t);
    // ---- end setup ----

    let mut swapper_balances = get_balances(&t, t.accounts["swapper"].address().to_string());
    let mut pool_liquidity = get_pool_liquidity(&t);
    let mut incentive_pool_balances = get_incentive_pool_balances(&t);

    let alloyed_denom = t
        .contract
        .query::<GetShareDenomResponse>(&QueryMsg::GetShareDenom {})
        .unwrap()
        .share_denom;

    // Execute swap exact amount in: 10,000,000,000 denom1 in, 100,000,000,000 denom2 out with 5,000,000,000 incentive to denom2
    let token_in = coin(10_000_000_000u128, "denom1");
    let token_out_without_incentive = coin(100_000_000_000u128, "denom2");
    let incentive_amount = 5_000_000_000u128;
    let token_out_with_incentive = coin(
        token_out_without_incentive.amount.u128() + incentive_amount,
        token_out_without_incentive.denom.clone(),
    );

    let result = cp
        .swap_exact_amount_in(
            MsgSwapExactAmountIn {
                sender: t.accounts["swapper"].address(),
                routes: vec![SwapAmountInRoute {
                    pool_id: t.contract.pool_id,
                    token_out_denom: token_out_with_incentive.denom.clone(),
                }],
                token_in: Some(token_in.clone().into()),
                token_out_min_amount: token_out_with_incentive.amount.to_string(),
            },
            &t.accounts["swapper"],
        )
        .unwrap();

    // assert swapper balances changes
    swapper_balances.sub(get_tx_fee(&result)).unwrap();
    swapper_balances.sub(token_in.clone()).unwrap();
    swapper_balances
        .add(token_out_with_incentive.clone())
        .unwrap();
    assert_eq!(
        swapper_balances,
        get_balances(&t, t.accounts["swapper"].address().to_string())
    );

    // assert pool liquidity changes
    pool_liquidity.add(token_in.clone()).unwrap();
    pool_liquidity
        .sub(token_out_without_incentive.clone())
        .unwrap();

    // Internal swap: convert alloyed asset to denom2 for incentive
    let internal_alloyed_in = coin(incentive_amount * 10, &alloyed_denom);
    let internal_token_out = coin(incentive_amount, "denom2");

    pool_liquidity.sub(internal_token_out.clone()).unwrap();
    assert_eq!(pool_liquidity, get_pool_liquidity(&t));

    // assert incentive pool changes
    incentive_pool_balances.sub(internal_alloyed_in).unwrap();
    assert_eq!(incentive_pool_balances, get_incentive_pool_balances(&t));

    assert_accounting_invariant(&t);
}

#[test]
fn test_swap_exact_amount_out_incentive_with_internal_alloyed_swap() {
    let app = OsmosisTestApp::new();
    let t = setup_test_env(&app);
    let cp = CosmwasmPool::new(&app);

    // ---- setup with only alloyed in the incentive pool ---
    t.contract
        .execute(
            &ExecMsg::JoinPool {},
            &[
                coin(10_000_000_000u128, "denom1"),
                coin(200_000_000_000u128, "denom2"),
                coin(2_000_000_000_000u128, "denom3"),
            ],
            &t.accounts["swapper"],
        )
        .unwrap();

    assert_accounting_invariant(&t);
    // ---- end setup ----

    let mut swapper_balances = get_balances(&t, t.accounts["swapper"].address().to_string());
    let mut pool_liquidity = get_pool_liquidity(&t);
    let mut incentive_pool_balances = get_incentive_pool_balances(&t);

    let alloyed_denom = t
        .contract
        .query::<GetShareDenomResponse>(&QueryMsg::GetShareDenom {})
        .unwrap()
        .share_denom;

    // Execute swap exact amount out: 100,000,000,000 denom2 out, with 5,000,000,000 incentive deducted from denom1 in
    let token_out = coin(100_000_000_000u128, "denom2");
    let token_in_without_incentive = coin(10_000_000_000u128, "denom1");
    let incentive_amount = 500_000_000u128;
    let token_in_with_incentive_deduction = coin(
        token_in_without_incentive.amount.u128() - incentive_amount,
        token_in_without_incentive.denom.clone(),
    );

    let result = cp
        .swap_exact_amount_out(
            MsgSwapExactAmountOut {
                sender: t.accounts["swapper"].address(),
                routes: vec![SwapAmountOutRoute {
                    pool_id: t.contract.pool_id,
                    token_in_denom: token_in_with_incentive_deduction.denom.clone(),
                }],
                token_out: Some(token_out.clone().into()),
                token_in_max_amount: token_in_with_incentive_deduction.amount.to_string(),
            },
            &t.accounts["swapper"],
        )
        .unwrap();

    // assert swapper balances changes
    swapper_balances.sub(get_tx_fee(&result)).unwrap();
    swapper_balances
        .sub(token_in_with_incentive_deduction.clone())
        .unwrap();
    swapper_balances.add(token_out.clone()).unwrap();
    assert_eq!(
        swapper_balances,
        get_balances(&t, t.accounts["swapper"].address().to_string())
    );

    // assert pool liquidity changes
    pool_liquidity
        .add(token_in_without_incentive.clone())
        .unwrap();
    pool_liquidity.sub(token_out.clone()).unwrap();

    // Internal swap: convert alloyed asset to denom1 for incentive
    let internal_alloyed_in = coin(incentive_amount * 100, &alloyed_denom);
    let internal_token_out = coin(incentive_amount, "denom1");

    pool_liquidity.sub(internal_token_out.clone()).unwrap();
    assert_eq!(pool_liquidity, get_pool_liquidity(&t));

    // assert incentive pool changes
    incentive_pool_balances.sub(internal_alloyed_in).unwrap();
    assert_eq!(incentive_pool_balances, get_incentive_pool_balances(&t));

    assert_accounting_invariant(&t);
}

#[test]
fn test_swap_tokens_to_alloyed_asset_exact_in_with_fee_deduction() {
    let app = OsmosisTestApp::new();
    let t = setup_test_env(&app);

    let initial_total_alloyed_asset_supply = get_alloyed_supply(&t);
    let alloy_denom = initial_total_alloyed_asset_supply.denom;

    let mut swapper_balances = get_balances(&t, t.accounts["swapper"].address().to_string());
    let mut pool_liquidity = get_pool_liquidity(&t);
    let mut incentive_pool_balances = get_incentive_pool_balances(&t);

    // Multiple tokens in that will make denom1 55%, denom2 25%
    let tokens_in = vec![
        coin(37_500_000_000u128, "denom1"), // 3_750_000_000_000 normalized
        coin(125_000_000_000u128, "denom2"), // 1_250_000_000_000 normalized
    ];

    // fee(denom1) = 25_000_000_000_000u128 * (5% * 1%) = 125_000_000_000u128
    // fee(group1) = 25_000_000_000_000u128 * 0% = 0
    let amount_out_before_fee = Uint128::from(3_750_000_000_000u128 + 1_250_000_000_000u128);
    let fee = Uint128::from(125_000_000_000u128);
    let token_out_amount = amount_out_before_fee - fee;

    // Try with correct min out (should succeed)
    let result = t
        .contract
        .execute(&ExecMsg::JoinPool {}, &tokens_in, &t.accounts["swapper"])
        .unwrap();

    // assert swapper balances changes
    swapper_balances.sub(get_tx_fee(&result)).unwrap();
    for token_in in tokens_in.iter() {
        swapper_balances.sub(token_in.clone()).unwrap();
    }
    swapper_balances
        .add(coin(token_out_amount.u128(), &alloy_denom))
        .unwrap();
    assert_eq!(
        swapper_balances,
        get_balances(&t, t.accounts["swapper"].address().to_string())
    );

    // assert pool liquidity changes
    for token_in in tokens_in.iter() {
        pool_liquidity.add(token_in.clone()).unwrap();
    }
    assert_eq!(pool_liquidity, get_pool_liquidity(&t));

    // assert incentive pool changes
    incentive_pool_balances
        .add(coin(fee.u128(), alloy_denom))
        .unwrap();
    assert_eq!(incentive_pool_balances, get_incentive_pool_balances(&t));

    assert_accounting_invariant(&t);

    // Check for total alloyed asset total supply
    let alloyed_asset_supply = get_alloyed_supply(&t);

    assert_eq!(
        alloyed_asset_supply.amount,
        initial_total_alloyed_asset_supply.amount + amount_out_before_fee
    );
}

#[test]
fn test_swap_tokens_to_alloyed_asset_exact_in_incentive() {
    // ---- setup incentive pool with some denom other than alloyed ---

    let app = OsmosisTestApp::new();
    let t = setup_test_env(&app);
    let cp = CosmwasmPool::new(&app);

    let alloyed_denom = t
        .contract
        .query::<GetShareDenomResponse>(&QueryMsg::GetShareDenom {})
        .unwrap()
        .share_denom;

    let swapper_address = t.accounts["swapper"].address().to_string();

    let token_in = coin(10_000_000_000u128 + 1_000_000_000u128, "denom1");
    let token_out_amount = 100_000_000_000u128;

    cp.swap_exact_amount_out(
        MsgSwapExactAmountOut {
            sender: t.accounts["swapper"].address(),
            routes: vec![SwapAmountOutRoute {
                pool_id: t.contract.pool_id,
                token_in_denom: "denom1".to_string(),
            }],
            token_out: Some(coin(token_out_amount, "denom2").into()),
            token_in_max_amount: token_in.amount.to_string(),
        },
        &t.accounts["swapper"],
    )
    .unwrap();

    assert_accounting_invariant(&t);

    // ----- end setup -----

    let mut swapper_balances = get_balances(&t, swapper_address.clone());
    let mut pool_liquidity = get_pool_liquidity(&t);
    let mut incentive_pool_balances = get_incentive_pool_balances(&t);

    let token_in = coin(200_000_000_000, "denom2");

    let amount_out = 2_000_000_000_000u128;
    let incentive = 95_000_000_000u128;
    let token_out = coin(amount_out + incentive, alloyed_denom.clone());

    let result = cp
        .swap_exact_amount_in(
            MsgSwapExactAmountIn {
                sender: t.accounts["swapper"].address(),
                routes: vec![SwapAmountInRoute {
                    pool_id: t.contract.pool_id,
                    token_out_denom: alloyed_denom.clone(),
                }],
                token_in: Some(token_in.clone().into()),
                token_out_min_amount: token_out.amount.to_string(),
            },
            &t.accounts["swapper"],
        )
        .unwrap();

    swapper_balances.sub(get_tx_fee(&result)).unwrap();
    swapper_balances.sub(token_in.clone()).unwrap();
    swapper_balances.add(token_out.clone()).unwrap();
    assert_eq!(swapper_balances, get_balances(&t, swapper_address.clone()));

    pool_liquidity.add(token_in.clone()).unwrap();
    pool_liquidity.add(coin(1_000_000_000, "denom1")).unwrap(); // internal swap denom1 -> alloyed
    assert_eq!(pool_liquidity, get_pool_liquidity(&t));

    incentive_pool_balances
        .sub(coin(1_000_000_000, "denom1"))
        .unwrap();
    incentive_pool_balances
        .add(coin(5_000_000_000, alloyed_denom))
        .unwrap();
    assert_eq!(incentive_pool_balances, get_incentive_pool_balances(&t));

    assert_accounting_invariant(&t);
}

#[test]
fn test_swap_tokens_to_alloyed_asset_exact_out_with_fee_deduction() {
    let app = OsmosisTestApp::new();
    let t = setup_test_env(&app);
    let cp = CosmwasmPool::new(&app);

    let initial_total_alloyed_asset_supply = get_alloyed_supply(&t);
    let share_denom: String = initial_total_alloyed_asset_supply.denom;

    let mut swapper_balances = get_balances(&t, t.accounts["swapper"].address().to_string());
    let mut pool_liquidity = get_pool_liquidity(&t);
    let mut incentive_pool_balances = get_incentive_pool_balances(&t);

    // fee(denom1) = 25_000_000_000_000u128 * ((5% * 10%) + (5% * 20%)) = 375_000_000_000
    // fee(group1) = 25_000_000_000_000u128 * (5% * 10%) = 125_000_000_000
    // = 500_000_000_000u128
    let token_out_amount = Uint128::from(5_000_000_000_000u128);
    let amount_in_before_fee = Uint128::from(50_000_000_000u128); // 5_000_000_000_000 / 100
    let fee = Uint128::from(5_000_000_000u128); // 500_000_000_000u / 100
    let token_in_amount = amount_in_before_fee + fee;

    // Try with max in too low (should fail)
    let err = cp
        .swap_exact_amount_out(
            MsgSwapExactAmountOut {
                sender: t.accounts["swapper"].address(),
                routes: vec![SwapAmountOutRoute {
                    pool_id: t.contract.pool_id,
                    token_in_denom: "denom1".to_string(),
                }],
                token_out: Some(coin(token_out_amount.u128(), &share_denom).into()),
                token_in_max_amount: (token_in_amount - Uint128::from(1u128)).to_string(),
            },
            &t.accounts["swapper"],
        )
        .unwrap_err();
    assert!(err.to_string().contains("Excessive token in required"));

    // Try with correct max in (should succeed)
    let result = cp
        .swap_exact_amount_out(
            MsgSwapExactAmountOut {
                sender: t.accounts["swapper"].address(),
                routes: vec![SwapAmountOutRoute {
                    pool_id: t.contract.pool_id,
                    token_in_denom: "denom1".to_string(),
                }],
                token_out: Some(coin(token_out_amount.u128(), &share_denom).into()),
                token_in_max_amount: token_in_amount.to_string(),
            },
            &t.accounts["swapper"],
        )
        .unwrap();

    // assert swapper balances changes
    swapper_balances.sub(get_tx_fee(&result)).unwrap();
    swapper_balances
        .sub(coin(token_in_amount.u128(), "denom1"))
        .unwrap();
    swapper_balances
        .add(coin(token_out_amount.u128(), &share_denom))
        .unwrap();
    assert_eq!(
        swapper_balances,
        get_balances(&t, t.accounts["swapper"].address().to_string())
    );

    // assert pool liquidity changes
    pool_liquidity
        .add(coin(amount_in_before_fee.u128(), "denom1"))
        .unwrap();
    assert_eq!(pool_liquidity, get_pool_liquidity(&t));

    // The contract should have minted the correct fee and output
    let fee_token = coin(fee.u128(), "denom1");
    incentive_pool_balances.add(fee_token).unwrap();
    assert_eq!(incentive_pool_balances, get_incentive_pool_balances(&t));

    // Check for total alloyed asset total supply
    let alloyed_asset_supply = get_alloyed_supply(&t);

    // The total supply should have increased by the output amount
    assert_eq!(
        alloyed_asset_supply.amount,
        initial_total_alloyed_asset_supply.amount + token_out_amount
    );
}

#[test]
fn test_swap_tokens_to_alloyed_asset_exact_out_incentive() {
    let app = OsmosisTestApp::new();
    let t = setup_test_env(&app);
    let cp = CosmwasmPool::new(&app);

    // ---- setup with only alloyed in the incentive pool ---
    t.contract
        .execute(
            &ExecMsg::JoinPool {},
            &[
                coin(10_000_000_000u128, "denom1"),
                coin(200_000_000_000u128, "denom2"),
                coin(2_000_000_000_000u128, "denom3"),
            ],
            &t.accounts["swapper"],
        )
        .unwrap();

    assert_accounting_invariant(&t);
    // ---- end setup ----
    let mut swapper_balances = get_balances(&t, t.accounts["swapper"].address().to_string());
    let mut pool_liquidity = get_pool_liquidity(&t);
    let mut incentive_pool_balances = get_incentive_pool_balances(&t);

    let alloyed_denom = t
        .contract
        .query::<GetShareDenomResponse>(&QueryMsg::GetShareDenom {})
        .unwrap()
        .share_denom;

    // Execute swap_exact_amount_out with 1,000,000,000,000 alloyed out and denom1 as token in
    let token_out = coin(1_000_000_000_000u128, &alloyed_denom);
    let amount_in_before_incentive_rebate = 10_000_000_000u128;
    let incentive = 500_000_000u128;
    let amount_in = amount_in_before_incentive_rebate - incentive;
    let max_token_in = coin(amount_in, "denom1"); // Set a reasonable max

    let result = cp
        .swap_exact_amount_out(
            MsgSwapExactAmountOut {
                sender: t.accounts["swapper"].address(),
                routes: vec![SwapAmountOutRoute {
                    pool_id: t.contract.pool_id,
                    token_in_denom: "denom1".to_string(),
                }],
                token_out: Some(token_out.clone().into()),
                token_in_max_amount: max_token_in.amount.to_string(),
            },
            &t.accounts["swapper"],
        )
        .unwrap();

    swapper_balances.sub(get_tx_fee(&result)).unwrap();
    swapper_balances.sub(coin(amount_in, "denom1")).unwrap();
    swapper_balances.add(token_out.clone()).unwrap();
    assert_eq!(
        swapper_balances,
        get_balances(&t, t.accounts["swapper"].address().to_string())
    );

    pool_liquidity.add(coin(amount_in, "denom1")).unwrap();
    assert_eq!(pool_liquidity, get_pool_liquidity(&t));

    incentive_pool_balances
        .sub(coin(50_000_000_000, alloyed_denom))
        .unwrap();
    assert_eq!(incentive_pool_balances, get_incentive_pool_balances(&t));

    assert_accounting_invariant(&t);
}

#[test]
fn test_swap_alloyed_asset_to_tokens_exact_in_with_fee_deduction() {
    let app = OsmosisTestApp::new();
    let t = setup_test_env(&app);
    let cp = CosmwasmPool::new(&app);
    let bank = Bank::new(&app);

    let initial_total_alloyed_asset_supply = get_alloyed_supply(&t);
    let share_denom: String = initial_total_alloyed_asset_supply.denom;

    // 2_000_000_000_000 alloyed in -> denom1
    let token_in_amount = Uint128::from(2_000_000_000_000u128);
    let token_out_amount_before_fee = Uint128::from(20_000_000_000u128);
    let fee = Uint128::from(222_222_223u128);
    let token_out_amount = token_out_amount_before_fee - fee;

    // send share_denom from provider to swapper with token_in_amount
    bank.send(
        MsgSend {
            from_address: t.accounts["provider"].address().to_string(),
            to_address: t.accounts["swapper"].address().to_string(),
            amount: vec![coin(token_in_amount.u128(), &share_denom).into()],
        },
        &t.accounts["provider"],
    )
    .unwrap();

    let mut swapper_balances = get_balances(&t, t.accounts["swapper"].address().to_string());
    let mut pool_liquidity = get_pool_liquidity(&t);
    let mut incentive_pool_balances = get_incentive_pool_balances(&t);

    // Try with min out too high (should fail)
    let err = cp
        .swap_exact_amount_in(
            MsgSwapExactAmountIn {
                sender: t.accounts["swapper"].address(),
                routes: vec![SwapAmountInRoute {
                    pool_id: t.contract.pool_id,
                    token_out_denom: "denom1".to_string(),
                }],
                token_in: Some(coin(token_in_amount.u128(), &share_denom).into()),
                token_out_min_amount: (token_out_amount + Uint128::from(1u128)).to_string(),
            },
            &t.accounts["swapper"],
        )
        .unwrap_err();

    assert!(err.to_string().contains("Insufficient token out"));

    // Try with correct min out (should succeed)
    let result = cp
        .swap_exact_amount_in(
            MsgSwapExactAmountIn {
                sender: t.accounts["swapper"].address(),
                routes: vec![SwapAmountInRoute {
                    pool_id: t.contract.pool_id,
                    token_out_denom: "denom1".to_string(),
                }],
                token_in: Some(coin(token_in_amount.u128(), &share_denom).into()),
                token_out_min_amount: token_out_amount.to_string(),
            },
            &t.accounts["swapper"],
        )
        .unwrap();

    // assert swapper balances changes
    swapper_balances.sub(get_tx_fee(&result)).unwrap();
    swapper_balances
        .sub(coin(token_in_amount.u128(), share_denom))
        .unwrap();
    swapper_balances
        .add(coin(token_out_amount.u128(), "denom1"))
        .unwrap();
    assert_eq!(
        swapper_balances,
        get_balances(&t, t.accounts["swapper"].address().to_string())
    );

    // assert pool liquidity changes
    pool_liquidity
        .sub(coin(token_out_amount_before_fee.u128(), "denom1"))
        .unwrap();
    assert_eq!(pool_liquidity, get_pool_liquidity(&t));

    // assert incentive pool changes
    incentive_pool_balances
        .add(coin(fee.u128(), "denom1"))
        .unwrap();
    assert_eq!(incentive_pool_balances, get_incentive_pool_balances(&t));

    assert_accounting_invariant(&t);

    let alloyed_asset_supply = get_alloyed_supply(&t);

    assert_eq!(
        alloyed_asset_supply.amount,
        initial_total_alloyed_asset_supply.amount - token_in_amount
    );
}

#[test]
fn test_swap_alloyed_asset_to_tokens_exact_in_incentive() {
    // ----- setup an unbalanced pool state with incentive pool filled -----
    let app = OsmosisTestApp::new();
    let t = setup_test_env(&app);
    let cp = CosmwasmPool::new(&app);

    let initial_total_alloyed_asset_supply = get_alloyed_supply(&t);
    let alloy_denom = initial_total_alloyed_asset_supply.denom;

    // Multiple tokens in that will make denom1 55%, denom2 25%
    let tokens_in = vec![
        coin(37_500_000_000u128, "denom1"), // 3_750_000_000_000 normalized
        coin(125_000_000_000u128, "denom2"), // 1_250_000_000_000 normalized
    ];

    // fee(denom1) = 25_000_000_000_000u128 * (5% * 1%) = 125_000_000_000u128
    // fee(group1) = 25_000_000_000_000u128 * 0% = 0
    t.contract
        .execute(&ExecMsg::JoinPool {}, &tokens_in, &t.accounts["swapper"])
        .unwrap();

    assert_accounting_invariant(&t);
    // ----- end setup -----

    let mut swapper_balances = get_balances(&t, t.accounts["swapper"].address().to_string());
    let mut pool_liquidity = get_pool_liquidity(&t);
    let mut incentive_pool_balances = get_incentive_pool_balances(&t);

    let amount_out = 48_750_000_000u128;

    // denom1 amount that actually got swapped from alloyed in the internal swap as first order incentive
    let incentive_before_correction = 799_689_440u128;
    let incentive = 709_926_485u128;

    let token_out = coin(amount_out + incentive, "denom1");
    let token_in = coin(4_875_000_000_000u128, &alloy_denom);

    let result = cp
        .swap_exact_amount_in(
            MsgSwapExactAmountIn {
                sender: t.accounts["swapper"].address(),
                routes: vec![SwapAmountInRoute {
                    pool_id: t.contract.pool_id,
                    token_out_denom: token_out.denom.clone(),
                }],
                token_in: Some(token_in.clone().into()),
                token_out_min_amount: (amount_out).to_string(),
            },
            &t.accounts["swapper"],
        )
        .unwrap();

    // assert swapper balances changes
    swapper_balances.sub(get_tx_fee(&result)).unwrap();
    swapper_balances.add(token_out.clone()).unwrap();
    swapper_balances.sub(token_in.clone()).unwrap();
    assert_eq!(
        swapper_balances,
        get_balances(&t, t.accounts["swapper"].address().to_string())
    );

    // assert pool liquidity changes
    pool_liquidity
        .sub(coin(amount_out + incentive_before_correction, "denom1"))
        .unwrap();
    assert_eq!(pool_liquidity, get_pool_liquidity(&t));

    // assert incentive pool changes
    incentive_pool_balances
        .add(coin(incentive_before_correction - incentive, "denom1"))
        .unwrap();

    incentive_pool_balances
        .sub(coin(incentive_before_correction * 100, alloy_denom))
        .unwrap();
    assert_eq!(incentive_pool_balances, get_incentive_pool_balances(&t));

    assert_accounting_invariant(&t);
}

#[test]
fn test_swap_alloyed_asset_to_tokens_exact_out_with_fee_deduction() {
    let app = OsmosisTestApp::new();
    let t = setup_test_env(&app);
    let bank = Bank::new(&app);

    let initial_total_alloyed_asset_supply = get_alloyed_supply(&t);
    let share_denom: String = initial_total_alloyed_asset_supply.denom;

    // Multiple tokens out that will make denom1 40%, group1 60%
    let tokens_out = vec![
        coin(80_000_000_000u128, "denom1"),
        coin(300_000_000_000u128, "denom2"),
        coin(4_000_000_000_000u128, "denom3"),
    ];

    // fee(denom1) = 20_000_000_000_000 * (5% * 1%) = 100_000_000_000
    // fee(group1) = 20_000_000_000_000 * (5% * 1%) = 100_000_000_000
    let amount_in_before_fee = Uint128::from(15_000_000_000_000u128);
    let fee = Uint128::from(200_000_000_000u128);
    let token_in_amount = amount_in_before_fee + fee;

    // send share_denom from provider to swapper with token_in_amount
    bank.send(
        MsgSend {
            from_address: t.accounts["provider"].address().to_string(),
            to_address: t.accounts["swapper"].address().to_string(),
            amount: vec![coin(token_in_amount.u128(), &share_denom).into()],
        },
        &t.accounts["provider"],
    )
    .unwrap();

    let mut swapper_balances = get_balances(&t, t.accounts["swapper"].address().to_string());
    let mut pool_liquidity = get_pool_liquidity(&t);
    let mut incentive_pool_balances = get_incentive_pool_balances(&t);

    let result = t
        .contract
        .execute(
            &ExecMsg::ExitPool {
                tokens_out: tokens_out.clone(),
            },
            &[],
            &t.accounts["swapper"],
        )
        .unwrap();

    // assert swapper balances changes
    swapper_balances.sub(get_tx_fee(&result)).unwrap();

    swapper_balances
        .sub(coin(token_in_amount.u128(), &share_denom))
        .unwrap();
    for coin_out in &tokens_out {
        swapper_balances.add(coin_out.clone()).unwrap();
    }
    assert_eq!(
        swapper_balances,
        get_balances(&t, t.accounts["swapper"].address().to_string())
    );

    // assert pool liquidity changes
    for coin_out in &tokens_out {
        pool_liquidity.sub(coin_out.clone()).unwrap();
    }
    assert_eq!(pool_liquidity, get_pool_liquidity(&t));

    // assert incentive pool changes
    incentive_pool_balances
        .add(coin(fee.u128(), &share_denom))
        .unwrap();
    assert_eq!(incentive_pool_balances, get_incentive_pool_balances(&t));

    assert_accounting_invariant(&t);

    let alloyed_asset_supply = get_alloyed_supply(&t);

    assert_eq!(
        alloyed_asset_supply.amount,
        initial_total_alloyed_asset_supply.amount - amount_in_before_fee
    );
}

#[test]
fn test_swap_alloyed_asset_to_tokens_exact_out_incentive() {
    // ----- setup an unbalanced pool state with incentive pool filled -----
    let app = OsmosisTestApp::new();
    let t = setup_test_env(&app);

    let initial_total_alloyed_asset_supply = get_alloyed_supply(&t);
    let alloy_denom = initial_total_alloyed_asset_supply.denom;

    // Multiple tokens in that will make denom1 55%, denom2 25%
    let tokens_in = vec![
        coin(37_500_000_000u128, "denom1"), // 3_750_000_000_000 normalized
        coin(125_000_000_000u128, "denom2"), // 1_250_000_000_000 normalized
    ];

    // fee(denom1) = 25_000_000_000_000u128 * (5% * 1%) = 125_000_000_000u128
    // fee(group1) = 25_000_000_000_000u128 * 0% = 0
    let amount_out_before_fee = Uint128::from(3_750_000_000_000u128 + 1_250_000_000_000u128);
    let fee = Uint128::from(125_000_000_000u128);
    let token_out_amount = amount_out_before_fee - fee;

    t.contract
        .execute(&ExecMsg::JoinPool {}, &tokens_in, &t.accounts["swapper"])
        .unwrap();

    assert_accounting_invariant(&t);
    // ----- end setup -----

    let mut swapper_balances = get_balances(&t, t.accounts["swapper"].address().to_string());
    let mut pool_liquidity = get_pool_liquidity(&t);
    let mut incentive_pool_balances = get_incentive_pool_balances(&t);

    let tokens_out = tokens_in;
    let token_in = coin(token_out_amount.u128(), &alloy_denom);

    let result = t
        .contract
        .execute(
            &ExecMsg::ExitPool {
                tokens_out: tokens_out.clone(),
            },
            &[],
            &t.accounts["swapper"],
        )
        .unwrap();

    // assert swapper balances changes
    swapper_balances.sub(get_tx_fee(&result)).unwrap();
    for token_out in tokens_out.iter() {
        swapper_balances.add(token_out.clone()).unwrap();
    }
    swapper_balances.sub(token_in.clone()).unwrap();
    assert_eq!(
        swapper_balances,
        get_balances(&t, t.accounts["swapper"].address().to_string())
    );

    // assert pool liquidity changes
    for token_out in tokens_out.iter() {
        pool_liquidity.sub(token_out.clone()).unwrap();
    }

    assert_eq!(pool_liquidity, get_pool_liquidity(&t));

    // assert incentive pool changes
    incentive_pool_balances
        .sub(coin(fee.u128(), alloy_denom))
        .unwrap();
    assert_eq!(incentive_pool_balances, get_incentive_pool_balances(&t));

    assert_accounting_invariant(&t);
}

fn setup_test_env<'a>(app: &'a OsmosisTestApp) -> TestEnv<'a> {
    let admin = app.init_account(&[coin(100_000u128, "uosmo")]).unwrap();

    let t = TestEnvBuilder::new()
        .with_account("admin", vec![])
        .with_account(
            "swapper",
            vec![
                coin(20_000_000_000_000, "denom1"),
                coin(200_000_000_000_000, "denom2"),
                coin(2_000_000_000_000_000, "denom3"),
            ],
        )
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

/// Asserts that the accounting invariant holds: the total contract balances
/// must equal the sum of pool liquidity and incentive pool balances.
///
/// This function verifies that no tokens are lost or gained unexpectedly
/// during swap operations by checking that:
/// - Pool liquidity + Incentive pool balances = Total contract balances
fn assert_accounting_invariant(t: &crate::test::test_env::TestEnv) {
    let pool_liquidity = get_pool_liquidity(t);
    let incentive_pool_balances = get_incentive_pool_balances(t);
    let expected_contract_balances = add_coins(pool_liquidity.clone(), incentive_pool_balances);

    let contract_balances = get_balances(t, t.contract.contract_addr.to_string());

    assert_eq!(expected_contract_balances, contract_balances);
}

fn get_balances(t: &crate::test::test_env::TestEnv, address: String) -> Coins {
    let bank = Bank::new(t.app);

    bank.query_all_balances(&QueryAllBalancesRequest {
        address,
        pagination: None,
        resolve_denom: false,
    })
    .unwrap()
    .balances
    .into_iter()
    .map(|c| coin(c.amount.parse::<u128>().unwrap(), c.denom))
    .collect::<Vec<_>>()
    .try_into()
    .unwrap()
}

fn get_pool_liquidity(t: &crate::test::test_env::TestEnv) -> Coins {
    t.contract
        .query::<GetTotalPoolLiquidityResponse>(&QueryMsg::GetTotalPoolLiquidity {})
        .unwrap()
        .total_pool_liquidity
        .into_iter()
        .collect::<Vec<_>>()
        .try_into()
        .unwrap()
}

fn get_incentive_pool_balances(t: &crate::test::test_env::TestEnv) -> Coins {
    t.contract
        .query::<GetIncentivePoolBalancesResponse>(&QueryMsg::GetIncentivePoolBalances {})
        .unwrap()
        .balances
        .into_iter()
        .collect::<Vec<_>>()
        .try_into()
        .unwrap()
}

fn add_coins(coins_a: Coins, coins_b: Coins) -> Coins {
    let mut coins = Coins::default();
    for coin in coins_a {
        coins.add(coin).unwrap();
    }
    for coin in coins_b {
        coins.add(coin).unwrap();
    }
    coins
}

/// Extracts the transaction fee from a transaction result
fn get_tx_fee<T>(result: &osmosis_test_tube::ExecuteResponse<T>) -> Coin
where
    T: osmosis_test_tube::cosmrs::proto::prost::Message + std::default::Default,
{
    result
        .events
        .iter()
        .find(|event| event.ty == "tx")
        .and_then(|event| {
            event
                .attributes
                .iter()
                .find(|attr| attr.key == "fee")
                .map(|attr| &attr.value)
        })
        .map(|fee_str| coin_from_str(fee_str))
        .expect("Fee event not found")
}

/// Parses a fee string in the format "123123uosmo" into a Coin
fn coin_from_str(fee_str: &str) -> Coin {
    // Find the first non-digit character to separate amount from denom
    let denom_start = fee_str.chars().position(|c| !c.is_ascii_digit()).unwrap();

    // Split into amount and denom
    let amount_str = &fee_str[..denom_start];
    let denom = &fee_str[denom_start..];

    // Parse the amount
    let amount: u128 = amount_str.parse().unwrap();

    coin(amount, denom)
}

fn get_alloyed_supply(t: &crate::test::test_env::TestEnv) -> Coin {
    let alloyed_asset_supply = t
        .contract
        .query::<GetTotalSharesResponse>(&QueryMsg::GetTotalShares {})
        .unwrap()
        .total_shares;

    // Query the share denom (alloyed asset denom)
    let share_denom: String = t
        .contract
        .query::<crate::contract::GetShareDenomResponse>(&QueryMsg::GetShareDenom {})
        .unwrap()
        .share_denom;

    coin(alloyed_asset_supply.u128(), share_denom)
}
