use cosmwasm_std::testing::{
    mock_dependencies, mock_env, mock_info, MockApi, MockQuerier, MockStorage,
};
use cosmwasm_std::{BankMsg, Coin, Empty, OwnedDeps, ReplyOn, SubMsg, Uint128};

use crate::contract::{ContractExecMsg, ExecMsg, Transmuter};
use crate::{execute, ContractError};

const SWAPPER: &'static str = "swapperaddr";

#[macro_export]
macro_rules! test_swap {
    ($test_name:ident [expect ok] { setup = $setup:expr, msg = $msg:expr, funds = $funds:expr, received = $received:expr }) => {
        #[test]
        fn $test_name() {
            test_swap_success_case($setup, $msg, &$funds, $received);
        }
    };
    ($test_name:ident [expect error] { setup = $setup:expr, msg = $msg:expr, funds = $funds:expr, err = $err:expr }) => {
        #[test]
        fn $test_name() {
            test_swap_failed_case($setup, $msg, &$funds, $err);
        }
    };
    ($test_name:ident [expect ok] { setup = $setup:expr, msgs = $msgs:expr, funds = $funds:expr, received = $received:expr }) => {
        #[test]
        fn $test_name() {
            for msg in $msgs {
                test_swap_success_case($setup, msg, &$funds, $received);
            }
        }
    };
    ($test_name:ident [expect error] { setup = $setup:expr, msgs = $msgs:expr, funds = $funds:expr, err = $err:expr }) => {
        #[test]
        fn $test_name() {
            for msg in $msgs {
                test_swap_failed_case($setup, msg, &$funds, $err);
            }
        }
    };
}

fn test_swap_success_case(
    mut deps: OwnedDeps<MockStorage, MockApi, MockQuerier, Empty>,
    msg: ExecMsg,
    funds: &[Coin],
    received: Coin,
) {
    assert_eq!(
        execute(
            deps.as_mut(),
            mock_env(),
            mock_info(SWAPPER, funds),
            ContractExecMsg::Transmuter(msg)
        )
        .unwrap()
        .messages,
        vec![SubMsg {
            id: 0,
            msg: BankMsg::Send {
                to_address: SWAPPER.into(),
                amount: vec![received],
            }
            .into(),
            gas_limit: None,
            reply_on: ReplyOn::Never,
        }]
    );
}

fn test_swap_failed_case(
    mut deps: OwnedDeps<MockStorage, MockApi, MockQuerier, Empty>,
    msg: ExecMsg,
    funds: &[Coin],
    err: ContractError,
) {
    assert_eq!(
        execute(
            deps.as_mut(),
            mock_env(),
            mock_info(SWAPPER, funds),
            ContractExecMsg::Transmuter(msg)
        )
        .unwrap_err(),
        err,
    )
}

fn pool_with_single_lp(
    pool_assets: &[Coin],
) -> OwnedDeps<MockStorage, MockApi, MockQuerier, Empty> {
    let transmuter = Transmuter::new();
    let mut deps = mock_dependencies();
    // instantiate contract
    transmuter
        .instantiate(
            (deps.as_mut(), mock_env(), mock_info("instantiator", &[])),
            pool_assets.iter().map(|c| c.denom.clone()).collect(),
        )
        .unwrap();

    // join pool with initial tokens
    transmuter
        .join_pool((deps.as_mut(), mock_env(), mock_info("joiner", pool_assets)))
        .unwrap();

    deps
}

mod empty_pool {
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
}

// TODO: configuration
// - [x] impl test_swap_exact_amount_out
// - impl invariant in test case
// - max pool
// - 3 pool
// - normal 2 pool
