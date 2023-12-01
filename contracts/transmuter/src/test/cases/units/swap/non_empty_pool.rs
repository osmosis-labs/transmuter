use super::*;

const REMAINING_DENOM0: u128 = 1_000_000_000_000;
const REMAINING_DENOM1: u128 = 1_000_000_000_000;

fn non_empty_pool(app: &'_ OsmosisTestApp) -> TestEnv<'_> {
    pool_with_single_lp(
        app,
        vec![
            Coin::new(REMAINING_DENOM0, "denom0"),
            Coin::new(REMAINING_DENOM1, "denom1"),
        ],
        vec![],
    )
}

test_swap! {
    swap_with_1denom0_should_succeed [expect ok] {
        setup = non_empty_pool,
        msgs = [
            SwapMsg::SwapExactAmountIn {
                token_in: Coin::new(1, "denom0"),
                token_out_denom: "denom1".to_string(),
                token_out_min_amount: Uint128::one(),
            },
            SwapMsg::SwapExactAmountOut {
                token_in_denom: "denom0".to_string(),
                token_in_max_amount: Uint128::one(),
                token_out: Coin::new(1, "denom1"),
            },
        ],
        received = Coin::new(1, "denom1")
    }
}

test_swap! {
    swap_with_1denom1_should_succeed [expect ok] {
        setup = non_empty_pool,
        msgs = [
            SwapMsg::SwapExactAmountIn {
                token_in: Coin::new(1, "denom1"),
                token_out_denom: "denom0".to_string(),
                token_out_min_amount: Uint128::one(),
            },
            SwapMsg::SwapExactAmountOut {
                token_in_denom: "denom1".to_string(),
                token_in_max_amount: Uint128::one(),
                token_out: Coin::new(1, "denom0"),
            },
        ],
        received = Coin::new(1, "denom0")
    }
}

test_swap! {
    swap_all_denom0_should_succeed [expect ok] {
        setup = non_empty_pool,
        msgs = [
            SwapMsg::SwapExactAmountIn {
                token_in: Coin::new(REMAINING_DENOM0, "denom0"),
                token_out_denom: "denom1".to_string(),
                token_out_min_amount: REMAINING_DENOM0.into(),
            },
            SwapMsg::SwapExactAmountOut {
                token_in_denom: "denom0".to_string(),
                token_in_max_amount: REMAINING_DENOM0.into(),
                token_out: Coin::new(REMAINING_DENOM0, "denom1"),
            },
        ],
        received = Coin::new(REMAINING_DENOM0, "denom1")
    }
}

test_swap! {
    swap_all_denom1_should_succeed [expect ok] {
        setup = non_empty_pool,
        msgs = [
            SwapMsg::SwapExactAmountIn {
                token_in: Coin::new(REMAINING_DENOM1, "denom1"),
                token_out_denom: "denom0".to_string(),
                token_out_min_amount: REMAINING_DENOM1.into(),
            },
            SwapMsg::SwapExactAmountOut {
                token_in_denom: "denom1".to_string(),
                token_in_max_amount: REMAINING_DENOM1.into(),
                token_out: Coin::new(REMAINING_DENOM1, "denom0"),
            },
        ],
        received = Coin::new(REMAINING_DENOM1, "denom0")
    }
}

test_swap! {
    swap_arbritary_amount_of_denom1_should_succeed [expect ok] {
        setup = non_empty_pool,
        msgs = [
            SwapMsg::SwapExactAmountIn {
                token_in: Coin::new(999_999, "denom1"),
                token_out_denom: "denom0".to_string(),
                token_out_min_amount: 999_999u128.into(),
            },
            SwapMsg::SwapExactAmountOut {
                token_in_denom: "denom1".to_string(),
                token_in_max_amount: 999_999u128.into(),
                token_out: Coin::new(999_999, "denom0"),
            },
        ],
        received = Coin::new(999_999, "denom0")
    }
}

test_swap! {
    swap_arbritary_amount_of_denom0_should_succeed [expect ok] {
        setup = non_empty_pool,
        msgs = [
            SwapMsg::SwapExactAmountIn {
                token_in: Coin::new(999_999, "denom0"),
                token_out_denom: "denom1".to_string(),
                token_out_min_amount: 999_999u128.into(),
            },
            SwapMsg::SwapExactAmountOut {
                token_in_denom: "denom0".to_string(),
                token_in_max_amount: 999_999u128.into(),
                token_out: Coin::new(999_999, "denom1"),
            },
        ],
        received = Coin::new(999_999, "denom1")
    }
}
