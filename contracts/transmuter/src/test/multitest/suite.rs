use std::vec;

use super::{modules::cosmwasm_pool::CosmwasmPool, test_env::*};
use crate::{
    contract::{
        ExecMsg, GetSharesResponse, GetTotalPoolLiquidityResponse, GetTotalSharesResponse,
        InstantiateMsg, IsActiveResponse, QueryMsg,
    },
    sudo::SudoMsg,
    ContractError,
};
use cosmwasm_std::{BankMsg, Coin, Decimal, Uint128};

use cw_multi_test::Executor;

use osmosis_std::types::osmosis::poolmanager::v1beta1::{MsgSwapExactAmountIn, SwapAmountInRoute};
use osmosis_test_tube::{
    cosmrs::proto::cosmos::bank::v1beta1::MsgSend, Account, Bank, Module, OsmosisTestApp,
};

const ETH_USDC: &str = "ibc/AXLETHUSDC";
const ETH_DAI: &str = "ibc/AXLETHDAI";
const COSMOS_USDC: &str = "ibc/COSMOSUSDC";

#[test]
fn test_join_pool() {
    let app = OsmosisTestApp::new();
    let t = TestEnvBuilder::new()
        .with_account(
            "provider_1",
            vec![
                Coin::new(2_000, COSMOS_USDC),
                Coin::new(2_000, ETH_USDC),
                Coin::new(2_000, "urandom"),
            ],
        )
        .with_account(
            "provider_2",
            vec![Coin::new(2_000, COSMOS_USDC), Coin::new(2_000, ETH_USDC)],
        )
        .with_instantiate_msg(InstantiateMsg {
            pool_asset_denoms: vec![ETH_USDC.to_string(), COSMOS_USDC.to_string()],
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
            expected_denom: vec![ETH_USDC.to_string(), COSMOS_USDC.to_string()],
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
        vec![Coin::new(0, ETH_USDC), tokens_in[0].clone()]
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
    let tokens_in = vec![Coin::new(1_000, ETH_USDC), Coin::new(1_000, COSMOS_USDC)];
    t.contract
        .execute(&ExecMsg::JoinPool {}, &tokens_in, &t.accounts["provider_1"])
        .unwrap();

    // check contract balances
    t.assert_contract_balances(&[Coin::new(1_000, ETH_USDC), Coin::new(2_000, COSMOS_USDC)]);

    // check pool balance
    let GetTotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .contract
        .query(&QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![Coin::new(1_000, ETH_USDC), Coin::new(2_000, COSMOS_USDC)]
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
    let tokens_in = vec![Coin::new(2_000, ETH_USDC), Coin::new(2_000, COSMOS_USDC)];
    t.contract
        .execute(&ExecMsg::JoinPool {}, &tokens_in, &t.accounts["provider_2"])
        .unwrap();

    // check contract balances
    t.assert_contract_balances(&[Coin::new(3_000, ETH_USDC), Coin::new(4_000, COSMOS_USDC)]);

    // check pool balance
    let GetTotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .contract
        .query(&QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![Coin::new(3_000, ETH_USDC), Coin::new(4_000, COSMOS_USDC)]
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

    let mut t = TestEnvBuilder::new()
        .with_account(
            "alice",
            vec![
                Coin::new(1_500, ETH_USDC),
                Coin::new(1_000, COSMOS_USDC),
                Coin::new(1_000, "urandom2"),
            ],
        )
        .with_account("bob", vec![Coin::new(29_902, ETH_USDC)])
        .with_account("provider", vec![Coin::new(200_000, COSMOS_USDC)])
        .with_instantiate_msg(InstantiateMsg {
            pool_asset_denoms: vec![ETH_USDC.to_string(), COSMOS_USDC.to_string()],
        })
        .build(&app);

    // join pool
    let tokens_in = vec![Coin::new(100_000, COSMOS_USDC).into()];

    // join pool with send tokens should not update pool balance
    bank.send(
        MsgSend {
            from_address: t.accounts["provider"].address(),
            to_address: t.contract.contract_addr.clone(),
            amount: tokens_in.iter().map(to_proto_coin).collect(),
        },
        &t.accounts["provider"],
    )
    .unwrap();

    let err = cp
        .swap_exact_amount_in(
            MsgSwapExactAmountIn {
                sender: t.accounts["alice"].address(),
                token_in: Some(Coin::new(1_500, ETH_USDC).into()),
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
            expected_denom: vec![ETH_USDC.to_string(), COSMOS_USDC.to_string()],
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
            expected_denom: vec![ETH_USDC.to_string(), COSMOS_USDC.to_string()],
        },
        err,
    );

    // swap with correct token_in should succeed this time
    let token_in = Coin::new(1_500, ETH_USDC);
    cp.swap_exact_amount_in(
        MsgSwapExactAmountIn {
            sender: t.accounts["alice"].address(),
            token_in: Some(token_in.into()),
            routes: vec![SwapAmountInRoute {
                pool_id: t.contract.pool_id,
                token_out_denom: COSMOS_USDC.to_string(),
            }],
            token_out_min_amount: Uint128::from(1_500u128).to_string(),
        },
        &t.accounts["alice"],
    )
    .unwrap();

    // check balances
    t.assert_contract_balances(&[
        Coin::new(1_500, ETH_USDC),
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
            Coin::new(1_500, ETH_USDC),
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
    let token_in = Coin::new(29_902, ETH_USDC);
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
        Coin::new(1_500 + 29_902, ETH_USDC),
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
            Coin::new(1_500 + 29_902, ETH_USDC),
            Coin::new(100_000 - 1_500 - 29_902, COSMOS_USDC)
        ]
    );

    t.assert_account_balances("bob", vec![Coin::new(29_902, COSMOS_USDC)], vec!["uosmo"]);
}

#[test]
fn test_exit_pool() {
    let app = OsmosisTestApp::new();
    let bank = Bank::new(&app);
    let cp = CosmwasmPool::new(&app);

    let t = TestEnvBuilder::new()
        .with_account("user", vec![Coin::new(1_500, ETH_USDC)])
        .with_account("provider_1", vec![Coin::new(100_000, COSMOS_USDC)])
        .with_account("provider_2", vec![Coin::new(100_000, COSMOS_USDC)])
        .with_instantiate_msg(InstantiateMsg {
            pool_asset_denoms: vec![ETH_USDC.to_string(), COSMOS_USDC.to_string()],
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
    let token_in = Coin::new(1_500, ETH_USDC);

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
                tokens_out: vec![Coin::new(1_500, ETH_USDC)],
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
                tokens_out: vec![Coin::new(500, ETH_USDC)],
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
        Coin::new(1500 - 500, ETH_USDC),
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
            Coin::new(1500 - 500, ETH_USDC),
            Coin::new(200_000 - 1500, COSMOS_USDC)
        ]
    );

    // provider can exit pool with any token
    t.contract
        .execute(
            &ExecMsg::ExitPool {
                tokens_out: vec![Coin::new(1_000, ETH_USDC), Coin::new(99_000, COSMOS_USDC)],
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
            Coin::new(0, ETH_USDC),
            Coin::new(200_000 - 1500 - 99_000, COSMOS_USDC)
        ]
    );

    // exit pool with excess shares fails
    let err = t
        .contract
        .execute(
            &ExecMsg::ExitPool {
                tokens_out: vec![Coin::new(1, ETH_USDC)],
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
                tokens_out: vec![Coin::new(1, ETH_USDC)],
            },
            &[],
            &t.accounts["provider_1"],
        )
        .unwrap_err();

    assert_contract_err(
        ContractError::InsufficientPoolAsset {
            required: Coin::new(1, ETH_USDC),
            available: Coin::new(0, ETH_USDC),
        },
        err,
    );
}

#[test]
fn test_3_pool_swap() {
    let mut t = TestEnvBuilder::new()
        .with_account("alice", vec![Coin::new(1_500, ETH_USDC)])
        .with_account("bob", vec![Coin::new(1_500, ETH_DAI)])
        .with_account("provider", vec![Coin::new(100_000, COSMOS_USDC)])
        .with_instantiate_msg(InstantiateMsg {
            pool_asset_denoms: vec![
                ETH_USDC.to_string(),
                ETH_DAI.to_string(),
                COSMOS_USDC.to_string(),
            ],
        })
        ._build();

    // join pool
    t.app
        .execute_contract(
            t.accounts["provider"].clone(),
            t.contract.clone(),
            &ExecMsg::JoinPool {},
            &[Coin::new(100_000, COSMOS_USDC)],
        )
        .unwrap();

    // check shares
    let GetSharesResponse { shares } = t
        .app
        .wrap()
        .query_wasm_smart(
            t.contract.clone(),
            &QueryMsg::GetShares {
                address: t.accounts["provider"].to_string(),
            },
        )
        .unwrap();

    assert_eq!(shares, Uint128::new(100_000));

    // check contract balances
    assert_eq!(
        t.app.wrap().query_all_balances(&t.contract).unwrap(),
        vec![Coin::new(100_000, COSMOS_USDC)]
    );

    // check provider balances
    assert_eq!(
        t.app
            .wrap()
            .query_all_balances(&t.accounts["provider"])
            .unwrap(),
        vec![]
    );

    // check pool
    let GetTotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .app
        .wrap()
        .query_wasm_smart(t.contract.clone(), &QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![
            Coin::new(0, ETH_USDC),
            Coin::new(0, ETH_DAI),
            Coin::new(100_000, COSMOS_USDC)
        ]
    );

    // swap ETH_USDC to ETH_DAI should fail
    let err = t
        .app
        .wasm_sudo(
            t.contract.clone(),
            &SudoMsg::SwapExactAmountIn {
                sender: t.accounts["alice"].to_string(),
                token_in: Coin::new(1_000, ETH_USDC),
                token_out_denom: ETH_DAI.to_string(),
                token_out_min_amount: Uint128::from(1_000u128),
                swap_fee: Decimal::zero(),
            },
        )
        .unwrap_err();

    assert_eq!(
        err.downcast_ref::<ContractError>().unwrap(),
        &ContractError::InsufficientPoolAsset {
            required: Coin::new(1_000, ETH_DAI),
            available: Coin::new(0, ETH_DAI),
        }
    );

    // swap ETH_USDC to COSMOS_USDC

    t.app
        .execute(
            t.accounts["alice"].clone(),
            BankMsg::Send {
                to_address: t.contract.to_string(),
                amount: vec![Coin::new(1_000, ETH_USDC)],
            }
            .into(),
        )
        .unwrap();

    t.app
        .wasm_sudo(
            t.contract.clone(),
            &SudoMsg::SwapExactAmountIn {
                sender: t.accounts["alice"].to_string(),
                token_in: Coin::new(1_000, ETH_USDC),
                token_out_denom: COSMOS_USDC.to_string(),
                token_out_min_amount: Uint128::from(1_000u128),
                swap_fee: Decimal::zero(),
            },
        )
        .unwrap();

    // check contract balances
    assert_eq!(
        t.app.wrap().query_all_balances(&t.contract).unwrap(),
        vec![Coin::new(1_000, ETH_USDC), Coin::new(99_000, COSMOS_USDC)]
    );

    // check alice balance
    assert_eq!(
        t.app
            .wrap()
            .query_all_balances(&t.accounts["alice"])
            .unwrap(),
        vec![Coin::new(500, ETH_USDC), Coin::new(1_000, COSMOS_USDC)]
    );

    // check pool
    let GetTotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .app
        .wrap()
        .query_wasm_smart(t.contract.clone(), &QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![
            Coin::new(1_000, ETH_USDC),
            Coin::new(0, ETH_DAI),
            Coin::new(99_000, COSMOS_USDC)
        ]
    );

    // swap ETH_DAI to ETH_USDC

    // bank send
    t.app
        .execute(
            t.accounts["bob"].clone(),
            BankMsg::Send {
                to_address: t.contract.to_string(),
                amount: vec![Coin::new(1_000, ETH_DAI)],
            }
            .into(),
        )
        .unwrap();

    t.app
        .wasm_sudo(
            t.contract.clone(),
            &SudoMsg::SwapExactAmountIn {
                sender: t.accounts["bob"].to_string(),
                token_in: Coin::new(1_000, ETH_DAI),
                token_out_denom: ETH_USDC.to_string(),
                token_out_min_amount: Uint128::from(1_000u128),
                swap_fee: Decimal::zero(),
            },
        )
        .unwrap();

    // check contract balances
    assert_eq!(
        t.app.wrap().query_all_balances(&t.contract).unwrap(),
        vec![Coin::new(1_000, ETH_DAI), Coin::new(99_000, COSMOS_USDC),]
    );

    // check bob balances
    assert_eq!(
        t.app.wrap().query_all_balances(&t.accounts["bob"]).unwrap(),
        vec![Coin::new(500, ETH_DAI), Coin::new(1_000, ETH_USDC)]
    );

    // check pool
    let GetTotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .app
        .wrap()
        .query_wasm_smart(t.contract.clone(), &QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![
            Coin::new(0, ETH_USDC),
            Coin::new(1_000, ETH_DAI),
            Coin::new(99_000, COSMOS_USDC)
        ]
    );

    // provider exit pool
    t.app
        .execute_contract(
            t.accounts["provider"].clone(),
            t.contract.clone(),
            &ExecMsg::ExitPool {
                tokens_out: vec![Coin::new(1_000, ETH_DAI), Coin::new(99_000, COSMOS_USDC)],
            },
            &[],
        )
        .unwrap();

    // check balances
    assert_eq!(
        t.app.wrap().query_all_balances(&t.contract).unwrap(),
        vec![]
    );

    // check pool
    let GetTotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .app
        .wrap()
        .query_wasm_smart(t.contract.clone(), &QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![
            Coin::new(0, ETH_USDC),
            Coin::new(0, ETH_DAI),
            Coin::new(0, COSMOS_USDC)
        ]
    );

    // check contract balances
    assert_eq!(
        t.app.wrap().query_all_balances(&t.contract).unwrap(),
        vec![]
    );

    // check provider balances
    assert_eq!(
        t.app
            .wrap()
            .query_all_balances(&t.accounts["provider"])
            .unwrap(),
        vec![Coin::new(1_000, ETH_DAI), Coin::new(99_000, COSMOS_USDC)]
    );
}

#[test]
#[ignore = "active status sudo is not implemented on cosmwasmpool"]
fn test_active_status() {
    let mut t = TestEnvBuilder::new()
        .with_account("user", vec![Coin::new(1_000, ETH_USDC)])
        .with_account("provider", vec![Coin::new(100_000, COSMOS_USDC)])
        .with_instantiate_msg(InstantiateMsg {
            pool_asset_denoms: vec![ETH_USDC.to_string(), COSMOS_USDC.to_string()],
        })
        ._build();

    // check status
    let IsActiveResponse { is_active } = t
        .app
        .wrap()
        .query_wasm_smart(t.contract.clone(), &QueryMsg::IsActive {})
        .unwrap();

    assert!(is_active); // active

    // execute should work
    t.app
        .execute_contract(
            t.accounts["provider"].clone(),
            t.contract.clone(),
            &ExecMsg::JoinPool {},
            &[Coin::new(50_000, COSMOS_USDC)],
        )
        .unwrap();

    // sudo should work
    t.app
        .wasm_sudo(
            t.contract.clone(),
            &SudoMsg::SwapExactAmountIn {
                sender: t.accounts["user"].to_string(),
                token_in: Coin::new(500, ETH_USDC),
                token_out_denom: COSMOS_USDC.to_string(),
                token_out_min_amount: Uint128::from(500u128),
                swap_fee: Decimal::zero(),
            },
        )
        .unwrap();

    // deactivate
    t.app
        .wasm_sudo(t.contract.clone(), &SudoMsg::SetActive { is_active: false })
        .unwrap();

    // check status
    let IsActiveResponse { is_active } = t
        .app
        .wrap()
        .query_wasm_smart(t.contract.clone(), &QueryMsg::IsActive {})
        .unwrap();

    assert!(!is_active); // inactive

    // execute shoud not work
    let err = t
        .app
        .execute_contract(
            t.accounts["provider"].clone(),
            t.contract.clone(),
            &ExecMsg::JoinPool {},
            &[Coin::new(50_000, COSMOS_USDC)],
        )
        .unwrap_err();

    assert_eq!(
        err.downcast_ref::<ContractError>().unwrap(),
        &ContractError::InactivePool {}
    );

    // sudo should not work
    let err = t
        .app
        .wasm_sudo(
            t.contract.clone(),
            &SudoMsg::SwapExactAmountIn {
                sender: t.accounts["user"].to_string(),
                token_in: Coin::new(500, ETH_USDC),
                token_out_denom: COSMOS_USDC.to_string(),
                token_out_min_amount: Uint128::from(500u128),
                swap_fee: Decimal::zero(),
            },
        )
        .unwrap_err();

    assert_eq!(
        err.downcast_ref::<ContractError>().unwrap(),
        &ContractError::InactivePool {}
    );

    // reactivate
    t.app
        .wasm_sudo(t.contract.clone(), &SudoMsg::SetActive { is_active: true })
        .unwrap();

    // check status
    let IsActiveResponse { is_active } = t
        .app
        .wrap()
        .query_wasm_smart(t.contract.clone(), &QueryMsg::IsActive {})
        .unwrap();

    assert!(is_active); // active
}
