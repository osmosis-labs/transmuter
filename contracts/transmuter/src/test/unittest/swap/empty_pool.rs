use super::*;

fn empty_pool() -> OwnedDeps<MockStorage, MockApi, MockQuerier, Empty> {
    pool_with_single_lp(&[Coin::new(0, "denom0"), Coin::new(0, "denom1")])
}

test_swap! {
    swap_with_0denom0_should_succeed [expect ok] {
        setup = empty_pool(),
        msgs = [
            ExecMsg::SwapExactAmountIn {
                token_in: Coin::new(0, "denom0"),
                token_out_denom: "denom1".to_string(),
                token_out_min_amount: Uint128::zero(),
            },
            ExecMsg::SwapExactAmountOut {
                token_in_denom: "denom0".to_string(),
                token_in_max_amount: Uint128::zero(),
                token_out: Coin::new(0, "denom1"),
            },
        ],
        funds = [Coin::new(0, "denom0")],
        received = Coin::new(0, "denom1")
    }
}

test_swap! {
    swap_with_0denom1_token_in_should_succeed [expect ok] {
        setup = empty_pool(),
        msgs = [
            ExecMsg::SwapExactAmountIn {
                token_in: Coin::new(0, "denom1"),
                token_out_denom: "denom0".to_string(),
                token_out_min_amount: Uint128::zero(),
            },
            ExecMsg::SwapExactAmountOut {
                token_in_denom: "denom1".to_string(),
                token_in_max_amount: Uint128::zero(),
                token_out: Coin::new(0, "denom0"),
            },
        ],
        funds = [Coin::new(0, "denom1")],
        received = Coin::new(0, "denom0")
    }
}

test_swap! {
    swap_1denom0_token_in_should_fail [expect error] {
        setup = empty_pool(),
        msgs = [
            ExecMsg::SwapExactAmountIn {
                token_in: Coin::new(1, "denom0"),
                token_out_denom: "denom1".to_string(),
                token_out_min_amount: Uint128::zero(),
            },
            ExecMsg::SwapExactAmountOut {
                token_in_denom: "denom0".to_string(),
                token_in_max_amount: Uint128::zero(),
                token_out: Coin::new(1, "denom1"),
            },
        ],
        funds = [Coin::new(1, "denom0")],
        err = ContractError::InsufficientPoolAsset {
            available: Coin::new(0, "denom1"),
            required: Coin::new(1, "denom1"),
        }
    }
}

test_swap! {
    swap_with_1denom1_token_in_should_fail [expect error] {
        setup = empty_pool(),
        msgs = [
            ExecMsg::SwapExactAmountIn {
                token_in: Coin::new(1, "denom1"),
                token_out_denom: "denom0".to_string(),
                token_out_min_amount: Uint128::zero(),
            },
            ExecMsg::SwapExactAmountOut {
                token_in_denom: "denom1".to_string(),
                token_in_max_amount: Uint128::zero(),
                token_out: Coin::new(1, "denom0"),
            },
        ],
        funds = [Coin::new(1, "denom1")],
        err = ContractError::InsufficientPoolAsset {
            available: Coin::new(0, "denom0"),
            required: Coin::new(1, "denom0"),
        }
    }
}
