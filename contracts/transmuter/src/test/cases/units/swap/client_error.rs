use cosmwasm_std::coin;

use super::*;

const REMAINING_DENOM0: u128 = 1_000_000_000_000;
const REMAINING_DENOM1: u128 = 1_000_000_000_000;

fn pool_with_assets(app: &'_ OsmosisTestApp) -> TestEnv<'_> {
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
    swap_exact_amount_in_with_token_out_less_than_token_out_min_amount_should_fail [expect error] {
        setup = pool_with_assets,
        msg = SwapMsg::SwapExactAmountIn {
            token_in: coin(1_000_000, "denom0"),
            token_out_denom: "denom1".to_string(),
            token_out_min_amount: 1_000_001u128.into(),
        },
        err = ContractError::InsufficientTokenOut {
            min_required: 1_000_001u128.into(),
            amount_out: 1_000_000u128.into()
        }
    }
}

test_swap! {
    swap_exact_aomunt_out_with_exceeding_token_in_max_should_fail [expect error] {
        setup = pool_with_assets,
        msg = SwapMsg::SwapExactAmountOut {
            token_in_denom: "denom0".to_string(),
            token_in_max_amount: 999_999u128.into(),
            token_out: coin(1_000_000, "denom1"),
        },
        err = ContractError::ExcessiveRequiredTokenIn {
            limit: 999_999u128.into(),
            required: 1_000_000u128.into()
        }
    }
}

test_swap! {
    swap_with_unsupported_denom_for_token_out_should_fail [expect error] {
        setup = pool_with_assets,
        msgs = [
            SwapMsg::SwapExactAmountIn {
                token_in: coin(1_000_000, "denom0"),
                token_out_denom: "uosmo".to_string(),
                token_out_min_amount: Uint128::one(),
            },
            SwapMsg::SwapExactAmountOut {
                token_in_denom: "denom0".to_string(),
                token_in_max_amount: 1_000_000u128.into(),
                token_out: coin(1_000_000, "uosmo"),
            },
        ],
        err = ContractError::InvalidTransmuteDenom {
            denom: "uosmo".to_string(),
            expected_denom: vec!["denom0".to_string(), "denom1".to_string()]
        }
    }
}

test_swap! {
    swap_with_unsupported_denom_for_token_in_should_fail [expect error] {
        setup = pool_with_assets,
        msgs = [
            SwapMsg::SwapExactAmountIn {
                token_in: coin(1_000_000, "uosmo"),
                token_out_denom: "denom1".to_string(),
                token_out_min_amount: Uint128::one(),
            },
            SwapMsg::SwapExactAmountOut {
                token_in_denom: "uosmo".to_string(),
                token_in_max_amount: 1_000_000u128.into(),
                token_out: coin(1_000_000, "denom1"),
            },
        ],
        err = ContractError::InvalidTransmuteDenom {
            denom: "uosmo".to_string(),
            expected_denom: vec!["denom0".to_string(), "denom1".to_string()]
        }
    }
}
