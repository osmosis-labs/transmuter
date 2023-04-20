use cosmwasm_std::testing::{
    mock_dependencies, mock_env, mock_info, MockApi, MockQuerier, MockStorage,
};
use cosmwasm_std::{
    from_binary, BankMsg, Coin, Deps, DepsMut, Empty, OwnedDeps, ReplyOn, SubMsg, Uint128,
};

use crate::contract::{
    ContractExecMsg, ContractQueryMsg, ExecMsg, QueryMsg, TotalPoolLiquidityResponse,
    TotalSharesResponse, Transmuter,
};
use crate::{execute, query, ContractError};

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

fn get_total_shares(deps: Deps) -> Uint128 {
    from_binary::<TotalSharesResponse>(
        &query(
            deps,
            mock_env(),
            ContractQueryMsg::Transmuter(QueryMsg::GetTotalShares {}),
        )
        .unwrap(),
    )
    .unwrap()
    .total_shares
}

fn get_total_pool_liquidity(deps: Deps) -> Vec<Coin> {
    from_binary::<TotalPoolLiquidityResponse>(
        &query(
            deps,
            mock_env(),
            ContractQueryMsg::Transmuter(QueryMsg::GetTotalPoolLiquidity {}),
        )
        .unwrap(),
    )
    .unwrap()
    .total_pool_liquidity
}

fn assert_invariants(
    mut deps: OwnedDeps<MockStorage, MockApi, MockQuerier, Empty>,
    act: impl FnOnce(DepsMut) -> (),
) {
    // store previous shares and pool assets
    let prev_shares = get_total_shares(deps.as_ref());
    let prev_pool_assets = get_total_pool_liquidity(deps.as_ref());
    let sum_prev_pool_asset_amount = prev_pool_assets
        .iter()
        .map(|c| c.amount)
        .fold(Uint128::zero(), |acc, x| acc + x);

    // run the action
    act(deps.as_mut());

    // assert that shares stays the same
    let update_shares = get_total_shares(deps.as_ref());
    assert_eq!(prev_shares, update_shares);

    // assert that sum of pool assets stays the same
    let updated_pool_assets = get_total_pool_liquidity(deps.as_ref());
    let sum_updated_pool_asset_amount = updated_pool_assets
        .iter()
        .map(|c| c.amount)
        .fold(Uint128::zero(), |acc, x| acc + x);

    assert_eq!(sum_prev_pool_asset_amount, sum_updated_pool_asset_amount);
}

fn test_swap_success_case(
    deps: OwnedDeps<MockStorage, MockApi, MockQuerier, Empty>,
    msg: ExecMsg,
    funds: &[Coin],
    received: Coin,
) {
    assert_invariants(deps, move |deps| {
        // swap
        assert_eq!(
            execute(
                deps,
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
    });
}

fn test_swap_failed_case(
    deps: OwnedDeps<MockStorage, MockApi, MockQuerier, Empty>,
    msg: ExecMsg,
    funds: &[Coin],
    err: ContractError,
) {
    assert_invariants(deps, move |deps| {
        assert_eq!(
            execute(
                deps,
                mock_env(),
                mock_info(SWAPPER, funds),
                ContractExecMsg::Transmuter(msg)
            )
            .unwrap_err(),
            err,
        )
    });
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
// - [ ] impl invariant in test case
// - max pool
// - 3 pool
// - normal 2 pool
