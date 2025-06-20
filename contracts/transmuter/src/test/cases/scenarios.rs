use std::{str::FromStr, vec};

use crate::{
    asset::AssetConfig,
    contract::{
        sv::{ExecMsg, InstantiateMsg, QueryMsg},
        GetShareDenomResponse, GetSharesResponse, GetTotalPoolLiquidityResponse,
        GetTotalSharesResponse,
    },
    limiter::LimiterParams,
    scope::Scope,
    test::{
        modules::cosmwasm_pool::CosmwasmPool,
        test_env::{assert_contract_err, TestEnvBuilder},
    },
    ContractError,
};
use cosmwasm_std::{coin, Decimal, Uint128};

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
                coin(2_000, COSMOS_USDC),
                coin(2_000, AXL_USDC),
                coin(2_000, "urandom"),
            ],
        )
        .with_account(
            "provider_2",
            vec![coin(2_000, COSMOS_USDC), coin(2_000, AXL_USDC)],
        )
        .with_account("moderator", vec![])
        .with_instantiate_msg(InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str(AXL_USDC),
                AssetConfig::from_denom_str(COSMOS_USDC),
            ],
            alloyed_asset_subdenom: "usdc".to_string(),
            alloyed_asset_normalization_factor: Uint128::one(),
            admin: None,
            moderator: "osmo1cyyzpxplxdzkeea7kwsydadg87357qnahakaks".to_string(),
        })
        .build(&app);

    // failed to join pool with 0 denom
    let err = t
        .contract
        .execute(&ExecMsg::JoinPool {}, &[], &t.accounts["provider_1"])
        .unwrap_err();

    assert_contract_err(ContractError::AtLeastSingleTokenExpected {}, err);

    // fail to join pool with denom that is not in the pool
    let tokens_in = vec![coin(1_000, "urandom")];
    let err = t
        .contract
        .execute(&ExecMsg::JoinPool {}, &tokens_in, &t.accounts["provider_1"])
        .unwrap_err();

    assert_contract_err(
        ContractError::InvalidTransmuteDenom {
            denom: "urandom".to_string(),
            expected_denom: vec![AXL_USDC.to_string(), COSMOS_USDC.to_string()],
        },
        err,
    );

    // join pool with correct pool's denom should added to the contract's balance and update state
    let tokens_in = vec![coin(1_000, COSMOS_USDC)];

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
        vec![coin(0, AXL_USDC), tokens_in[0].clone()]
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
    let tokens_in = vec![coin(1_000, AXL_USDC), coin(1_000, COSMOS_USDC)];
    t.contract
        .execute(&ExecMsg::JoinPool {}, &tokens_in, &t.accounts["provider_1"])
        .unwrap();

    // check contract balances
    t.assert_contract_balances(&[coin(1_000, AXL_USDC), coin(2_000, COSMOS_USDC)]);

    // check pool balance
    let GetTotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .contract
        .query(&QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![coin(1_000, AXL_USDC), coin(2_000, COSMOS_USDC)]
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
    let tokens_in = vec![coin(2_000, AXL_USDC), coin(2_000, COSMOS_USDC)];
    t.contract
        .execute(&ExecMsg::JoinPool {}, &tokens_in, &t.accounts["provider_2"])
        .unwrap();

    // check contract balances
    t.assert_contract_balances(&[coin(3_000, AXL_USDC), coin(4_000, COSMOS_USDC)]);

    // check pool balance
    let GetTotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .contract
        .query(&QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![coin(3_000, AXL_USDC), coin(4_000, COSMOS_USDC)]
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
                coin(1_500, AXL_USDC),
                coin(1_000, COSMOS_USDC),
                coin(1_000, "urandom2"),
            ],
        )
        .with_account("bob", vec![coin(29_902, AXL_USDC)])
        .with_account("provider", vec![coin(200_000, COSMOS_USDC)])
        .with_instantiate_msg(InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str(AXL_USDC),
                AssetConfig::from_denom_str(COSMOS_USDC),
            ],
            alloyed_asset_subdenom: "usdc".to_string(),
            alloyed_asset_normalization_factor: Uint128::one(),
            admin: None,
            moderator: "osmo1cyyzpxplxdzkeea7kwsydadg87357qnahakaks".to_string(),
        })
        .build(&app);

    // join pool
    let tokens_in = vec![coin(100_000, COSMOS_USDC)];

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
                token_in: Some(coin(1_500, AXL_USDC).into()),
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
            required: coin(1_500, COSMOS_USDC),
            available: coin(0, COSMOS_USDC),
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
                token_in: Some(coin(1_000, COSMOS_USDC).into()),
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
                token_in: Some(coin(1_000, COSMOS_USDC).into()),
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
            #[allow(deprecated)]
            &EstimateSwapExactAmountInRequest {
                pool_id: t.contract.pool_id,
                token_in: format!("1500{AXL_USDC}"),
                routes: routes.clone(),
                sender: t.accounts["alice"].address(),
            },
        )
        .unwrap();

    assert_eq!(token_out_amount, "1500");

    // swap with correct token_in should succeed this time
    let token_in = coin(1_500, AXL_USDC);
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
        coin(1_500, AXL_USDC),
        coin(100_000 + 100_000 - 1_500, COSMOS_USDC), // +100_000 due to bank send
    ]);

    let GetTotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .contract
        .query(&QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![coin(1_500, AXL_USDC), coin(100_000 - 1_500, COSMOS_USDC)]
    );

    // +1_000 due to existing alice balance
    t.assert_account_balances(
        "alice",
        vec![coin(1_500 + 1_000, COSMOS_USDC), coin(1_000, "urandom2")],
        vec!["uosmo"],
    );

    // swap again with another user
    // swap with correct token_in should succeed this time
    let token_in = coin(29_902, AXL_USDC);
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
        coin(1_500 + 29_902, AXL_USDC),
        coin(100_000 + 100_000 - 1_500 - 29_902, COSMOS_USDC), // +100_000 due to bank send
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
            coin(1_500 + 29_902, AXL_USDC),
            coin(100_000 - 1_500 - 29_902, COSMOS_USDC)
        ]
    );

    t.assert_account_balances("bob", vec![coin(29_902, COSMOS_USDC)], vec!["uosmo"]);

    // swap back with `SwapExactAmountOut`
    let token_out = coin(1_500, AXL_USDC);

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
        vec![coin(29_902, AXL_USDC), coin(100_000 - 29_902, COSMOS_USDC),]
    );

    // check balances
    t.assert_contract_balances(&[
        coin(29_902, AXL_USDC),
        coin(100_000 + 100_000 - 29_902, COSMOS_USDC),
    ]);

    t.assert_account_balances(
        "bob",
        vec![
            coin(1_500, AXL_USDC),
            coin(29_902 - 1_500, COSMOS_USDC), // +100_000 due to bank send
        ],
        vec!["uosmo"],
    );
}

#[test]
fn test_exit_pool() {
    let app = OsmosisTestApp::new();

    let t = TestEnvBuilder::new()
        .with_account(
            "provider",
            vec![coin(100_000, AXL_USDC), coin(100_000, COSMOS_USDC)],
        )
        .with_account("user", vec![coin(1_000, AXL_USDC)])
        .with_instantiate_msg(InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str(AXL_USDC),
                AssetConfig::from_denom_str(COSMOS_USDC),
            ],
            alloyed_asset_subdenom: "usdc".to_string(),
            alloyed_asset_normalization_factor: Uint128::one(),
            admin: None,
            moderator: "osmo1cyyzpxplxdzkeea7kwsydadg87357qnahakaks".to_string(),
        })
        .build(&app);

    // Join pool with 50:50 ratio
    t.contract
        .execute(
            &ExecMsg::JoinPool {},
            &[coin(50_000, AXL_USDC), coin(50_000, COSMOS_USDC)],
            &t.accounts["provider"],
        )
        .unwrap();

    // Check initial shares
    let GetSharesResponse { shares } = t
        .contract
        .query(&QueryMsg::GetShares {
            address: t.accounts["provider"].address(),
        })
        .unwrap();

    assert_eq!(shares, Uint128::new(100_000));

    // Non-provider cannot exit pool
    let err = t
        .contract
        .execute(
            &ExecMsg::ExitPool {
                tokens_out: vec![coin(1_000, AXL_USDC)],
            },
            &[],
            &t.accounts["user"],
        )
        .unwrap_err();

    assert_contract_err(
        ContractError::InsufficientShares {
            required: 1000u128.into(),
            available: Uint128::zero(),
        },
        err,
    );

    // Provider can exit pool with partial amount
    t.contract
        .execute(
            &ExecMsg::ExitPool {
                tokens_out: vec![coin(10_000, AXL_USDC)],
            },
            &[],
            &t.accounts["provider"],
        )
        .unwrap();

    // Check updated shares
    let GetSharesResponse { shares } = t
        .contract
        .query(&QueryMsg::GetShares {
            address: t.accounts["provider"].address(),
        })
        .unwrap();

    assert_eq!(shares, Uint128::new(90_000));

    // Provider can exit pool with multiple tokens
    t.contract
        .execute(
            &ExecMsg::ExitPool {
                tokens_out: vec![coin(20_000, AXL_USDC), coin(20_000, COSMOS_USDC)],
            },
            &[],
            &t.accounts["provider"],
        )
        .unwrap();

    // Check final shares
    let GetSharesResponse { shares } = t
        .contract
        .query(&QueryMsg::GetShares {
            address: t.accounts["provider"].address(),
        })
        .unwrap();

    assert_eq!(shares, Uint128::new(50_000));

    // Exit pool with excess shares fails
    let err = t
        .contract
        .execute(
            &ExecMsg::ExitPool {
                tokens_out: vec![coin(60_000, AXL_USDC)],
            },
            &[],
            &t.accounts["provider"],
        )
        .unwrap_err();

    assert_contract_err(
        ContractError::InsufficientShares {
            required: 60000u128.into(),
            available: Uint128::new(50_000),
        },
        err,
    );

    // Exit remaining tokens in the pool
    let GetTotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .contract
        .query(&QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    t.contract
        .execute(
            &ExecMsg::ExitPool {
                tokens_out: total_pool_liquidity.clone(),
            },
            &[],
            &t.accounts["provider"],
        )
        .unwrap();

    // Check final shares should be zero
    let GetSharesResponse { shares } = t
        .contract
        .query(&QueryMsg::GetShares {
            address: t.accounts["provider"].address(),
        })
        .unwrap();

    assert_eq!(shares, Uint128::zero());
}

#[test]
fn test_3_pool_swap() {
    let app = OsmosisTestApp::new();
    let cp = CosmwasmPool::new(&app);

    let t = TestEnvBuilder::new()
        .with_account("alice", vec![coin(1_500, AXL_USDC)])
        .with_account("bob", vec![coin(1_500, AXL_DAI)])
        .with_account("provider", vec![coin(100_000, COSMOS_USDC)])
        .with_instantiate_msg(InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str(AXL_USDC),
                AssetConfig::from_denom_str(AXL_DAI),
                AssetConfig::from_denom_str(COSMOS_USDC),
            ],
            alloyed_asset_subdenom: "usdc".to_string(),
            alloyed_asset_normalization_factor: Uint128::one(),
            admin: None,
            moderator: "osmo1cyyzpxplxdzkeea7kwsydadg87357qnahakaks".to_string(),
        })
        .build(&app);

    // pool share denom
    let GetShareDenomResponse { share_denom } =
        t.contract.query(&QueryMsg::GetShareDenom {}).unwrap();

    // join pool
    t.contract
        .execute(
            &ExecMsg::JoinPool {},
            &[coin(100_000, COSMOS_USDC)],
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
    t.assert_contract_balances(&[coin(100_000, COSMOS_USDC)]);

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
            coin(0, AXL_USDC),
            coin(0, AXL_DAI),
            coin(100_000, COSMOS_USDC)
        ]
    );

    // swap ETH_USDC to ETH_DAI should fail

    let err = cp
        .swap_exact_amount_in(
            MsgSwapExactAmountIn {
                sender: t.accounts["alice"].address(),
                token_in: Some(coin(1_000, AXL_USDC).into()),
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
            required: coin(1_000, AXL_DAI),
            available: coin(0, AXL_DAI),
        },
        err,
    );

    // swap ETH_USDC to COSMOS_USDC
    cp.swap_exact_amount_in(
        MsgSwapExactAmountIn {
            sender: t.accounts["alice"].address(),
            token_in: Some(coin(1_000, AXL_USDC).into()),
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
    t.assert_contract_balances(&[coin(1_000, AXL_USDC), coin(99_000, COSMOS_USDC)]);

    // check alice balance
    t.assert_account_balances(
        "alice",
        vec![coin(500, AXL_USDC), coin(1_000, COSMOS_USDC)],
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
            coin(1_000, AXL_USDC),
            coin(0, AXL_DAI),
            coin(99_000, COSMOS_USDC)
        ]
    );

    // swap ETH_DAI to ETH_USDC

    cp.swap_exact_amount_in(
        MsgSwapExactAmountIn {
            sender: t.accounts["bob"].address(),
            token_in: Some(coin(1_000, AXL_DAI).into()),
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
    t.assert_contract_balances(&[coin(1_000, AXL_DAI), coin(99_000, COSMOS_USDC)]);

    // check bob balances
    t.assert_account_balances(
        "bob",
        vec![coin(500, AXL_DAI), coin(1_000, AXL_USDC)],
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
            coin(0, AXL_USDC),
            coin(1_000, AXL_DAI),
            coin(99_000, COSMOS_USDC)
        ]
    );

    // provider exit pool
    t.contract
        .execute(
            &ExecMsg::ExitPool {
                tokens_out: vec![coin(1_000, AXL_DAI), coin(99_000, COSMOS_USDC)],
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
        vec![coin(0, AXL_USDC), coin(0, AXL_DAI), coin(0, COSMOS_USDC)]
    );

    t.assert_contract_balances(&[]);
    t.assert_account_balances(
        "provider",
        vec![coin(1_000, AXL_DAI), coin(99_000, COSMOS_USDC)],
        vec!["uosmo"],
    );
}

#[test]
fn test_swap_alloyed_asset() {
    let app = OsmosisTestApp::new();

    let alloyed_asset_subdenom = "eth";
    let t = TestEnvBuilder::new()
        .with_account("alice", vec![coin(1_500, AXL_ETH)])
        .with_account("bob", vec![coin(1_500, WH_ETH)])
        .with_account(
            "provider",
            vec![coin(100_000, AXL_ETH), coin(100_000, WH_ETH)],
        )
        .with_instantiate_msg(InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str(AXL_ETH),
                AssetConfig::from_denom_str(WH_ETH),
            ],
            alloyed_asset_subdenom: alloyed_asset_subdenom.to_string(),
            alloyed_asset_normalization_factor: Uint128::one(),
            admin: None,
            moderator: "osmo1cyyzpxplxdzkeea7kwsydadg87357qnahakaks".to_string(),
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
    let admin = app.init_account(&[coin(100_000u128, "uosmo")]).unwrap();

    let t = TestEnvBuilder::new()
        .with_account(
            "user",
            vec![coin(1_000_000, AXL_USDC), coin(1_000_000, COSMOS_USDC)],
        )
        .with_account("admin", vec![])
        .with_instantiate_msg(InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str(AXL_USDC),
                AssetConfig::from_denom_str(COSMOS_USDC),
            ],
            alloyed_asset_subdenom: "usdc".to_string(),
            alloyed_asset_normalization_factor: Uint128::one(),
            admin: Some(admin.address()),
            moderator: "osmo1cyyzpxplxdzkeea7kwsydadg87357qnahakaks".to_string(),
        })
        .build(&app);

    // Register a static limiter for AXL_USDC at 60%
    t.contract
        .execute(
            &ExecMsg::RegisterLimiter {
                scope: Scope::Denom(AXL_USDC.to_string()),
                label: "static".to_string(),
                limiter_params: LimiterParams::StaticLimiter {
                    upper_limit: Decimal::percent(60),
                },
            },
            &[],
            &t.accounts["admin"],
        )
        .unwrap();

    // Join pool with 50:50, should succeed
    t.contract
        .execute(
            &ExecMsg::JoinPool {},
            &[coin(500_000, AXL_USDC), coin(500_000, COSMOS_USDC)],
            &t.accounts["user"],
        )
        .unwrap();

    // Try to add more AXL_USDC to exceed 60% weight, should fail
    let err = t
        .contract
        .execute(
            &ExecMsg::JoinPool {},
            &[coin(300_000, AXL_USDC)],
            &t.accounts["user"],
        )
        .unwrap_err();

    assert_contract_err(
        ContractError::UpperLimitExceeded {
            scope: Scope::denom(AXL_USDC),
            upper_limit: Decimal::from_str("0.6").unwrap(),
            value: Decimal::from_str("0.615384615384615384").unwrap(), // match contract precision
        },
        err,
    );

    // Remove AXL_USDC (decrease weight), should always succeed
    t.contract
        .execute(
            &ExecMsg::ExitPool {
                tokens_out: vec![coin(100_000, AXL_USDC)],
            },
            &[],
            &t.accounts["user"],
        )
        .unwrap();
}

#[test]
fn test_register_limiter_after_having_liquidity() {
    let app = OsmosisTestApp::new();
    let admin = app.init_account(&[coin(100_000u128, "uosmo")]).unwrap();

    let t = TestEnvBuilder::new()
        .with_account(
            "provider",
            vec![coin(1_000_000, AXL_USDC), coin(1_000_000, COSMOS_USDC)],
        )
        .with_account("admin", vec![])
        .with_account("user", vec![coin(1_000, AXL_USDC)])
        .with_instantiate_msg(InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str(AXL_USDC),
                AssetConfig::from_denom_str(COSMOS_USDC),
            ],
            alloyed_asset_subdenom: "usdc".to_string(),
            alloyed_asset_normalization_factor: Uint128::one(),
            admin: Some(admin.address()),
            moderator: "osmo1cyyzpxplxdzkeea7kwsydadg87357qnahakaks".to_string(),
        })
        .build(&app);

    // Join pool with initial liquidity (50:50 ratio)
    t.contract
        .execute(
            &ExecMsg::JoinPool {},
            &[coin(500_000, AXL_USDC), coin(500_000, COSMOS_USDC)],
            &t.accounts["provider"],
        )
        .unwrap();

    // Register a limiter for COSMOS_USDC at 60% after liquidity exists
    t.contract
        .execute(
            &ExecMsg::RegisterLimiter {
                scope: Scope::Denom(COSMOS_USDC.to_string()),
                label: "static".to_string(),
                limiter_params: LimiterParams::StaticLimiter {
                    upper_limit: Decimal::percent(60),
                },
            },
            &[],
            &t.accounts["admin"],
        )
        .unwrap();

    // Try to add more COSMOS_USDC to exceed 60% weight, should fail
    let err = t
        .contract
        .execute(
            &ExecMsg::JoinPool {},
            &[coin(400_000, COSMOS_USDC)],
            &t.accounts["provider"],
        )
        .unwrap_err();

    assert_contract_err(
        ContractError::UpperLimitExceeded {
            scope: Scope::denom(COSMOS_USDC),
            upper_limit: Decimal::from_str("0.6").unwrap(),
            value: Decimal::from_str("0.642857142857142857").unwrap(), // match contract precision
        },
        err,
    );

    // Adding AXL_USDC should succeed (doesn't affect COSMOS_USDC weight)
    t.contract
        .execute(
            &ExecMsg::JoinPool {},
            &[coin(100_000, AXL_USDC)],
            &t.accounts["provider"],
        )
        .unwrap();

    // Removing COSMOS_USDC should succeed (decreasing weight is always allowed)
    t.contract
        .execute(
            &ExecMsg::ExitPool {
                tokens_out: vec![coin(100_000, COSMOS_USDC)],
            },
            &[],
            &t.accounts["provider"],
        )
        .unwrap();
}

#[test]
fn test_limiter_already_exceeded() {
    let app = OsmosisTestApp::new();
    let admin = app.init_account(&[coin(100_000u128, "uosmo")]).unwrap();

    let t = TestEnvBuilder::new()
        .with_account(
            "provider",
            vec![coin(1_000_000, AXL_USDC), coin(1_000_000, COSMOS_USDC)],
        )
        .with_account("admin", vec![])
        .with_instantiate_msg(InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str(AXL_USDC),
                AssetConfig::from_denom_str(COSMOS_USDC),
            ],
            alloyed_asset_subdenom: "usdc".to_string(),
            alloyed_asset_normalization_factor: Uint128::one(),
            admin: Some(admin.address()),
            moderator: "osmo1cyyzpxplxdzkeea7kwsydadg87357qnahakaks".to_string(),
        })
        .build(&app);

    // Join pool with 50:50 ratio (AXL_USDC is 50% of pool)
    t.contract
        .execute(
            &ExecMsg::JoinPool {},
            &[coin(500_000, AXL_USDC), coin(500_000, COSMOS_USDC)],
            &t.accounts["provider"],
        )
        .unwrap();

    // Register a limiter for AXL_USDC at 40% (already exceeded since pool has 50% AXL_USDC)
    t.contract
        .execute(
            &ExecMsg::RegisterLimiter {
                scope: Scope::Denom(AXL_USDC.to_string()),
                label: "static".to_string(),
                limiter_params: LimiterParams::StaticLimiter {
                    upper_limit: Decimal::percent(40),
                },
            },
            &[],
            &t.accounts["admin"],
        )
        .unwrap();

    // Try to add even 1 AXL_USDC, should fail because already over 40% limit
    let err = t
        .contract
        .execute(
            &ExecMsg::JoinPool {},
            &[coin(1, AXL_USDC)],
            &t.accounts["provider"],
        )
        .unwrap_err();

    assert_contract_err(
        ContractError::UpperLimitExceeded {
            scope: Scope::denom(AXL_USDC),
            upper_limit: Decimal::from_str("0.4").unwrap(),
            value: Decimal::from_str("0.5000004999995").unwrap(), // match contract precision
        },
        err,
    );

    // Adding COSMOS_USDC should succeed (doesn't affect AXL_USDC weight)
    t.contract
        .execute(
            &ExecMsg::JoinPool {},
            &[coin(100_000, COSMOS_USDC)],
            &t.accounts["provider"],
        )
        .unwrap();

    // Removing AXL_USDC should succeed (decreasing weight is always allowed)
    t.contract
        .execute(
            &ExecMsg::ExitPool {
                tokens_out: vec![coin(100_000, AXL_USDC)],
            },
            &[],
            &t.accounts["provider"],
        )
        .unwrap();
}
