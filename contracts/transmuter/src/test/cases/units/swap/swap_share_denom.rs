use cosmwasm_std::{Coin, Uint128};

use crate::contract::{GetShareDenomResponse, QueryMsg};

use super::{pool_with_single_lp, test_swap_share_denom_success_case, SwapMsg};
const REMAINING_DENOM0: u128 = 1_000_000_000_000;
const REMAINING_DENOM1: u128 = 1_000_000_000_000;

#[test]
fn test_swap_exect_amount_in_with_share_denom() {
    let app = osmosis_test_tube::OsmosisTestApp::new();
    let t = pool_with_single_lp(
        &app,
        vec![
            Coin::new(REMAINING_DENOM0, "denom0"),
            Coin::new(REMAINING_DENOM1, "denom1"),
        ],
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
        Coin::new(1_000, share_denom.clone()),
    );

    test_swap_share_denom_success_case(
        &t,
        SwapMsg::SwapExactAmountIn {
            token_in: Coin::new(1_000, share_denom),
            token_out_denom: "denom1".to_string(),
            token_out_min_amount: Uint128::one(),
        },
        Coin::new(1_000, "denom1".to_string()),
    );
}
