use cosmwasm_std::{Coin, Uint128};

use crate::{asset::AssetConfig, contract::sv::QueryMsg, contract::GetShareDenomResponse};

use super::{pool_with_single_lp, test_swap_share_denom_success_case, SwapMsg};
const REMAINING_DENOM0: u128 = 1_000_000_000_000;
const REMAINING_DENOM1: u128 = 1_000_000_000_000;

#[test]
fn test_swap_exact_amount_in_with_share_denom() {
    let app = osmosis_test_tube::OsmosisTestApp::new();
    let t = pool_with_single_lp(
        &app,
        vec![
            Coin::new(REMAINING_DENOM0, "denom0"),
            Coin::new(REMAINING_DENOM1, "denom1"),
        ],
        vec![],
    );

    // get share denom
    let share_denom = t
        .contract
        .query::<GetShareDenomResponse>(&QueryMsg::GetShareDenom {})
        .unwrap()
        .share_denom;

    test_swap_share_denom_success_case(
        &t,
        SwapMsg::SwapExactAmountIn {
            token_in: Coin::new(1_000, "denom0".to_string()),
            token_out_denom: share_denom.clone(),
            token_out_min_amount: Uint128::one(),
        },
        Coin::new(1_000, "denom0".to_string()),
        Coin::new(1_000, share_denom.clone()),
    );

    test_swap_share_denom_success_case(
        &t,
        SwapMsg::SwapExactAmountIn {
            token_in: Coin::new(1_000, share_denom.clone()),
            token_out_denom: "denom1".to_string(),
            token_out_min_amount: Uint128::one(),
        },
        Coin::new(1_000, share_denom),
        Coin::new(1_000, "denom1".to_string()),
    );
}

#[test]
fn test_swap_exact_amount_in_with_share_denom_and_normalization_factor() {
    let app = osmosis_test_tube::OsmosisTestApp::new();
    let t = pool_with_single_lp(
        &app,
        vec![
            Coin::new(REMAINING_DENOM0, "denom0"),
            Coin::new(REMAINING_DENOM1 * 10u128.pow(8), "denom1"),
        ],
        vec![AssetConfig {
            denom: "denom1".to_string(),
            normalization_factor: 10u128.pow(8).into(),
        }],
    );

    // get share denom
    let share_denom = t
        .contract
        .query::<GetShareDenomResponse>(&QueryMsg::GetShareDenom {})
        .unwrap()
        .share_denom;

    test_swap_share_denom_success_case(
        &t,
        SwapMsg::SwapExactAmountIn {
            token_in: Coin::new(1_000 * 10u128.pow(8), "denom1".to_string()),
            token_out_denom: share_denom.clone(),
            token_out_min_amount: Uint128::one(),
        },
        Coin::new(1_000 * 10u128.pow(8), "denom1".to_string()),
        Coin::new(1_000, share_denom.clone()),
    );

    test_swap_share_denom_success_case(
        &t,
        SwapMsg::SwapExactAmountIn {
            token_in: Coin::new(1000, share_denom.clone()),
            token_out_denom: "denom1".to_string(),
            token_out_min_amount: Uint128::one(),
        },
        Coin::new(1000, share_denom),
        Coin::new(1000 * 10u128.pow(8), "denom1".to_string()),
    );
}

#[test]
fn test_swap_exact_amount_out_with_share_denom() {
    let app = osmosis_test_tube::OsmosisTestApp::new();
    let t = pool_with_single_lp(
        &app,
        vec![
            Coin::new(REMAINING_DENOM0, "denom0"),
            Coin::new(REMAINING_DENOM1, "denom1"),
        ],
        vec![],
    );

    // get share denom
    let share_denom = t
        .contract
        .query::<GetShareDenomResponse>(&QueryMsg::GetShareDenom {})
        .unwrap()
        .share_denom;

    test_swap_share_denom_success_case(
        &t,
        SwapMsg::SwapExactAmountOut {
            token_in_denom: "denom0".to_string(),
            token_in_max_amount: Uint128::from(1_000u128),
            token_out: Coin::new(1_000, share_denom.clone()),
        },
        Coin::new(1_000, "denom0".to_string()),
        Coin::new(1_000, share_denom.clone()),
    );

    test_swap_share_denom_success_case(
        &t,
        SwapMsg::SwapExactAmountOut {
            token_in_denom: share_denom.clone(),
            token_in_max_amount: Uint128::from(1_000u128),
            token_out: Coin::new(1_000, "denom1".to_string()),
        },
        Coin::new(1_000, share_denom),
        Coin::new(1_000, "denom1".to_string()),
    );
}

#[test]
fn test_swap_exact_amount_out_with_share_denom_single_asset_pool() {
    let app = osmosis_test_tube::OsmosisTestApp::new();
    let t = pool_with_single_lp(&app, vec![Coin::new(REMAINING_DENOM0, "denom0")], vec![]);

    // get share denom
    let share_denom = t
        .contract
        .query::<GetShareDenomResponse>(&QueryMsg::GetShareDenom {})
        .unwrap()
        .share_denom;

    test_swap_share_denom_success_case(
        &t,
        SwapMsg::SwapExactAmountOut {
            token_in_denom: "denom0".to_string(),
            token_in_max_amount: Uint128::from(1_000u128),
            token_out: Coin::new(1_000, share_denom.clone()),
        },
        Coin::new(1_000, "denom0".to_string()),
        Coin::new(1_000, share_denom.clone()),
    );

    test_swap_share_denom_success_case(
        &t,
        SwapMsg::SwapExactAmountOut {
            token_in_denom: share_denom.clone(),
            token_in_max_amount: Uint128::from(1_000u128),
            token_out: Coin::new(1_000, "denom0".to_string()),
        },
        Coin::new(1_000, share_denom.clone()),
        Coin::new(1_000, "denom0".to_string()),
    );
}

#[test]
fn test_swap_exact_amount_in_with_share_denom_and_normalization_factor_single_asset() {
    let app = osmosis_test_tube::OsmosisTestApp::new();
    let t = pool_with_single_lp(
        &app,
        vec![Coin::new(REMAINING_DENOM1 * 10u128.pow(8), "denom1")],
        vec![AssetConfig {
            denom: "denom1".to_string(),
            normalization_factor: 10u128.pow(8).into(),
        }],
    );

    // get share denom
    let share_denom = t
        .contract
        .query::<GetShareDenomResponse>(&QueryMsg::GetShareDenom {})
        .unwrap()
        .share_denom;

    test_swap_share_denom_success_case(
        &t,
        SwapMsg::SwapExactAmountIn {
            token_in: Coin::new(1_000 * 10u128.pow(8), "denom1".to_string()),
            token_out_denom: share_denom.clone(),
            token_out_min_amount: Uint128::one(),
        },
        Coin::new(1_000 * 10u128.pow(8), "denom1".to_string()),
        Coin::new(1_000, share_denom.clone()),
    );

    test_swap_share_denom_success_case(
        &t,
        SwapMsg::SwapExactAmountIn {
            token_in: Coin::new(1000, share_denom.clone()),
            token_out_denom: "denom1".to_string(),
            token_out_min_amount: Uint128::one(),
        },
        Coin::new(1000, share_denom),
        Coin::new(1000 * 10u128.pow(8), "denom1".to_string()),
    );
}

#[test]
fn test_swap_exact_amount_out_with_share_denom_where_token_in_max_is_exceeding_expectation() {
    let app = osmosis_test_tube::OsmosisTestApp::new();
    let t = pool_with_single_lp(
        &app,
        vec![
            Coin::new(REMAINING_DENOM0, "denom0"),
            Coin::new(REMAINING_DENOM1, "denom1"),
        ],
        vec![],
    );

    // get share denom
    let share_denom = t
        .contract
        .query::<GetShareDenomResponse>(&QueryMsg::GetShareDenom {})
        .unwrap()
        .share_denom;

    // swap 1001 denom0 for 1001 share_denom
    test_swap_share_denom_success_case(
        &t,
        SwapMsg::SwapExactAmountOut {
            token_in_denom: "denom0".to_string(),
            token_in_max_amount: Uint128::from(1_001u128),
            token_out: Coin::new(1_001, share_denom.clone()),
        },
        Coin::new(1_001, "denom0".to_string()),
        Coin::new(1_001, share_denom.clone()),
    );

    // swap 1000 share_denom for 1000 denom1 but set token_in_max_amount to 1001 (1 extra share_denom)
    test_swap_share_denom_success_case(
        &t,
        SwapMsg::SwapExactAmountOut {
            token_in_denom: share_denom.clone(),
            token_in_max_amount: Uint128::from(1_001u128),
            token_out: Coin::new(1_000, "denom1".to_string()),
        },
        Coin::new(1_000, share_denom),
        Coin::new(1_000, "denom1".to_string()),
    );
}
