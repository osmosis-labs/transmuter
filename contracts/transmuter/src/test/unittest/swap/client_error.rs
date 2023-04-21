use super::*;

const REMAINING_DENOM0: u128 = 1_000_000_000_000;
const REMAINING_DENOM1: u128 = 1_000_000_000_000;

fn pool_with_assets() -> OwnedDeps<MockStorage, MockApi, MockQuerier, Empty> {
    pool_with_single_lp(&[
        Coin::new(REMAINING_DENOM0, "denom0"),
        Coin::new(REMAINING_DENOM1, "denom1"),
    ])
}

test_swap! {
    swap_with_mismatch_funds_and_token_in_should_fail [expect error] {
        setup = pool_with_assets(),
        msgs = [
            ExecMsg::SwapExactAmountIn {
                token_in: Coin::new(1_000_000, "denom0"),
                token_out_denom: "denom1".to_string(),
                token_out_min_amount: Uint128::zero(),
            },
            ExecMsg::SwapExactAmountOut {
                token_in_denom: "denom0".to_string(),
                token_in_max_amount: 1_000_000u128.into(),
                token_out: Coin::new(1_000_000, "denom1"),
            },
        ],
        funds = [Coin::new(999_999, "denom0")],
        err = ContractError::FundsMismatchTokenIn {
            funds: vec![Coin::new(999_999, "denom0")],
            token_in: Coin::new(1_000_000, "denom0")
        }
    }
}

test_swap! {
    swap_exact_amount_in_with_token_out_less_than_token_out_min_amount_should_fail [expect error] {
        setup = pool_with_assets(),
        msg = ExecMsg::SwapExactAmountIn {
            token_in: Coin::new(1_000_000, "denom0"),
            token_out_denom: "denom1".to_string(),
            token_out_min_amount: 1_000_001u128.into(),
        },
        funds = [Coin::new(1_000_000, "denom0")],
        err = ContractError::InsufficientTokenOut {
            required: 1_000_001u128.into(),
            available: 1_000_000u128.into()
        }
    }
}

test_swap! {
    swap_exact_aomunt_out_with_exceeding_token_in_max_should_fail [expect error] {
        setup = pool_with_assets(),
        msg = ExecMsg::SwapExactAmountOut {
            token_in_denom: "denom0".to_string(),
            token_in_max_amount: 999_999u128.into(),
            token_out: Coin::new(1_000_000, "denom1"),
        },
        funds = [Coin::new(1_000_000, "denom0")],
        err = ContractError::ExcessiveRequiredTokenIn {
            limit: 999_999u128.into(),
            required: 1_000_000u128.into()
        }
    }
}

test_swap! {
    swap_with_unsupported_denom_for_token_out_should_fail [expect error] {
        setup = pool_with_assets(),
        msgs = [
            ExecMsg::SwapExactAmountIn {
                token_in: Coin::new(1_000_000, "denom0"),
                token_out_denom: "INVALID_DENOM".to_string(),
                token_out_min_amount: Uint128::zero(),
            },
            ExecMsg::SwapExactAmountOut {
                token_in_denom: "denom0".to_string(),
                token_in_max_amount: 1_000_000u128.into(),
                token_out: Coin::new(1_000_000, "INVALID_DENOM"),
            },
        ],
        funds = [Coin::new(1_000_000, "denom0")],
        err = ContractError::InvalidTransmuteDenom {
            denom: "INVALID_DENOM".to_string(),
            expected_denom: vec!["denom0".to_string(), "denom1".to_string()]
        }
    }
}

test_swap! {
    swap_with_unsupported_denom_for_token_in_should_fail [expect error] {
        setup = pool_with_assets(),
        msgs = [
            ExecMsg::SwapExactAmountIn {
                token_in: Coin::new(1_000_000, "INVALID_DENOM"),
                token_out_denom: "denom1".to_string(),
                token_out_min_amount: Uint128::zero(),
            },
            ExecMsg::SwapExactAmountOut {
                token_in_denom: "INVALID_DENOM".to_string(),
                token_in_max_amount: 1_000_000u128.into(),
                token_out: Coin::new(1_000_000, "denom1"),
            },
        ],
        funds = [Coin::new(1_000_000, "INVALID_DENOM")],
        err = ContractError::InvalidTransmuteDenom {
            denom: "INVALID_DENOM".to_string(),
            expected_denom: vec!["denom0".to_string(), "denom1".to_string()]
        }
    }
}
