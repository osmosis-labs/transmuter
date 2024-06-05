use cosmwasm_std::{Coin, Uint128};

use osmosis_std::types::cosmos::bank::v1beta1::QueryBalanceRequest;
use osmosis_std::types::osmosis::poolmanager::v1beta1::{
    MsgSwapExactAmountIn, MsgSwapExactAmountOut, SwapAmountInRoute, SwapAmountOutRoute,
};

use osmosis_test_tube::{Account, Bank, Module, OsmosisTestApp};

use crate::asset::{convert_amount, AssetConfig, Rounding};
use crate::contract::sv::QueryMsg;
use crate::contract::{
    GetShareDenomResponse, GetTotalPoolLiquidityResponse, GetTotalSharesResponse,
    ListAssetConfigsResponse,
};

use crate::math::lcm_from_iter;
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

fn assert_invariants(t: TestEnv, act: impl FnOnce(&TestEnv) -> String) {
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

    let asset_configs = t
        .contract
        .query::<ListAssetConfigsResponse>(&QueryMsg::ListAssetConfigs {})
        .unwrap()
        .asset_configs;

    let sum_prev_pool_asset_value = total_pool_asset_value(&asset_configs, &prev_pool_assets);

    // run the action
    let rounding_denom = act(&t);

    // assert that shares stays the same
    let update_shares = t
        .contract
        .query::<GetTotalSharesResponse>(&QueryMsg::GetTotalShares {})
        .unwrap()
        .total_shares;
    assert_eq!(prev_shares, update_shares);

    // assert that sum of pool assets value stays the same or increase due to rounding
    let updated_pool_assets = t
        .contract
        .query::<GetTotalPoolLiquidityResponse>(&QueryMsg::GetTotalPoolLiquidity {})
        .unwrap()
        .total_pool_liquidity;
    let sum_updated_pool_asset_value = total_pool_asset_value(&asset_configs, &updated_pool_assets);

    // in case of rounding up token_in or rounding down token out

    if sum_updated_pool_asset_value > sum_prev_pool_asset_value {
        let normalization_factor = asset_configs
            .iter()
            .find(|ac| ac.denom == rounding_denom)
            .unwrap()
            .normalization_factor;

        let prev_pool_value_with_rounding_bound = sum_prev_pool_asset_value
            + convert_amount(
                Uint128::one(),
                normalization_factor,
                lcm_normalization_factor(&asset_configs),
                &Rounding::Down,
            )
            .unwrap();
        assert!(sum_updated_pool_asset_value < prev_pool_value_with_rounding_bound);
    } else {
        assert_eq!(sum_prev_pool_asset_value, sum_updated_pool_asset_value);
    }
}

fn total_pool_asset_value(asset_configs: &[AssetConfig], pool_assets: &[Coin]) -> Uint128 {
    pool_assets
        .iter()
        .map(|c| {
            let normalization_factor = asset_configs
                .iter()
                .find(|ac| ac.denom == c.denom)
                .unwrap()
                .normalization_factor;
            convert_amount(
                c.amount,
                normalization_factor,
                lcm_normalization_factor(asset_configs),
                &Rounding::Down,
            )
            .unwrap()
        })
        .fold(Uint128::zero(), |acc, x| acc + x)
}

fn lcm_normalization_factor(configs: &[AssetConfig]) -> Uint128 {
    let norm_factors = configs.iter().map(|c| c.normalization_factor);
    lcm_from_iter(norm_factors).unwrap()
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
        let rounding_denom = match msg {
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
                            token_out_denom: token_out_denom.clone(),
                        }],
                        token_in: Some(token_in.into()),
                        token_out_min_amount: token_out_min_amount.to_string(),
                    },
                    &t.accounts[SWAPPER],
                )
                .unwrap();

                token_out_denom
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
                            token_in_denom: token_in_denom.clone(),
                        }],
                        token_out: Some(token_out.into()),
                        token_in_max_amount: token_in_max_amount.to_string(),
                    },
                    &t.accounts[SWAPPER],
                )
                .unwrap();

                token_in_denom
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

        rounding_denom
    });
}

pub fn test_swap_share_denom_success_case(t: &TestEnv, msg: SwapMsg, sent: Coin, received: Coin) {
    let cp = CosmwasmPool::new(t.app);
    let bank = Bank::new(t.app);

    let asset_configs = t
        .contract
        .query::<ListAssetConfigsResponse>(&QueryMsg::ListAssetConfigs {})
        .unwrap()
        .asset_configs;

    let asset_config_map = asset_configs
        .iter()
        .map(|ac| (ac.denom.clone(), ac.clone()))
        .collect::<std::collections::HashMap<_, _>>();

    let share_denom = t
        .contract
        .query::<GetShareDenomResponse>(&QueryMsg::GetShareDenom {})
        .unwrap()
        .share_denom;

    let prev_in_denom_balance = bank
        .query_balance(&QueryBalanceRequest {
            address: t.accounts[SWAPPER].address(),
            denom: sent.denom.clone(),
        })
        .unwrap()
        .balance
        .unwrap();

    let prev_out_denom_balance = bank
        .query_balance(&QueryBalanceRequest {
            address: t.accounts[SWAPPER].address(),
            denom: received.denom.clone(),
        })
        .unwrap()
        .balance
        .unwrap();

    let prev_total_shares = t
        .contract
        .query::<GetTotalSharesResponse>(&QueryMsg::GetTotalShares {})
        .unwrap()
        .total_shares
        .u128();

    let prev_pool_assets = t
        .contract
        .query::<GetTotalPoolLiquidityResponse>(&QueryMsg::GetTotalPoolLiquidity {})
        .unwrap()
        .total_pool_liquidity;

    let sum_prev_pool_asset_value = total_pool_asset_value(&asset_configs, &prev_pool_assets);

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

    let updated_pool_assets = t
        .contract
        .query::<GetTotalPoolLiquidityResponse>(&QueryMsg::GetTotalPoolLiquidity {})
        .unwrap()
        .total_pool_liquidity;
    let sum_updated_pool_asset_value = total_pool_asset_value(&asset_configs, &updated_pool_assets);

    // updated out denom balance
    let updated_out_denom_balance = bank
        .query_balance(&QueryBalanceRequest {
            address: t.accounts[SWAPPER].address(),
            denom: received.denom.clone(),
        })
        .unwrap()
        .balance
        .unwrap();

    // updated in denom balance
    let updated_in_denom_balance = bank
        .query_balance(&QueryBalanceRequest {
            address: t.accounts[SWAPPER].address(),
            denom: sent.denom.clone(),
        })
        .unwrap()
        .balance
        .unwrap();

    let updated_total_shares = t
        .contract
        .query::<GetTotalSharesResponse>(&QueryMsg::GetTotalShares {})
        .unwrap()
        .total_shares
        .u128();

    let received_factor = asset_config_map
        .get(&received.denom)
        .map(|c| c.normalization_factor)
        .unwrap();

    // join pool equivalent
    if received.denom == share_denom {
        // -> minted new share tokens to swapper
        assert_eq!(
            updated_total_shares,
            prev_total_shares + received.amount.u128()
        );

        assert_eq!(
            sum_updated_pool_asset_value,
            sum_prev_pool_asset_value
                + convert_amount(
                    received.amount,
                    received_factor,
                    lcm_normalization_factor(&asset_configs),
                    &Rounding::Down // Rounding shouldn't matter since the target is LCM
                )
                .unwrap()
        );
    } else {
        // sent is alloyed denom, so we subtract the amount from total shares
        assert_eq!(updated_total_shares, prev_total_shares - sent.amount.u128());

        assert_eq!(
            sum_updated_pool_asset_value,
            sum_prev_pool_asset_value
                - convert_amount(
                    received.amount,
                    received_factor,
                    lcm_normalization_factor(&asset_configs),
                    &Rounding::Down // Rounding shouldn't matter since the target is LCM
                )
                .unwrap()
        );
    }

    assert_eq!(
        updated_in_denom_balance.amount.parse::<u128>().unwrap(),
        prev_in_denom_balance.amount.parse::<u128>().unwrap() - sent.amount.u128()
    );

    assert_eq!(
        received.amount.u128(),
        updated_out_denom_balance.amount.parse::<u128>().unwrap()
            - prev_out_denom_balance.amount.parse::<u128>().unwrap()
    );
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

        match msg {
            SwapMsg::SwapExactAmountIn {
                token_out_denom, ..
            } => token_out_denom,
            SwapMsg::SwapExactAmountOut { token_in_denom, .. } => token_in_denom,
        }
    });
}

fn pool_with_single_lp(
    app: &'_ OsmosisTestApp,
    pool_assets: Vec<Coin>,
    asset_configs: Vec<AssetConfig>,
) -> TestEnv<'_> {
    let cp = CosmwasmPool::new(app);
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
                .map(|coin| Coin::new(10000000000000000000000000, coin.denom.clone()))
                .collect(),
        )
        .with_instantiate_msg(crate::contract::sv::InstantiateMsg {
            pool_asset_configs: pool_assets
                .iter()
                .map(|c| {
                    asset_configs
                        .iter()
                        .find(|ac| ac.denom == c.denom)
                        .cloned()
                        .unwrap_or_else(|| AssetConfig::from_denom_str(c.denom.as_str()))
                })
                .collect(),
            alloyed_asset_subdenom: "transmuter/poolshare".to_string(),
            alloyed_asset_normalization_factor: Uint128::one(),
            admin: None,
            moderator: "osmo1cyyzpxplxdzkeea7kwsydadg87357qnahakaks".to_string(),
        })
        .build(app);

    let GetShareDenomResponse { share_denom } =
        t.contract.query(&QueryMsg::GetShareDenom {}).unwrap();

    if !non_zero_pool_assets.is_empty() {
        for token_in in non_zero_pool_assets {
            cp.swap_exact_amount_in(
                MsgSwapExactAmountIn {
                    sender: t.accounts["provider"].address(),
                    token_in: Some(token_in.into()),
                    routes: vec![SwapAmountInRoute {
                        pool_id: t.contract.pool_id,
                        token_out_denom: share_denom.clone(),
                    }],
                    token_out_min_amount: Uint128::one().to_string(),
                },
                &t.accounts["provider"],
            )
            .unwrap();
        }
    }

    t
}

mod client_error;
mod non_empty_pool;
mod swap_share_denom;
