use cosmwasm_std::coin;

use super::*;

const REMAINING_DENOM0: u128 = 1_000_000_000_000_000_000;
const REMAINING_DENOM1: u128 = 1_000_000_000_000_000_000;

fn non_empty_pool(app: &'_ OsmosisTestApp) -> TestEnv<'_> {
    pool_with_single_lp(
        app,
        vec![
            coin(REMAINING_DENOM0, "denom0"),
            coin(REMAINING_DENOM1, "denom1"),
        ],
        vec![],
    )
}

test_swap! {
    swap_with_1denom0_should_succeed [expect ok] {
        setup = non_empty_pool,
        msgs = [
            SwapMsg::SwapExactAmountIn {
                token_in: coin(1, "denom0"),
                token_out_denom: "denom1".to_string(),
                token_out_min_amount: Uint128::one(),
            },
            SwapMsg::SwapExactAmountOut {
                token_in_denom: "denom0".to_string(),
                token_in_max_amount: Uint128::one(),
                token_out: coin(1, "denom1"),
            },
        ],
        received = coin(1, "denom1")
    }
}

test_swap! {
    swap_with_1denom1_should_succeed [expect ok] {
        setup = non_empty_pool,
        msgs = [
            SwapMsg::SwapExactAmountIn {
                token_in: coin(1, "denom1"),
                token_out_denom: "denom0".to_string(),
                token_out_min_amount: Uint128::one(),
            },
            SwapMsg::SwapExactAmountOut {
                token_in_denom: "denom1".to_string(),
                token_in_max_amount: Uint128::one(),
                token_out: coin(1, "denom0"),
            },
        ],
        received = coin(1, "denom0")
    }
}

test_swap! {
    swap_all_denom0_should_succeed [expect ok] {
        setup = non_empty_pool,
        msgs = [
            SwapMsg::SwapExactAmountIn {
                token_in: coin(REMAINING_DENOM0, "denom0"),
                token_out_denom: "denom1".to_string(),
                token_out_min_amount: REMAINING_DENOM0.into(),
            },
            SwapMsg::SwapExactAmountOut {
                token_in_denom: "denom0".to_string(),
                token_in_max_amount: REMAINING_DENOM0.into(),
                token_out: coin(REMAINING_DENOM0, "denom1"),
            },
        ],
        received = coin(REMAINING_DENOM0, "denom1")
    }
}

test_swap! {
    swap_all_denom1_should_succeed [expect ok] {
        setup = non_empty_pool,
        msgs = [
            SwapMsg::SwapExactAmountIn {
                token_in: coin(REMAINING_DENOM1, "denom1"),
                token_out_denom: "denom0".to_string(),
                token_out_min_amount: REMAINING_DENOM1.into(),
            },
            SwapMsg::SwapExactAmountOut {
                token_in_denom: "denom1".to_string(),
                token_in_max_amount: REMAINING_DENOM1.into(),
                token_out: coin(REMAINING_DENOM1, "denom0"),
            },
        ],
        received = coin(REMAINING_DENOM1, "denom0")
    }
}

test_swap! {
    swap_arbritary_amount_of_denom1_should_succeed [expect ok] {
        setup = non_empty_pool,
        msgs = [
            SwapMsg::SwapExactAmountIn {
                token_in: coin(999_999, "denom1"),
                token_out_denom: "denom0".to_string(),
                token_out_min_amount: 999_999u128.into(),
            },
            SwapMsg::SwapExactAmountOut {
                token_in_denom: "denom1".to_string(),
                token_in_max_amount: 999_999u128.into(),
                token_out: coin(999_999, "denom0"),
            },
        ],
        received = coin(999_999, "denom0")
    }
}

test_swap! {
    swap_arbritary_amount_of_denom0_should_succeed [expect ok] {
        setup = non_empty_pool,
        msgs = [
            SwapMsg::SwapExactAmountIn {
                token_in: coin(999_999, "denom0"),
                token_out_denom: "denom1".to_string(),
                token_out_min_amount: 999_999u128.into(),
            },
            SwapMsg::SwapExactAmountOut {
                token_in_denom: "denom0".to_string(),
                token_in_max_amount: 999_999u128.into(),
                token_out: coin(999_999, "denom1"),
            },
        ],
        received = coin(999_999, "denom1")
    }
}

fn non_empty_pool_with_normalization_factor(app: &'_ OsmosisTestApp) -> TestEnv<'_> {
    pool_with_single_lp(
        app,
        vec![
            coin(REMAINING_DENOM0, "denom0"),
            coin(REMAINING_DENOM1, "denom1"),
        ],
        vec![
            AssetConfig {
                denom: "denom0".to_string(),
                normalization_factor: (3u128 * 10u128.pow(16)).into(),
            },
            AssetConfig {
                denom: "denom1".to_string(),
                normalization_factor: (10u128.pow(14)).into(),
            },
        ],
    )
}

test_swap! {
    swap_with_1denom1_with_normalization_factor_should_succeed [expect ok] {
        setup = non_empty_pool_with_normalization_factor,
        msgs = [
            SwapMsg::SwapExactAmountIn {
                token_in: coin(3u128 * 10u128.pow(2), "denom0"),
                token_out_denom: "denom1".to_string(),
                token_out_min_amount: Uint128::one(),
            },
            SwapMsg::SwapExactAmountOut {
                token_in_denom: "denom0".to_string(),
                token_in_max_amount: (300u128).into(),
                token_out: coin(1, "denom1"),
            },
        ],
        received = coin(1, "denom1")
    }
}

test_swap! {
    swap_exact_in_with_1000denom0_with_normalization_factor_should_succeed_with_round_down_token_out [expect ok] {
        setup = non_empty_pool_with_normalization_factor,
        msgs = [
            SwapMsg::SwapExactAmountIn {
                token_in: coin(1000, "denom0"),
                token_out_denom: "denom1".to_string(),
                token_out_min_amount: Uint128::from(3u128),
            },

        ],
        received = coin(3, "denom1")
    }
}

test_swap! {
    swap_exact_out_3denom1_with_normalization_factor_should_succeed_with_round_up_token_in [expect ok] {
        setup = non_empty_pool_with_normalization_factor,
        msgs = [
            SwapMsg::SwapExactAmountOut {
                token_in_denom: "denom0".to_string(),
                token_in_max_amount: (900u128).into(),
                token_out: coin(3, "denom1"),
            },
        ],
        received = coin(3, "denom1")
    }
}
