use cosmwasm_std::{Coin, Uint128};

use osmosis_std::types::cosmos::bank::v1beta1::QueryBalanceRequest;
use osmosis_std::types::osmosis::poolmanager::v1beta1::{
    MsgSwapExactAmountIn, MsgSwapExactAmountOut, SwapAmountInRoute, SwapAmountOutRoute,
};

use osmosis_test_tube::{Account, Bank, Module, OsmosisTestApp};

use crate::contract::{ExecMsg, GetTotalPoolLiquidityResponse, GetTotalSharesResponse, QueryMsg};

use crate::test::modules::cosmwasm_pool::CosmwasmPool;
use crate::test::test_env::{assert_contract_err, TestEnv, TestEnvBuilder};
use crate::ContractError;

const SWAPPER: &str = "swapperaddr";

#[macro_export]
macro_rules! test_swap {
    ($test_name:ident [expect ok] { setup = $setup:ident, msg = $msg:expr, received = $received:expr }) => {
        #[test]
        fn $test_name() {
            let app = osmosis_test_tube::OsmosisTestApp::new();
            test_swap_success_case($setup(&app), $msg, $received);
        }
    };
    ($test_name:ident [expect error] { setup = $setup:ident, msg = $msg:expr, err = $err:expr }) => {
        #[test]
        fn $test_name() {
            let app = osmosis_test_tube::OsmosisTestApp::new();
            test_swap_failed_case($setup(&app), $msg, $err);
        }
    };
    ($test_name:ident [expect ok] { setup = $setup:expr, msgs = $msgs:expr, received = $received:expr }) => {
        #[test]
        fn $test_name() {
            for msg in $msgs {
                let app = osmosis_test_tube::OsmosisTestApp::new();
                test_swap_success_case($setup(&app), msg, $received);
            }
        }
    };
    ($test_name:ident [expect error] { setup = $setup:ident, msgs = $msgs:expr, err = $err:expr }) => {
        #[test]
        fn $test_name() {
            for msg in $msgs {
                let app = osmosis_test_tube::OsmosisTestApp::new();
                test_swap_failed_case($setup(&app), msg, $err);
            }
        }
    };
}

#[derive(Debug, Clone)]
pub enum SwapMsg {
    SwapExactAmountIn {
        token_in: Coin,
        token_out_denom: String,
        token_out_min_amount: Uint128,
    },
    SwapExactAmountOut {
        token_in_denom: String,
        token_in_max_amount: Uint128,
        token_out: Coin,
    },
}

fn assert_invariants(t: TestEnv, act: impl FnOnce(&TestEnv)) {
    // store previous shares and pool assets
    let prev_shares = t
        .contract
        .query::<GetTotalSharesResponse>(&QueryMsg::GetTotalShares {})
        .unwrap()
        .total_shares;
    let prev_pool_assets = t
        .contract
        .query::<GetTotalPoolLiquidityResponse>(&QueryMsg::GetTotalPoolLiquidity {})
        .unwrap()
        .total_pool_liquidity;

    let sum_prev_pool_asset_amount = prev_pool_assets
        .iter()
        .map(|c| c.amount)
        .fold(Uint128::zero(), |acc, x| acc + x);

    // run the action
    act(&t);

    // assert that shares stays the same
    let update_shares = t
        .contract
        .query::<GetTotalSharesResponse>(&QueryMsg::GetTotalShares {})
        .unwrap()
        .total_shares;
    assert_eq!(prev_shares, update_shares);

    // assert that sum of pool assets stays the same
    let updated_pool_assets = t
        .contract
        .query::<GetTotalPoolLiquidityResponse>(&QueryMsg::GetTotalPoolLiquidity {})
        .unwrap()
        .total_pool_liquidity;
    let sum_updated_pool_asset_amount = updated_pool_assets
        .iter()
        .map(|c| c.amount)
        .fold(Uint128::zero(), |acc, x| acc + x);

    assert_eq!(sum_prev_pool_asset_amount, sum_updated_pool_asset_amount);
}

fn test_swap_success_case(t: TestEnv, msg: SwapMsg, received: Coin) {
    assert_invariants(t, move |t| {
        let cp = CosmwasmPool::new(t.app);
        let bank = Bank::new(t.app);

        let prev_out_denom_balance = bank
            .query_balance(&QueryBalanceRequest {
                address: t.accounts[SWAPPER].address(),
                denom: received.denom.clone(),
            })
            .unwrap()
            .balance
            .unwrap();

        // swap
        match msg {
            SwapMsg::SwapExactAmountIn {
                token_in,
                token_out_denom,
                token_out_min_amount,
            } => {
                cp.swap_exact_amount_in(
                    MsgSwapExactAmountIn {
                        sender: t.accounts[SWAPPER].address(),
                        routes: vec![SwapAmountInRoute {
                            pool_id: 1,
                            token_out_denom,
                        }],
                        token_in: Some(token_in.into()),
                        token_out_min_amount: token_out_min_amount.to_string(),
                    },
                    &t.accounts[SWAPPER],
                )
                .unwrap();
            }
            SwapMsg::SwapExactAmountOut {
                token_in_denom,
                token_in_max_amount,
                token_out,
            } => {
                cp.swap_exact_amount_out(
                    MsgSwapExactAmountOut {
                        sender: t.accounts[SWAPPER].address(),
                        routes: vec![SwapAmountOutRoute {
                            pool_id: 1,
                            token_in_denom,
                        }],
                        token_out: Some(token_out.into()),
                        token_in_max_amount: token_in_max_amount.to_string(),
                    },
                    &t.accounts[SWAPPER],
                )
                .unwrap();
            }
        };

        // updated out denom balance
        let updated_out_denom_balance = bank
            .query_balance(&QueryBalanceRequest {
                address: t.accounts[SWAPPER].address(),
                denom: received.denom.clone(),
            })
            .unwrap()
            .balance
            .unwrap();

        assert_eq!(
            received.amount.u128(),
            updated_out_denom_balance.amount.parse::<u128>().unwrap()
                - prev_out_denom_balance.amount.parse::<u128>().unwrap()
        );
    });
}

fn test_swap_failed_case(t: TestEnv, msg: SwapMsg, err: ContractError) {
    assert_invariants(t, move |t| {
        let cp = CosmwasmPool::new(t.app);

        let actual_err = match msg.clone() {
            SwapMsg::SwapExactAmountIn {
                token_in,
                token_out_denom,
                token_out_min_amount,
            } => cp
                .swap_exact_amount_in(
                    MsgSwapExactAmountIn {
                        sender: t.accounts[SWAPPER].address(),
                        routes: vec![SwapAmountInRoute {
                            pool_id: 1,
                            token_out_denom,
                        }],
                        token_in: Some(token_in.into()),
                        token_out_min_amount: token_out_min_amount.to_string(),
                    },
                    &t.accounts[SWAPPER],
                )
                .unwrap_err(),
            SwapMsg::SwapExactAmountOut {
                token_in_denom,
                token_in_max_amount,
                token_out,
            } => cp
                .swap_exact_amount_out(
                    MsgSwapExactAmountOut {
                        sender: t.accounts[SWAPPER].address(),
                        routes: vec![SwapAmountOutRoute {
                            pool_id: 1,
                            token_in_denom,
                        }],
                        token_out: Some(token_out.into()),
                        token_in_max_amount: token_in_max_amount.to_string(),
                    },
                    &t.accounts[SWAPPER],
                )
                .unwrap_err(),
        };

        assert_contract_err(err, actual_err);
    });
}

fn pool_with_single_lp(app: &'_ OsmosisTestApp, pool_assets: Vec<Coin>) -> TestEnv<'_> {
    let non_zero_pool_assets = pool_assets
        .clone()
        .into_iter()
        .filter(|coin| !coin.amount.is_zero())
        .collect::<Vec<Coin>>();

    let t = TestEnvBuilder::new()
        .with_account("provider", non_zero_pool_assets.clone())
        .with_account(
            SWAPPER,
            non_zero_pool_assets
                .iter()
                .filter(|coin| !coin.amount.is_zero())
                .map(|coin| Coin::new(100000000000000000000, coin.denom.clone()))
                .collect(),
        )
        .with_instantiate_msg(crate::contract::InstantiateMsg {
            pool_asset_denoms: pool_assets.iter().map(|c| c.denom.clone()).collect(),
            admin: None,
        })
        .build(app);

    if !non_zero_pool_assets.is_empty() {
        t.contract
            .execute(
                &ExecMsg::JoinPool {},
                &non_zero_pool_assets,
                &t.accounts["provider"],
            )
            .unwrap();
    }

    t
}

mod client_error;
mod non_empty_pool;
