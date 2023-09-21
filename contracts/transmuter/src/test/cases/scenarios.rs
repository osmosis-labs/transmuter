use std::{str::FromStr, vec};

use crate::{
    contract::{
        ExecMsg, GetShareDenomResponse, GetSharesResponse, GetTotalPoolLiquidityResponse,
        GetTotalSharesResponse, InstantiateMsg, ListLimitersResponse, QueryMsg,
    },
    limiter::{ChangeLimiter, Limiter, LimiterParams, StaticLimiter, WindowConfig},
    test::{
        modules::cosmwasm_pool::CosmwasmPool,
        test_env::{assert_contract_err, TestEnvBuilder},
    },
    ContractError,
};
use cosmwasm_std::{Coin, Decimal, Uint128, Uint64};

use osmosis_std::types::{
    cosmos::bank::v1beta1::MsgSend,
    osmosis::poolmanager::v1beta1::{
        EstimateSwapExactAmountInRequest, EstimateSwapExactAmountInResponse, MsgSwapExactAmountIn,
        MsgSwapExactAmountOut, SwapAmountInRoute, SwapAmountOutRoute,
    },
};
use osmosis_test_tube::{Account, Bank, Module, OsmosisTestApp, Runner};

const AXL_USDC: &str = "ibc/AXLETHUSDC";
const AXL_DAI: &str = "ibc/AXLETHDAI";
const COSMOS_USDC: &str = "ibc/COSMOSUSDC";

const AXL_ETH: &str = "ibc/AXLETH";
const WH_ETH: &str = "ibc/WHETH";

#[test]
fn test_join_pool() {
    let app = OsmosisTestApp::new();
    let t = TestEnvBuilder::new()
        .with_account(
            "provider_1",
            vec![
                Coin::new(2_000, COSMOS_USDC),
                Coin::new(2_000, AXL_USDC),
                Coin::new(2_000, "urandom"),
            ],
        )
        .with_account(
            "provider_2",
            vec![Coin::new(2_000, COSMOS_USDC), Coin::new(2_000, AXL_USDC)],
        )
        .with_instantiate_msg(InstantiateMsg {
            pool_asset_denoms: vec![AXL_USDC.to_string(), COSMOS_USDC.to_string()],
            alloyed_asset_subdenom: "usdc".to_string(),
            admin: None,
            moderator: None,
        })
        .build(&app);

    // failed to join pool with 0 denom
    let err = t
        .contract
        .execute(&ExecMsg::JoinPool {}, &[], &t.accounts["provider_1"])
        .unwrap_err();

    assert_contract_err(ContractError::AtLeastSingleTokenExpected {}, err);

    // fail to join pool with denom that is not in the pool
    let tokens_in = vec![Coin::new(1_000, "urandom")];
    let err = t
        .contract
        .execute(&ExecMsg::JoinPool {}, &tokens_in, &t.accounts["provider_1"])
        .unwrap_err();

    assert_contract_err(
        ContractError::InvalidJoinPoolDenom {
            denom: "urandom".to_string(),
            expected_denom: vec![AXL_USDC.to_string(), COSMOS_USDC.to_string()],
        },
        err,
    );

    // join pool with correct pool's denom should added to the contract's balance and update state
    let tokens_in = vec![Coin::new(1_000, COSMOS_USDC)];

    t.contract
        .execute(&ExecMsg::JoinPool {}, &tokens_in, &t.accounts["provider_1"])
        .unwrap();

    // check contract balances
    t.assert_contract_balances(&tokens_in);

    // check pool balance
    let GetTotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .contract
        .query(&QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![Coin::new(0, AXL_USDC), tokens_in[0].clone()]
    );

    // check shares
    let GetSharesResponse { shares } = t
        .contract
        .query(&QueryMsg::GetShares {
            address: t.accounts["provider_1"].address(),
        })
        .unwrap();

    assert_eq!(shares, tokens_in[0].amount);

    // check total shares
    let GetTotalSharesResponse { total_shares } =
        t.contract.query(&QueryMsg::GetTotalShares {}).unwrap();

    assert_eq!(total_shares, tokens_in[0].amount);

    // join pool with multiple correct pool's denom should added to the contract's balance and update state
    let tokens_in = vec![Coin::new(1_000, AXL_USDC), Coin::new(1_000, COSMOS_USDC)];
    t.contract
        .execute(&ExecMsg::JoinPool {}, &tokens_in, &t.accounts["provider_1"])
        .unwrap();

    // check contract balances
    t.assert_contract_balances(&[Coin::new(1_000, AXL_USDC), Coin::new(2_000, COSMOS_USDC)]);

    // check pool balance
    let GetTotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .contract
        .query(&QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![Coin::new(1_000, AXL_USDC), Coin::new(2_000, COSMOS_USDC)]
    );

    // check shares
    let GetSharesResponse { shares } = t
        .contract
        .query(&QueryMsg::GetShares {
            address: t.accounts["provider_1"].address(),
        })
        .unwrap();

    assert_eq!(shares, Uint128::new(3000));

    // check total shares
    let GetTotalSharesResponse { total_shares } =
        t.contract.query(&QueryMsg::GetTotalShares {}).unwrap();

    assert_eq!(total_shares, Uint128::new(3000));

    // join pool with another provider with multiple correct pool's denom should added to the contract's balance and update state
    let tokens_in = vec![Coin::new(2_000, AXL_USDC), Coin::new(2_000, COSMOS_USDC)];
    t.contract
        .execute(&ExecMsg::JoinPool {}, &tokens_in, &t.accounts["provider_2"])
        .unwrap();

    // check contract balances
    t.assert_contract_balances(&[Coin::new(3_000, AXL_USDC), Coin::new(4_000, COSMOS_USDC)]);

    // check pool balance
    let GetTotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .contract
        .query(&QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![Coin::new(3_000, AXL_USDC), Coin::new(4_000, COSMOS_USDC)]
    );

    // check shares
    let GetSharesResponse { shares } = t
        .contract
        .query(&QueryMsg::GetShares {
            address: t.accounts["provider_2"].address(),
        })
        .unwrap();

    assert_eq!(shares, Uint128::new(4000));

    // check total shares
    let GetTotalSharesResponse { total_shares } =
        t.contract.query(&QueryMsg::GetTotalShares {}).unwrap();

    assert_eq!(total_shares, Uint128::new(7000));
}

#[test]
fn test_swap() {
    let app = OsmosisTestApp::new();
    let bank = Bank::new(&app);
    let cp = CosmwasmPool::new(&app);

    let t = TestEnvBuilder::new()
        .with_account(
            "alice",
            vec![
                Coin::new(1_500, AXL_USDC),
                Coin::new(1_000, COSMOS_USDC),
                Coin::new(1_000, "urandom2"),
            ],
        )
        .with_account("bob", vec![Coin::new(29_902, AXL_USDC)])
        .with_account("provider", vec![Coin::new(200_000, COSMOS_USDC)])
        .with_instantiate_msg(InstantiateMsg {
            pool_asset_denoms: vec![AXL_USDC.to_string(), COSMOS_USDC.to_string()],
            alloyed_asset_subdenom: "usdc".to_string(),
            admin: None,
            moderator: None,
        })
        .build(&app);

    // join pool
    let tokens_in = vec![Coin::new(100_000, COSMOS_USDC)];

    // join pool with send tokens should not update pool balance
    bank.send(
        MsgSend {
            from_address: t.accounts["provider"].address(),
            to_address: t.contract.contract_addr.clone(),
            amount: tokens_in.clone().into_iter().map(Into::into).collect(),
        },
        &t.accounts["provider"],
    )
    .unwrap();

    let err = cp
        .swap_exact_amount_in(
            MsgSwapExactAmountIn {
                sender: t.accounts["alice"].address(),
                token_in: Some(Coin::new(1_500, AXL_USDC).into()),
                routes: vec![SwapAmountInRoute {
                    pool_id: t.contract.pool_id,
                    token_out_denom: COSMOS_USDC.to_string(),
                }],
                token_out_min_amount: Uint128::from(1500u128).to_string(),
            },
            &t.accounts["alice"],
        )
        .unwrap_err();

    assert_contract_err(
        ContractError::InsufficientPoolAsset {
            required: Coin::new(1_500, COSMOS_USDC),
            available: Coin::new(0, COSMOS_USDC),
        },
        err,
    );

    // join pool properly
    t.contract
        .execute(&ExecMsg::JoinPool {}, &tokens_in, &t.accounts["provider"])
        .unwrap();

    // transmute with incorrect funds should still fail
    let err = cp
        .swap_exact_amount_in(
            MsgSwapExactAmountIn {
                sender: t.accounts["alice"].address(),
                token_in: Some(Coin::new(1_000, COSMOS_USDC).into()),
                routes: vec![SwapAmountInRoute {
                    pool_id: t.contract.pool_id,
                    token_out_denom: "urandom".to_string(),
                }],
                token_out_min_amount: Uint128::from(1_000u128).to_string(),
            },
            &t.accounts["alice"],
        )
        .unwrap_err();

    assert_contract_err(
        ContractError::InvalidTransmuteDenom {
            denom: "urandom".to_string(),
            expected_denom: vec![AXL_USDC.to_string(), COSMOS_USDC.to_string()],
        },
        err,
    );

    let err = cp
        .swap_exact_amount_in(
            MsgSwapExactAmountIn {
                sender: t.accounts["alice"].address(),
                token_in: Some(Coin::new(1_000, COSMOS_USDC).into()),
                routes: vec![SwapAmountInRoute {
                    pool_id: t.contract.pool_id,
                    token_out_denom: "urandom2".to_string(),
                }],
                token_out_min_amount: Uint128::from(1_000u128).to_string(),
            },
            &t.accounts["alice"],
        )
        .unwrap_err();

    assert_contract_err(
        ContractError::InvalidTransmuteDenom {
            denom: "urandom2".to_string(),
            expected_denom: vec![AXL_USDC.to_string(), COSMOS_USDC.to_string()],
        },
        err,
    );

    let routes = vec![SwapAmountInRoute {
        pool_id: t.contract.pool_id,
        token_out_denom: COSMOS_USDC.to_string(),
    }];

    let EstimateSwapExactAmountInResponse { token_out_amount } = t
        .app
        .query(
            "/osmosis.poolmanager.v1beta1.Query/EstimateSwapExactAmountIn",
            &EstimateSwapExactAmountInRequest {
                pool_id: t.contract.pool_id,
                token_in: format!("1500{AXL_USDC}"),
                routes: routes.clone(),
            },
        )
        .unwrap();

    assert_eq!(token_out_amount, "1500");

    // swap with correct token_in should succeed this time
    let token_in = Coin::new(1_500, AXL_USDC);
    cp.swap_exact_amount_in(
        MsgSwapExactAmountIn {
            sender: t.accounts["alice"].address(),
            token_in: Some(token_in.into()),
            routes,
            token_out_min_amount: Uint128::from(1_500u128).to_string(),
        },
        &t.accounts["alice"],
    )
    .unwrap();

    // check balances
    t.assert_contract_balances(&[
        Coin::new(1_500, AXL_USDC),
        Coin::new(100_000 + 100_000 - 1_500, COSMOS_USDC), // +100_000 due to bank send
    ]);

    let GetTotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .contract
        .query(&QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![
            Coin::new(1_500, AXL_USDC),
            Coin::new(100_000 - 1_500, COSMOS_USDC)
        ]
    );

    // +1_000 due to existing alice balance
    t.assert_account_balances(
        "alice",
        vec![
            Coin::new(1_500 + 1_000, COSMOS_USDC),
            Coin::new(1_000, "urandom2"),
        ],
        vec!["uosmo"],
    );

    // swap again with another user
    // swap with correct token_in should succeed this time
    let token_in = Coin::new(29_902, AXL_USDC);
    cp.swap_exact_amount_in(
        MsgSwapExactAmountIn {
            sender: t.accounts["bob"].address(),
            token_in: Some(token_in.into()),
            routes: vec![SwapAmountInRoute {
                pool_id: t.contract.pool_id,
                token_out_denom: COSMOS_USDC.to_string(),
            }],
            token_out_min_amount: Uint128::from(29_902u128).to_string(),
        },
        &t.accounts["bob"],
    )
    .unwrap();

    // check balances
    t.assert_contract_balances(&[
        Coin::new(1_500 + 29_902, AXL_USDC),
        Coin::new(100_000 + 100_000 - 1_500 - 29_902, COSMOS_USDC), // +100_000 due to bank send
    ]);

    let GetTotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .contract
        .query(&QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![
            Coin::new(1_500 + 29_902, AXL_USDC),
            Coin::new(100_000 - 1_500 - 29_902, COSMOS_USDC)
        ]
    );

    t.assert_account_balances("bob", vec![Coin::new(29_902, COSMOS_USDC)], vec!["uosmo"]);

    // swap back with `SwapExactAmountOut`
    let token_out = Coin::new(1_500, AXL_USDC);

    cp.swap_exact_amount_out(
        MsgSwapExactAmountOut {
            sender: t.accounts["bob"].address(),
            token_out: Some(token_out.into()),
            routes: vec![SwapAmountOutRoute {
                pool_id: t.contract.pool_id,
                token_in_denom: COSMOS_USDC.to_string(),
            }],
            token_in_max_amount: Uint128::from(1_500u128).to_string(),
        },
        &t.accounts["bob"],
    )
    .unwrap();

    let GetTotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .contract
        .query(&QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![
            Coin::new(29_902, AXL_USDC),
            Coin::new(100_000 - 29_902, COSMOS_USDC),
        ]
    );

    // check balances
    t.assert_contract_balances(&[
        Coin::new(29_902, AXL_USDC),
        Coin::new(100_000 + 100_000 - 29_902, COSMOS_USDC),
    ]);

    t.assert_account_balances(
        "bob",
        vec![
            Coin::new(1_500, AXL_USDC),
            Coin::new(29_902 - 1_500, COSMOS_USDC), // +100_000 due to bank send
        ],
        vec!["uosmo"],
    );
}

#[test]
fn test_exit_pool() {
    let app = OsmosisTestApp::new();
    let cp = CosmwasmPool::new(&app);

    let t = TestEnvBuilder::new()
        .with_account("user", vec![Coin::new(1_500, AXL_USDC)])
        .with_account("provider_1", vec![Coin::new(100_000, COSMOS_USDC)])
        .with_account("provider_2", vec![Coin::new(100_000, COSMOS_USDC)])
        .with_instantiate_msg(InstantiateMsg {
            pool_asset_denoms: vec![AXL_USDC.to_string(), COSMOS_USDC.to_string()],
            alloyed_asset_subdenom: "usdc".to_string(),
            admin: None,
            moderator: None,
        })
        .build(&app);

    // join pool
    t.contract
        .execute(
            &ExecMsg::JoinPool {},
            &[Coin::new(100_000, COSMOS_USDC)],
            &t.accounts["provider_1"],
        )
        .unwrap();

    t.contract
        .execute(
            &ExecMsg::JoinPool {},
            &[Coin::new(100_000, COSMOS_USDC)],
            &t.accounts["provider_2"],
        )
        .unwrap();

    // swap to build up token_in
    let token_in = Coin::new(1_500, AXL_USDC);

    cp.swap_exact_amount_in(
        MsgSwapExactAmountIn {
            sender: t.accounts["user"].address(),
            token_in: Some(token_in.into()),
            routes: vec![SwapAmountInRoute {
                pool_id: t.contract.pool_id,
                token_out_denom: COSMOS_USDC.to_string(),
            }],
            token_out_min_amount: Uint128::from(1_500u128).to_string(),
        },
        &t.accounts["user"],
    )
    .unwrap();

    // non-provider cannot exit_pool
    let err = t
        .contract
        .execute(
            &ExecMsg::ExitPool {
                tokens_out: vec![Coin::new(1_500, AXL_USDC)],
            },
            &[],
            &t.accounts["user"],
        )
        .unwrap_err();

    assert_contract_err(
        ContractError::InsufficientShares {
            required: 1500u128.into(),
            available: Uint128::zero(),
        },
        err,
    );

    // provider can exit pool
    t.contract
        .execute(
            &ExecMsg::ExitPool {
                tokens_out: vec![Coin::new(500, AXL_USDC)],
            },
            &[],
            &t.accounts["provider_1"],
        )
        .unwrap();

    // check shares
    let GetSharesResponse { shares } = t
        .contract
        .query(&QueryMsg::GetShares {
            address: t.accounts["provider_1"].address(),
        })
        .unwrap();

    assert_eq!(shares, Uint128::new(100_000 - 500));

    // check total shares
    let GetTotalSharesResponse { total_shares } =
        t.contract.query(&QueryMsg::GetTotalShares {}).unwrap();

    assert_eq!(total_shares, Uint128::new(200_000 - 500));

    // check balances
    t.assert_contract_balances(&[
        Coin::new(1500 - 500, AXL_USDC),
        Coin::new(200_000 - 1500, COSMOS_USDC),
    ]);

    let GetTotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .contract
        .query(&QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![
            Coin::new(1500 - 500, AXL_USDC),
            Coin::new(200_000 - 1500, COSMOS_USDC)
        ]
    );

    // provider can exit pool with any token
    t.contract
        .execute(
            &ExecMsg::ExitPool {
                tokens_out: vec![Coin::new(1_000, AXL_USDC), Coin::new(99_000, COSMOS_USDC)],
            },
            &[],
            &t.accounts["provider_2"],
        )
        .unwrap();

    // check shares
    let GetSharesResponse { shares } = t
        .contract
        .query(&QueryMsg::GetShares {
            address: t.accounts["provider_2"].address(),
        })
        .unwrap();

    assert_eq!(shares, Uint128::new(0));

    // check total shares
    let GetTotalSharesResponse { total_shares } =
        t.contract.query(&QueryMsg::GetTotalShares {}).unwrap();

    assert_eq!(total_shares, Uint128::new(200_000 - 500 - 1000 - 99_000));

    // check balances
    t.assert_contract_balances(&[Coin::new(200_000 - 1500 - 99_000, COSMOS_USDC)]);

    let GetTotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .contract
        .query(&QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![
            Coin::new(0, AXL_USDC),
            Coin::new(200_000 - 1500 - 99_000, COSMOS_USDC)
        ]
    );

    // exit pool with excess shares fails
    let err = t
        .contract
        .execute(
            &ExecMsg::ExitPool {
                tokens_out: vec![Coin::new(1, AXL_USDC)],
            },
            &[],
            &t.accounts["provider_2"],
        )
        .unwrap_err();

    assert_contract_err(
        ContractError::InsufficientShares {
            required: Uint128::one(),
            available: Uint128::zero(),
        },
        err,
    );

    // has remaining shares but no coins on the requested side
    let err = t
        .contract
        .execute(
            &ExecMsg::ExitPool {
                tokens_out: vec![Coin::new(1, AXL_USDC)],
            },
            &[],
            &t.accounts["provider_1"],
        )
        .unwrap_err();

    assert_contract_err(
        ContractError::InsufficientPoolAsset {
            required: Coin::new(1, AXL_USDC),
            available: Coin::new(0, AXL_USDC),
        },
        err,
    );
}

#[test]
fn test_3_pool_swap() {
    let app = OsmosisTestApp::new();
    let cp = CosmwasmPool::new(&app);

    let t = TestEnvBuilder::new()
        .with_account("alice", vec![Coin::new(1_500, AXL_USDC)])
        .with_account("bob", vec![Coin::new(1_500, AXL_DAI)])
        .with_account("provider", vec![Coin::new(100_000, COSMOS_USDC)])
        .with_instantiate_msg(InstantiateMsg {
            pool_asset_denoms: vec![
                AXL_USDC.to_string(),
                AXL_DAI.to_string(),
                COSMOS_USDC.to_string(),
            ],
            alloyed_asset_subdenom: "usdc".to_string(),
            admin: None,
            moderator: None,
        })
        .build(&app);

    // pool share denom
    let GetShareDenomResponse { share_denom } =
        t.contract.query(&QueryMsg::GetShareDenom {}).unwrap();

    // join pool
    t.contract
        .execute(
            &ExecMsg::JoinPool {},
            &[Coin::new(100_000, COSMOS_USDC)],
            &t.accounts["provider"],
        )
        .unwrap();

    // check shares
    let GetSharesResponse { shares } = t
        .contract
        .query(&QueryMsg::GetShares {
            address: t.accounts["provider"].address(),
        })
        .unwrap();

    assert_eq!(shares, Uint128::new(100_000));

    // check contract balances
    t.assert_contract_balances(&[Coin::new(100_000, COSMOS_USDC)]);

    // check provider balances
    t.assert_account_balances("provider", vec![], vec!["uosmo", &share_denom]);

    // check pool
    let GetTotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .contract
        .query(&QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![
            Coin::new(0, AXL_USDC),
            Coin::new(0, AXL_DAI),
            Coin::new(100_000, COSMOS_USDC)
        ]
    );

    // swap ETH_USDC to ETH_DAI should fail

    let err = cp
        .swap_exact_amount_in(
            MsgSwapExactAmountIn {
                sender: t.accounts["alice"].address(),
                token_in: Some(Coin::new(1_000, AXL_USDC).into()),
                routes: vec![SwapAmountInRoute {
                    pool_id: t.contract.pool_id,
                    token_out_denom: AXL_DAI.to_string(),
                }],
                token_out_min_amount: Uint128::from(1_000u128).to_string(),
            },
            &t.accounts["alice"],
        )
        .unwrap_err();

    assert_contract_err(
        ContractError::InsufficientPoolAsset {
            required: Coin::new(1_000, AXL_DAI),
            available: Coin::new(0, AXL_DAI),
        },
        err,
    );

    // swap ETH_USDC to COSMOS_USDC
    cp.swap_exact_amount_in(
        MsgSwapExactAmountIn {
            sender: t.accounts["alice"].address(),
            token_in: Some(Coin::new(1_000, AXL_USDC).into()),
            routes: vec![SwapAmountInRoute {
                pool_id: t.contract.pool_id,
                token_out_denom: COSMOS_USDC.to_string(),
            }],
            token_out_min_amount: Uint128::from(1_000u128).to_string(),
        },
        &t.accounts["alice"],
    )
    .unwrap();

    // check contract balances
    t.assert_contract_balances(&[Coin::new(1_000, AXL_USDC), Coin::new(99_000, COSMOS_USDC)]);

    // check alice balance
    t.assert_account_balances(
        "alice",
        vec![Coin::new(500, AXL_USDC), Coin::new(1_000, COSMOS_USDC)],
        vec!["uosmo", "ucosm"],
    );

    // check pool
    let GetTotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .contract
        .query(&QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![
            Coin::new(1_000, AXL_USDC),
            Coin::new(0, AXL_DAI),
            Coin::new(99_000, COSMOS_USDC)
        ]
    );

    // swap ETH_DAI to ETH_USDC

    cp.swap_exact_amount_in(
        MsgSwapExactAmountIn {
            sender: t.accounts["bob"].address(),
            token_in: Some(Coin::new(1_000, AXL_DAI).into()),
            routes: vec![SwapAmountInRoute {
                pool_id: t.contract.pool_id,
                token_out_denom: AXL_USDC.to_string(),
            }],
            token_out_min_amount: Uint128::from(1_000u128).to_string(),
        },
        &t.accounts["bob"],
    )
    .unwrap();

    // check contract balances
    t.assert_contract_balances(&[Coin::new(1_000, AXL_DAI), Coin::new(99_000, COSMOS_USDC)]);

    // check bob balances
    t.assert_account_balances(
        "bob",
        vec![Coin::new(500, AXL_DAI), Coin::new(1_000, AXL_USDC)],
        vec!["uosmo"],
    );

    // check pool
    let GetTotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .contract
        .query(&QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![
            Coin::new(0, AXL_USDC),
            Coin::new(1_000, AXL_DAI),
            Coin::new(99_000, COSMOS_USDC)
        ]
    );

    // provider exit pool
    t.contract
        .execute(
            &ExecMsg::ExitPool {
                tokens_out: vec![Coin::new(1_000, AXL_DAI), Coin::new(99_000, COSMOS_USDC)],
            },
            &[],
            &t.accounts["provider"],
        )
        .unwrap();

    // check balances
    t.assert_contract_balances(&[]);

    // // check pool
    let GetTotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .contract
        .query(&QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![
            Coin::new(0, AXL_USDC),
            Coin::new(0, AXL_DAI),
            Coin::new(0, COSMOS_USDC)
        ]
    );

    t.assert_contract_balances(&[]);
    t.assert_account_balances(
        "provider",
        vec![Coin::new(1_000, AXL_DAI), Coin::new(99_000, COSMOS_USDC)],
        vec!["uosmo"],
    );
}

#[test]
fn test_swap_alloyed_asset() {
    let app = OsmosisTestApp::new();

    let alloyed_asset_subdenom = "eth";
    let t = TestEnvBuilder::new()
        .with_account("alice", vec![Coin::new(1_500, AXL_ETH)])
        .with_account("bob", vec![Coin::new(1_500, WH_ETH)])
        .with_account(
            "provider",
            vec![Coin::new(100_000, AXL_ETH), Coin::new(100_000, WH_ETH)],
        )
        .with_instantiate_msg(InstantiateMsg {
            pool_asset_denoms: vec![AXL_ETH.to_string(), WH_ETH.to_string()],
            alloyed_asset_subdenom: alloyed_asset_subdenom.to_string(),
            admin: None,
            moderator: None,
        })
        .build(&app);

    // alloyed asset denom
    let GetShareDenomResponse { share_denom } =
        t.contract.query(&QueryMsg::GetShareDenom {}).unwrap();

    assert_eq!(
        format!(
            "factory/{}/alloyed/{}",
            t.contract.contract_addr, alloyed_asset_subdenom
        ),
        share_denom
    );
}

#[test]
fn test_limiters() {
    let app = OsmosisTestApp::new();
    let cp = CosmwasmPool::new(&app);

    let admin = app
        .init_account(&[Coin::new(100_000u128, "uosmo")])
        .unwrap();

    let t = TestEnvBuilder::new()
        .with_account(
            "alice",
            vec![
                Coin::new(1_000_000, AXL_USDC),
                Coin::new(1_000_000, COSMOS_USDC),
            ],
        )
        .with_account(
            "bob",
            vec![
                Coin::new(1_000_000, AXL_USDC),
                Coin::new(1_000_000, COSMOS_USDC),
            ],
        )
        .with_account("admin", vec![])
        .with_account(
            "provider",
            vec![
                Coin::new(1_000_000, AXL_USDC),
                Coin::new(1_000_000, COSMOS_USDC),
            ],
        )
        .with_instantiate_msg(InstantiateMsg {
            pool_asset_denoms: vec![AXL_USDC.to_string(), COSMOS_USDC.to_string()],
            alloyed_asset_subdenom: "usdc".to_string(),
            admin: Some(admin.address()),
            moderator: None,
        })
        .build(&app);

    // register limiters
    let config_1h = WindowConfig {
        window_size: Uint64::from(3_600_000_000_000u64), // 1 hrs
        division_count: Uint64::from(2u64),              // 30 mins each
    };

    let config_1w = WindowConfig {
        window_size: Uint64::from(25_920_000_000_000u64), // 7 days
        division_count: Uint64::from(2u64),               // 3.5 days each
    };

    t.contract
        .execute(
            &ExecMsg::RegisterLimiter {
                denom: AXL_USDC.to_string(),
                label: "1h".to_string(),
                limiter_params: LimiterParams::ChangeLimiter {
                    window_config: config_1h.clone(),
                    boundary_offset: Decimal::percent(10),
                },
            },
            &[],
            &t.accounts["admin"],
        )
        .unwrap();

    t.contract
        .execute(
            &ExecMsg::RegisterLimiter {
                denom: AXL_USDC.to_string(),
                label: "1w".to_string(),
                limiter_params: LimiterParams::ChangeLimiter {
                    window_config: config_1w.clone(),
                    boundary_offset: Decimal::percent(5),
                },
            },
            &[],
            &t.accounts["admin"],
        )
        .unwrap();

    t.contract
        .execute(
            &ExecMsg::RegisterLimiter {
                denom: COSMOS_USDC.to_string(),
                label: "1h".to_string(),
                limiter_params: LimiterParams::ChangeLimiter {
                    window_config: config_1h.clone(),
                    boundary_offset: Decimal::percent(10),
                },
            },
            &[],
            &t.accounts["admin"],
        )
        .unwrap();

    t.contract
        .execute(
            &ExecMsg::RegisterLimiter {
                denom: COSMOS_USDC.to_string(),
                label: "1w".to_string(),
                limiter_params: LimiterParams::ChangeLimiter {
                    window_config: config_1w.clone(),
                    boundary_offset: Decimal::percent(5),
                },
            },
            &[],
            &t.accounts["admin"],
        )
        .unwrap();

    t.contract
        .execute(
            &ExecMsg::RegisterLimiter {
                denom: COSMOS_USDC.to_string(),
                label: "static".to_string(),
                limiter_params: LimiterParams::StaticLimiter {
                    upper_limit: Decimal::percent(55),
                },
            },
            &[],
            &t.accounts["admin"],
        )
        .unwrap();

    // list all limiters
    let ListLimitersResponse { limiters } = t.contract.query(&QueryMsg::ListLimiters {}).unwrap();
    // assert that queried limiters = assigned limiters
    assert_eq!(
        limiters,
        vec![
            (
                (AXL_USDC.to_string(), "1h".to_string()),
                Limiter::ChangeLimiter(
                    ChangeLimiter::new(config_1h.clone(), Decimal::percent(10)).unwrap()
                )
            ),
            (
                (AXL_USDC.to_string(), "1w".to_string()),
                Limiter::ChangeLimiter(
                    ChangeLimiter::new(config_1w.clone(), Decimal::percent(5)).unwrap()
                )
            ),
            (
                (COSMOS_USDC.to_string(), "1h".to_string()),
                Limiter::ChangeLimiter(
                    ChangeLimiter::new(config_1h, Decimal::percent(10)).unwrap()
                )
            ),
            (
                (COSMOS_USDC.to_string(), "1w".to_string()),
                Limiter::ChangeLimiter(ChangeLimiter::new(config_1w, Decimal::percent(5)).unwrap())
            ),
            (
                (COSMOS_USDC.to_string(), "static".to_string()),
                Limiter::StaticLimiter(StaticLimiter::new(Decimal::percent(55)).unwrap())
            ),
        ]
    );

    // join pool - weight = 50:50
    t.contract
        .execute(
            &ExecMsg::JoinPool {},
            &[
                Coin::new(500_000, AXL_USDC),
                Coin::new(500_000, COSMOS_USDC),
            ],
            &t.accounts["provider"],
        )
        .unwrap();

    app.increase_time(60 * 60); // -> + 60 mins

    // swap 100_001 AXL_USDC to COSMOS_USDC
    let err = cp
        .swap_exact_amount_in(
            MsgSwapExactAmountIn {
                sender: t.accounts["alice"].address(),
                token_in: Some(Coin::new(100_001, AXL_USDC).into()),
                routes: vec![SwapAmountInRoute {
                    pool_id: t.contract.pool_id,
                    token_out_denom: COSMOS_USDC.to_string(),
                }],
                token_out_min_amount: Uint128::from(100_001u128).to_string(),
            },
            &t.accounts["alice"],
        )
        .unwrap_err();

    assert_contract_err(
        ContractError::UpperLimitExceeded {
            denom: AXL_USDC.to_string(),
            upper_limit: Decimal::from_str("0.6").unwrap(),
            value: Decimal::from_str("0.600001").unwrap(),
        },
        err,
    );

    // swap exact amount out 50_001 COSMOS_USDC to AXL_USDC
    let err = cp
        .swap_exact_amount_out(
            MsgSwapExactAmountOut {
                sender: t.accounts["alice"].address(),
                token_out: Some(Coin::new(50_001, AXL_USDC).into()),
                routes: vec![SwapAmountOutRoute {
                    pool_id: t.contract.pool_id,
                    token_in_denom: COSMOS_USDC.to_string(),
                }],
                token_in_max_amount: Uint128::from(50_001u128).to_string(),
            },
            &t.accounts["alice"],
        )
        .unwrap_err();

    assert_contract_err(
        ContractError::UpperLimitExceeded {
            denom: COSMOS_USDC.to_string(),
            upper_limit: Decimal::from_str("0.55").unwrap(),
            value: Decimal::from_str("0.550001").unwrap(),
        },
        err,
    );

    // swap 50_000 COSMOS_USDC to AXL_USDC
    cp.swap_exact_amount_in(
        MsgSwapExactAmountIn {
            sender: t.accounts["alice"].address(),
            token_in: Some(Coin::new(50_000, COSMOS_USDC).into()),
            routes: vec![SwapAmountInRoute {
                pool_id: t.contract.pool_id,
                token_out_denom: AXL_USDC.to_string(),
            }],
            token_out_min_amount: Uint128::from(50_000u128).to_string(),
        },
        &t.accounts["alice"],
    )
    .unwrap();

    app.increase_time(60 * 60); // -> + 120 mins

    // exit pool 200_000 COSMOSUSDC
    let err = t
        .contract
        .execute(
            &ExecMsg::ExitPool {
                tokens_out: vec![Coin::new(200_000, COSMOS_USDC)],
            },
            &[],
            &t.accounts["provider"],
        )
        .unwrap_err();

    assert_contract_err(
        ContractError::UpperLimitExceeded {
            denom: AXL_USDC.to_string(),
            upper_limit: Decimal::from_str("0.55").unwrap(),
            value: Decimal::from_str("0.5625").unwrap(),
        },
        err,
    );

    // join pool 200_000 AXL_USDC
    let err = t
        .contract
        .execute(
            &ExecMsg::JoinPool {},
            &[Coin::new(200_000, AXL_USDC)],
            &t.accounts["provider"],
        )
        .unwrap_err();

    assert_contract_err(
        ContractError::UpperLimitExceeded {
            denom: AXL_USDC.to_string(),
            upper_limit: Decimal::from_str("0.525034626038781163").unwrap(),
            value: Decimal::from_str("0.5416666666666666").unwrap(),
        },
        err,
    );

    // query share denom
    let GetShareDenomResponse { share_denom } =
        t.contract.query(&QueryMsg::GetShareDenom {}).unwrap();

    let alloyed_denom = share_denom;

    // swap amount in alloyed denom to AXL_USDC
    let err = cp
        .swap_exact_amount_in(
            MsgSwapExactAmountIn {
                sender: t.accounts["provider"].address(),
                token_in: Some(Coin::new(200_000, alloyed_denom.clone()).into()),
                routes: vec![SwapAmountInRoute {
                    pool_id: t.contract.pool_id,
                    token_out_denom: AXL_USDC.to_string(),
                }],
                token_out_min_amount: Uint128::from(200_000u128).to_string(),
            },
            &t.accounts["provider"],
        )
        .unwrap_err();

    assert_contract_err(
        ContractError::UpperLimitExceeded {
            denom: COSMOS_USDC.to_string(),
            upper_limit: Decimal::from_str("0.65").unwrap(),
            value: Decimal::from_str("0.6875").unwrap(),
        },
        err,
    );

    // swap amount out COSMOS_USDC -> alloyed denom
    let err = cp
        .swap_exact_amount_out(
            MsgSwapExactAmountOut {
                sender: t.accounts["provider"].address(),
                token_out: Some(Coin::new(200_000, alloyed_denom).into()),
                routes: vec![SwapAmountOutRoute {
                    pool_id: t.contract.pool_id,
                    token_in_denom: COSMOS_USDC.to_string(),
                }],
                token_in_max_amount: Uint128::from(200_000u128).to_string(),
            },
            &t.accounts["provider"],
        )
        .unwrap_err();

    assert_contract_err(
        ContractError::UpperLimitExceeded {
            denom: COSMOS_USDC.to_string(),
            upper_limit: Decimal::from_str("0.575").unwrap(),
            value: Decimal::from_str("0.625").unwrap(),
        },
        err,
    );
}
