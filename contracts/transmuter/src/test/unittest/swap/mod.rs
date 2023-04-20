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

mod empty_pool;
// TODO: configuration
// - [x] impl test_swap_exact_amount_out
// - impl invariant in test case
// - max pool
// - 3 pool
// - normal 2 pool
