use std::vec;

use super::test_env::*;
use crate::{
    contract::{
        ExecMsg, InstantiateMsg, IsActiveResponse, QueryMsg, SharesResponse,
        TotalPoolLiquidityResponse, TotalSharesResponse,
    },
    sudo::SudoMsg,
    ContractError,
};
use cosmwasm_std::{Addr, BankMsg, Coin, Decimal, Uint128};
use cw_multi_test::Executor;

const ETH_USDC: &str = "ibc/AXLETHUSDC";
const ETH_DAI: &str = "ibc/AXLETHDAI";
const COSMOS_USDC: &str = "ibc/COSMOSUSDC";

#[test]
fn test_join_pool() {
    let mut t = TestEnvBuilder::new()
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
        .build();

    // failed to join pool with 0 denom
    let err = t
        .app
        .execute_contract(
            t.accounts["provider_1"].clone(),
            t.contract.clone(),
            &ExecMsg::JoinPool {},
            &[],
        )
        .unwrap_err();

    assert_eq!(
        err.downcast_ref::<ContractError>().unwrap(),
        &ContractError::AtLeastSingleTokenExpected {}
    );

    // fail to join pool with denom that is not in the pool
    let tokens_in = vec![Coin::new(1_000, "urandom")];
    let err = t
        .app
        .execute_contract(
            t.accounts["provider_1"].clone(),
            t.contract.clone(),
            &ExecMsg::JoinPool {},
            &tokens_in,
        )
        .unwrap_err();

    assert_eq!(
        err.downcast_ref::<ContractError>().unwrap(),
        &ContractError::InvalidJoinPoolDenom {
            denom: "urandom".to_string(),
            expected_denom: vec![ETH_USDC.to_string(), COSMOS_USDC.to_string()]
        }
    );

    // join pool with correct pool's denom should added to the contract's balance and update state
    let tokens_in = vec![Coin::new(1_000, COSMOS_USDC)];
    t.app
        .execute_contract(
            t.accounts["provider_1"].clone(),
            t.contract.clone(),
            &ExecMsg::JoinPool {},
            &tokens_in,
        )
        .unwrap();

    // check contract balances
    let contract_balances = t.app.wrap().query_all_balances(&t.contract).unwrap();
    assert_eq!(contract_balances, tokens_in);

    // check pool balance
    let TotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .app
        .wrap()
        .query_wasm_smart(t.contract.clone(), &QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![Coin::new(0, ETH_USDC), tokens_in[0].clone()]
    );
    // check shares
    let SharesResponse { shares } = t
        .app
        .wrap()
        .query_wasm_smart(
            t.contract.clone(),
            &QueryMsg::GetShares {
                address: t.accounts["provider_1"].to_string(),
            },
        )
        .unwrap();

    assert_eq!(shares, tokens_in[0].amount);

    // check total shares
    let TotalSharesResponse { total_shares } = t
        .app
        .wrap()
        .query_wasm_smart(t.contract.clone(), &QueryMsg::GetTotalShares {})
        .unwrap();

    assert_eq!(total_shares, tokens_in[0].amount);

    // join pool with multiple correct pool's denom should added to the contract's balance and update state
    let tokens_in = vec![Coin::new(1_000, ETH_USDC), Coin::new(1_000, COSMOS_USDC)];
    t.app
        .execute_contract(
            t.accounts["provider_1"].clone(),
            t.contract.clone(),
            &ExecMsg::JoinPool {},
            &tokens_in,
        )
        .unwrap();

    // check contract balances
    let contract_balances = t.app.wrap().query_all_balances(&t.contract).unwrap();
    assert_eq!(
        contract_balances,
        vec![Coin::new(1_000, ETH_USDC), Coin::new(2_000, COSMOS_USDC)]
    );

    // check pool balance
    let TotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .app
        .wrap()
        .query_wasm_smart(t.contract.clone(), &QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![Coin::new(1_000, ETH_USDC), Coin::new(2_000, COSMOS_USDC)]
    );
    // check shares
    let SharesResponse { shares } = t
        .app
        .wrap()
        .query_wasm_smart(
            t.contract.clone(),
            &QueryMsg::GetShares {
                address: t.accounts["provider_1"].to_string(),
            },
        )
        .unwrap();

    assert_eq!(shares, Uint128::new(3000));

    // check total shares
    let TotalSharesResponse { total_shares } = t
        .app
        .wrap()
        .query_wasm_smart(t.contract.clone(), &QueryMsg::GetTotalShares {})
        .unwrap();

    assert_eq!(total_shares, Uint128::new(3000));

    // join pool with another provider with multiple correct pool's denom should added to the contract's balance and update state
    let tokens_in = vec![Coin::new(2_000, ETH_USDC), Coin::new(2_000, COSMOS_USDC)];
    t.app
        .execute_contract(
            t.accounts["provider_2"].clone(),
            t.contract.clone(),
            &ExecMsg::JoinPool {},
            &tokens_in,
        )
        .unwrap();

    // check contract balances
    let contract_balances = t.app.wrap().query_all_balances(&t.contract).unwrap();
    assert_eq!(
        contract_balances,
        vec![Coin::new(3_000, ETH_USDC), Coin::new(4_000, COSMOS_USDC)]
    );

    // check pool balance
    let TotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .app
        .wrap()
        .query_wasm_smart(t.contract.clone(), &QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![Coin::new(3_000, ETH_USDC), Coin::new(4_000, COSMOS_USDC)]
    );

    // check shares
    let SharesResponse { shares } = t
        .app
        .wrap()
        .query_wasm_smart(
            t.contract.clone(),
            &QueryMsg::GetShares {
                address: t.accounts["provider_2"].to_string(),
            },
        )
        .unwrap();

    assert_eq!(shares, Uint128::new(4000));

    // check total shares
    let TotalSharesResponse { total_shares } = t
        .app
        .wrap()
        .query_wasm_smart(t.contract.clone(), &QueryMsg::GetTotalShares {})
        .unwrap();

    assert_eq!(total_shares, Uint128::new(7000));
}

#[test]
fn test_swap() {
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
        .build();

    // join pool
    let tokens_in = vec![Coin::new(100_000, COSMOS_USDC)];

    // join pool with send tokens should not update pool balance
    t.app
        .send_tokens(
            t.accounts["provider"].clone(),
            t.contract.clone(),
            &tokens_in,
        )
        .unwrap();

    // transmute should fail since there has no token_out_denom remaining in the pool
    let err = t
        .app
        .wasm_sudo(
            t.contract.clone(),
            &SudoMsg::SwapExactAmountIn {
                sender: t.accounts["alice"].clone().into(),
                token_in: Coin::new(1_500, ETH_USDC),
                token_out_denom: COSMOS_USDC.to_string(),
                token_out_min_amount: Uint128::from(1500u128),
                swap_fee: Decimal::zero(),
            },
        )
        .unwrap_err();

    assert_eq!(
        err.downcast_ref::<ContractError>().unwrap(),
        &ContractError::InsufficientPoolAsset {
            required: Coin::new(1_500, COSMOS_USDC),
            available: Coin::new(0, COSMOS_USDC)
        }
    );

    // join pool properly
    t.app
        .execute_contract(
            t.accounts["provider"].clone(),
            t.contract.clone(),
            &ExecMsg::JoinPool {},
            &tokens_in,
        )
        .unwrap();

    // transmute with incorrect funds should still fail
    let err = t
        .app
        .wasm_sudo(
            t.contract.clone(),
            &SudoMsg::SwapExactAmountIn {
                sender: t.accounts["alice"].clone().into(),
                token_in: Coin::new(1_000, COSMOS_USDC),
                token_out_denom: "urandom".to_string(),
                token_out_min_amount: Uint128::from(1_000u128),
                swap_fee: Decimal::zero(),
            },
        )
        .unwrap_err();

    assert_eq!(
        err.downcast_ref::<ContractError>().unwrap(),
        &ContractError::InvalidTransmuteDenom {
            denom: "urandom".to_string(),
            expected_denom: vec![ETH_USDC.to_string(), COSMOS_USDC.to_string()]
        }
    );

    let err = t
        .app
        .wasm_sudo(
            t.contract.clone(),
            &SudoMsg::SwapExactAmountIn {
                sender: t.accounts["alice"].clone().into(),
                token_in: Coin::new(1_000, COSMOS_USDC),
                token_out_denom: "urandom2".to_string(),
                token_out_min_amount: Uint128::from(1_000u128),
                swap_fee: Decimal::zero(),
            },
        )
        .unwrap_err();

    assert_eq!(
        err.downcast_ref::<ContractError>().unwrap(),
        &ContractError::InvalidTransmuteDenom {
            denom: "urandom2".to_string(),
            expected_denom: vec![ETH_USDC.to_string(), COSMOS_USDC.to_string()]
        }
    );

    // bank send before sudo

    let token_in = Coin::new(1_500, ETH_USDC);

    t.app
        .execute(
            t.accounts["alice"].clone(),
            BankMsg::Send {
                to_address: t.contract.to_string(),
                amount: vec![token_in.clone()],
            }
            .into(),
        )
        .unwrap();

    // swap with correct token_in should succeed this time
    t.app
        .wasm_sudo(
            t.contract.clone(),
            &SudoMsg::SwapExactAmountIn {
                sender: t.accounts["alice"].to_string(),
                token_in,
                token_out_denom: COSMOS_USDC.to_string(),
                token_out_min_amount: Uint128::from(1_500u128),
                swap_fee: Decimal::zero(),
            },
        )
        .unwrap();

    // check balances
    let contract_balances = t.app.wrap().query_all_balances(&t.contract).unwrap();
    let TotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .app
        .wrap()
        .query_wasm_smart(&t.contract, &QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();
    let alice_balances = t
        .app
        .wrap()
        .query_all_balances(&t.accounts["alice"])
        .unwrap();

    assert_eq!(
        contract_balances,
        vec![
            Coin::new(1_500, ETH_USDC),
            Coin::new(100_000 + 100_000 - 1_500, COSMOS_USDC), // +100_000 due to bank send
        ]
    );

    assert_eq!(
        total_pool_liquidity,
        vec![
            Coin::new(1_500, ETH_USDC),
            Coin::new(100_000 - 1_500, COSMOS_USDC)
        ]
    );

    // +1_000 due to existing alice balance
    assert_eq!(
        alice_balances,
        vec![
            Coin::new(1_500 + 1_000, COSMOS_USDC),
            Coin::new(1_000, "urandom2")
        ]
    );

    // // swap again with another user

    let token_in = Coin::new(29_902, ETH_USDC);

    t.app
        .execute(
            t.accounts["bob"].clone(),
            BankMsg::Send {
                to_address: t.contract.to_string(),
                amount: vec![token_in.clone()],
            }
            .into(),
        )
        .unwrap();

    // swap with correct token_in should succeed this time
    t.app
        .wasm_sudo(
            t.contract.clone(),
            &SudoMsg::SwapExactAmountIn {
                sender: t.accounts["bob"].to_string(),
                token_in,
                token_out_denom: COSMOS_USDC.to_string(),
                token_out_min_amount: Uint128::from(29_902u128),
                swap_fee: Decimal::zero(),
            },
        )
        .unwrap();

    // check balances
    let contract_balances = t.app.wrap().query_all_balances(&t.contract).unwrap();
    let TotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .app
        .wrap()
        .query_wasm_smart(t.contract, &QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();
    let bob_balances = t.app.wrap().query_all_balances(&t.accounts["bob"]).unwrap();

    assert_eq!(
        contract_balances,
        vec![
            Coin::new(1_500 + 29_902, ETH_USDC),
            Coin::new(100_000 + 100_000 - 1_500 - 29_902, COSMOS_USDC), // +100_000 due to bank send
        ]
    );

    assert_eq!(
        total_pool_liquidity,
        vec![
            Coin::new(1_500 + 29_902, ETH_USDC),
            Coin::new(100_000 - 1_500 - 29_902, COSMOS_USDC)
        ]
    );

    assert_eq!(bob_balances, vec![Coin::new(29_902, COSMOS_USDC)]);
}

#[test]
fn test_exit_pool() {
    let mut t = TestEnvBuilder::new()
        .with_account("user", vec![Coin::new(1_500, ETH_USDC)])
        .with_account("provider_1", vec![Coin::new(100_000, COSMOS_USDC)])
        .with_account("provider_2", vec![Coin::new(100_000, COSMOS_USDC)])
        .with_instantiate_msg(InstantiateMsg {
            pool_asset_denoms: vec![ETH_USDC.to_string(), COSMOS_USDC.to_string()],
        })
        .build();

    // join pool
    t.app
        .execute_contract(
            t.accounts["provider_1"].clone(),
            t.contract.clone(),
            &ExecMsg::JoinPool {},
            &[Coin::new(100_000, COSMOS_USDC)],
        )
        .unwrap();

    t.app
        .execute_contract(
            t.accounts["provider_2"].clone(),
            t.contract.clone(),
            &ExecMsg::JoinPool {},
            &[Coin::new(100_000, COSMOS_USDC)],
        )
        .unwrap();

    // swap to build up token_in
    let token_in = Coin::new(1_500, ETH_USDC);

    t.app
        .execute(
            t.accounts["user"].clone(),
            BankMsg::Send {
                to_address: t.contract.to_string(),
                amount: vec![token_in.clone()],
            }
            .into(),
        )
        .unwrap();

    t.app
        .wasm_sudo(
            t.contract.clone(),
            &SudoMsg::SwapExactAmountIn {
                sender: t.accounts["user"].to_string(),
                token_in,
                token_out_denom: COSMOS_USDC.to_string(),
                token_out_min_amount: Uint128::from(1_500u128),
                swap_fee: Decimal::zero(),
            },
        )
        .unwrap();

    // non-provider cannot exit_pool
    let err = t
        .app
        .execute_contract(
            t.accounts["user"].clone(),
            t.contract.clone(),
            &ExecMsg::ExitPool {
                tokens_out: vec![Coin::new(1_500, ETH_USDC)],
            },
            &[],
        )
        .unwrap_err();

    assert_eq!(
        err.downcast_ref::<ContractError>().unwrap(),
        &ContractError::InsufficientShares {
            required: 1500u128.into(),
            available: Uint128::zero()
        }
    );

    // provider can exit pool
    t.app
        .execute_contract(
            t.accounts["provider_1"].clone(),
            t.contract.clone(),
            &ExecMsg::ExitPool {
                tokens_out: vec![Coin::new(500, ETH_USDC)],
            },
            &[],
        )
        .unwrap();

    // check shares
    let SharesResponse { shares } = t
        .app
        .wrap()
        .query_wasm_smart(
            t.contract.clone(),
            &QueryMsg::GetShares {
                address: t.accounts["provider_1"].to_string(),
            },
        )
        .unwrap();

    assert_eq!(shares, Uint128::new(100_000 - 500));

    // check total shares
    let TotalSharesResponse { total_shares } = t
        .app
        .wrap()
        .query_wasm_smart(t.contract.clone(), &QueryMsg::GetTotalShares {})
        .unwrap();

    assert_eq!(total_shares, Uint128::new(200_000 - 500));

    // check balances
    assert_eq!(
        t.app.wrap().query_all_balances(&t.contract).unwrap(),
        vec![
            Coin::new(1500 - 500, ETH_USDC),
            Coin::new(200_000 - 1500, COSMOS_USDC)
        ]
    );

    let TotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .app
        .wrap()
        .query_wasm_smart(t.contract.clone(), &QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![
            Coin::new(1500 - 500, ETH_USDC),
            Coin::new(200_000 - 1500, COSMOS_USDC)
        ]
    );

    // provider can exit pool with any token
    t.app
        .execute_contract(
            t.accounts["provider_2"].clone(),
            t.contract.clone(),
            &ExecMsg::ExitPool {
                tokens_out: vec![Coin::new(1_000, ETH_USDC), Coin::new(99_000, COSMOS_USDC)],
            },
            &[],
        )
        .unwrap();

    // check shares
    let SharesResponse { shares } = t
        .app
        .wrap()
        .query_wasm_smart(
            t.contract.clone(),
            &QueryMsg::GetShares {
                address: t.accounts["provider_2"].to_string(),
            },
        )
        .unwrap();

    assert_eq!(shares, Uint128::new(0));

    // check total shares
    let TotalSharesResponse { total_shares } = t
        .app
        .wrap()
        .query_wasm_smart(t.contract.clone(), &QueryMsg::GetTotalShares {})
        .unwrap();

    assert_eq!(total_shares, Uint128::new(200_000 - 500 - 1000 - 99_000));

    // check balances
    assert_eq!(
        t.app.wrap().query_all_balances(&t.contract).unwrap(),
        vec![Coin::new(200_000 - 1500 - 99_000, COSMOS_USDC)]
    );

    let TotalPoolLiquidityResponse {
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
            Coin::new(200_000 - 1500 - 99_000, COSMOS_USDC)
        ]
    );

    // exit pool with excess shares fails
    let err = t
        .app
        .execute_contract(
            Addr::unchecked("provider_2"),
            t.contract.clone(),
            &ExecMsg::ExitPool {
                tokens_out: vec![Coin::new(1, ETH_USDC)],
            },
            &[],
        )
        .unwrap_err();

    assert_eq!(
        err.downcast_ref::<ContractError>().unwrap(),
        &ContractError::InsufficientShares {
            required: Uint128::one(),
            available: Uint128::zero()
        }
    );

    // has remaining shares but no coins on the requested side
    let err = t
        .app
        .execute_contract(
            Addr::unchecked("provider_1"),
            t.contract.clone(),
            &ExecMsg::ExitPool {
                tokens_out: vec![Coin::new(1, ETH_USDC), Coin::new(1, COSMOS_USDC)],
            },
            &[],
        )
        .unwrap_err();

    assert_eq!(
        err.downcast_ref::<ContractError>().unwrap(),
        &ContractError::InsufficientPoolAsset {
            required: Coin::new(1, ETH_USDC),
            available: Coin::new(0, ETH_USDC)
        }
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
        .build();

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
    let SharesResponse { shares } = t
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
    let TotalPoolLiquidityResponse {
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
    let TotalPoolLiquidityResponse {
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
    let TotalPoolLiquidityResponse {
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
    let TotalPoolLiquidityResponse {
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
fn test_active_status() {
    let mut t = TestEnvBuilder::new()
        .with_account("user", vec![Coin::new(1_000, ETH_USDC)])
        .with_account("provider", vec![Coin::new(100_000, COSMOS_USDC)])
        .with_instantiate_msg(InstantiateMsg {
            pool_asset_denoms: vec![ETH_USDC.to_string(), COSMOS_USDC.to_string()],
        })
        .build();

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
