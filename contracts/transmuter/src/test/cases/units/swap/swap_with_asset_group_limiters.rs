use cosmwasm_std::{coin, Decimal, Uint128};

use crate::{
    asset::AssetConfig, contract::sv::ExecMsg, limiter::LimiterParams, scope::Scope, ContractError,
};

use super::{pool_with_single_lp, test_swap_failed_case, test_swap_success_case, SwapMsg};

const REMAINING_DENOM0: u128 = 1_000_000_000_000;
const REMAINING_DENOM1: u128 = 1_000_000_000_000;
const REMAINING_DENOM2: u128 = 1_000_000_000_000;

#[test]
fn test_swap_with_asset_group_limiters() {
    let app = osmosis_test_tube::OsmosisTestApp::new();
    let t = pool_with_single_lp(
        &app,
        vec![
            coin(REMAINING_DENOM0, "denom0"),
            coin(REMAINING_DENOM1, "denom1"),
            coin(REMAINING_DENOM2, "denom2"),
        ],
        vec![
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
    );

    // Add asset group and limiters
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
            &ExecMsg::RegisterLimiter {
                scope: Scope::asset_group("group1"),
                label: "limiter1".to_string(),
                limiter_params: LimiterParams::StaticLimiter {
                    upper_limit: Decimal::percent(10),
                },
            },
            &[],
            &t.accounts["admin"],
        )
        .unwrap();

    // swap within group, even an agressive one wouldn't effect anything
    test_swap_success_case(
        &t,
        SwapMsg::SwapExactAmountOut {
            token_in_denom: "denom0".to_string(),
            token_in_max_amount: Uint128::from(REMAINING_DENOM1),
            token_out: coin(REMAINING_DENOM1, "denom1".to_string()),
        },
        coin(REMAINING_DENOM1, "denom1".to_string()),
    );

    app.increase_time(5);

    // swap denom0 to denom2 -> increase group1 weight by adding more denom0
    test_swap_failed_case(
        &t,
        SwapMsg::SwapExactAmountOut {
            token_in_denom: "denom0".to_string(),
            token_in_max_amount: Uint128::from(REMAINING_DENOM0),
            token_out: coin(REMAINING_DENOM0, "denom2".to_string()),
        },
        ContractError::UpperLimitExceeded {
            scope: Scope::asset_group("group1"),
            upper_limit: "0.766666666666666666".parse().unwrap(),
            value: Decimal::percent(100),
        },
    );
}
