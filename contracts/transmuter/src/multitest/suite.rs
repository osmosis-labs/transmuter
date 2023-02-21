use std::vec;

use super::test_env::*;
use crate::{
    contract::{ExecMsg, InstantiateMsg, PoolResponse, QueryMsg, SharesResponse},
    transmuter_pool::TransmuterPool,
    ContractError,
};
use cosmwasm_std::{Addr, Coin, Uint128};
use cw_multi_test::Executor;

const ETH_USDC: &str = "ibc/AXLETHUSDC";
const COSMOS_USDC: &str = "ibc/COSMOSUSDC";

#[test]
fn test_join_pool() {
    let mut t = TestEnvBuilder::new()
        .with_account(
            "provider",
            vec![
                Coin::new(2_000, COSMOS_USDC),
                Coin::new(2_000, ETH_USDC),
                Coin::new(2_000, "urandom"),
            ],
        )
        .with_instantiate_msg(InstantiateMsg {
            in_denom: ETH_USDC.to_string(),
            out_denom: COSMOS_USDC.to_string(),
        })
        .build();

    // failed to join pool with 0 denom
    let err = t
        .app
        .execute_contract(
            t.accounts["provider"].clone(),
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
            t.accounts["provider"].clone(),
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
            t.accounts["provider"].clone(),
            t.contract.clone(),
            &ExecMsg::JoinPool {},
            &tokens_in,
        )
        .unwrap();

    // check contract balances
    let contract_balances = t.app.wrap().query_all_balances(&t.contract).unwrap();
    assert_eq!(contract_balances, tokens_in);

    // check pool balance
    let PoolResponse { pool } = t
        .app
        .wrap()
        .query_wasm_smart(t.contract.clone(), &QueryMsg::Pool {})
        .unwrap();

    assert_eq!(
        pool,
        TransmuterPool {
            pool_assets: vec![Coin::new(0, ETH_USDC), tokens_in[0].clone()]
        }
    );

    // check shares
    let SharesResponse { shares } = t
        .app
        .wrap()
        .query_wasm_smart(
            t.contract.clone(),
            &QueryMsg::Shares {
                address: t.accounts["provider"].to_string(),
            },
        )
        .unwrap();

    assert_eq!(shares, tokens_in[0].amount);

    // join pool with multiple correct pool's denom should added to the contract's balance and update state
    let tokens_in = vec![Coin::new(1_000, ETH_USDC), Coin::new(1_000, COSMOS_USDC)];
    t.app
        .execute_contract(
            t.accounts["provider"].clone(),
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
    let PoolResponse { pool } = t
        .app
        .wrap()
        .query_wasm_smart(t.contract.clone(), &QueryMsg::Pool {})
        .unwrap();

    assert_eq!(
        pool,
        TransmuterPool {
            pool_assets: vec![Coin::new(1_000, ETH_USDC), Coin::new(2_000, COSMOS_USDC)]
        }
    );

    // check shares
    let SharesResponse { shares } = t
        .app
        .wrap()
        .query_wasm_smart(
            t.contract.clone(),
            &QueryMsg::Shares {
                address: t.accounts["provider"].to_string(),
            },
        )
        .unwrap();

    assert_eq!(shares, Uint128::new(3000));
}

#[test]
fn test_transmute() {
    let mut t = TestEnvBuilder::new()
        .with_account(
            "alice",
            vec![Coin::new(1_500, ETH_USDC), Coin::new(1_000, COSMOS_USDC)],
        )
        .with_account("bob", vec![Coin::new(29_902, ETH_USDC)])
        .with_account("provider", vec![Coin::new(200_000, COSMOS_USDC)])
        .with_instantiate_msg(InstantiateMsg {
            in_denom: ETH_USDC.to_string(),
            out_denom: COSMOS_USDC.to_string(),
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

    // transmute should fail since there has no out_coin_reserve in the pool
    let err = t
        .app
        .execute_contract(
            t.accounts["alice"].clone(),
            t.contract.clone(),
            &ExecMsg::Transmute {},
            &[Coin::new(1_500, ETH_USDC)],
        )
        .unwrap_err();

    assert_eq!(
        err.downcast_ref::<ContractError>().unwrap(),
        &ContractError::InsufficientFund {
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
        .execute_contract(
            t.accounts["alice"].clone(),
            t.contract.clone(),
            &ExecMsg::Transmute {},
            &[Coin::new(1_000, COSMOS_USDC)],
        )
        .unwrap_err();

    assert_eq!(
        err.downcast_ref::<ContractError>().unwrap(),
        &ContractError::InvalidTransmuteDenom {
            denom: COSMOS_USDC.to_string(),
            expected_denom: ETH_USDC.to_string()
        }
    );

    // transmute with funds length != 1 should fail
    let err = t
        .app
        .execute_contract(
            t.accounts["alice"].clone(),
            t.contract.clone(),
            &ExecMsg::Transmute {},
            &[Coin::new(1_000, ETH_USDC), Coin::new(1_000, COSMOS_USDC)],
        )
        .unwrap_err();

    assert_eq!(
        err.downcast_ref::<ContractError>().unwrap(),
        &ContractError::SingleTokenExpected {}
    );

    // transmute with correct in_coin should succeed this time
    t.app
        .execute_contract(
            t.accounts["alice"].clone(),
            t.contract.clone(),
            &ExecMsg::Transmute {},
            &[Coin::new(1_500, ETH_USDC)],
        )
        .unwrap();

    // check balances
    let contract_balances = t.app.wrap().query_all_balances(&t.contract).unwrap();
    let PoolResponse { pool } = t
        .app
        .wrap()
        .query_wasm_smart(&t.contract, &QueryMsg::Pool {})
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
        pool,
        TransmuterPool {
            pool_assets: vec![
                Coin::new(1_500, ETH_USDC),
                Coin::new(100_000 - 1_500, COSMOS_USDC)
            ]
        }
    );

    // +1_000 due to existing alice balance
    assert_eq!(alice_balances, vec![Coin::new(1_500 + 1_000, COSMOS_USDC)]);

    // transmute again with another user
    t.app
        .execute_contract(
            t.accounts["bob"].clone(),
            t.contract.clone(),
            &ExecMsg::Transmute {},
            &[Coin::new(29_902, ETH_USDC)],
        )
        .unwrap();

    // check balances
    let contract_balances = t.app.wrap().query_all_balances(&t.contract).unwrap();
    let PoolResponse { pool } = t
        .app
        .wrap()
        .query_wasm_smart(t.contract, &QueryMsg::Pool {})
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
        pool,
        TransmuterPool {
            pool_assets: vec![
                Coin::new(1_500 + 29_902, ETH_USDC),
                Coin::new(100_000 - 1_500 - 29_902, COSMOS_USDC)
            ]
        }
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
            in_denom: ETH_USDC.to_string(),
            out_denom: COSMOS_USDC.to_string(),
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

    // transmute to build up some in_coin
    t.app
        .execute_contract(
            t.accounts["user"].clone(),
            t.contract.clone(),
            &ExecMsg::Transmute {},
            &[Coin::new(1_500, ETH_USDC)],
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
            &QueryMsg::Shares {
                address: t.accounts["provider_1"].to_string(),
            },
        )
        .unwrap();

    assert_eq!(shares, Uint128::new(100_000 - 500));

    // check balances
    assert_eq!(
        t.app.wrap().query_all_balances(&t.contract).unwrap(),
        vec![
            Coin::new(1500 - 500, ETH_USDC),
            Coin::new(200_000 - 1500, COSMOS_USDC)
        ]
    );

    let PoolResponse { pool } = t
        .app
        .wrap()
        .query_wasm_smart(t.contract.clone(), &QueryMsg::Pool {})
        .unwrap();

    assert_eq!(
        pool,
        TransmuterPool {
            pool_assets: vec![
                Coin::new(1500 - 500, ETH_USDC),
                Coin::new(200_000 - 1500, COSMOS_USDC)
            ]
        }
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
            &QueryMsg::Shares {
                address: t.accounts["provider_2"].to_string(),
            },
        )
        .unwrap();

    assert_eq!(shares, Uint128::new(0));

    // check balances
    assert_eq!(
        t.app.wrap().query_all_balances(&t.contract).unwrap(),
        vec![Coin::new(200_000 - 1500 - 99_000, COSMOS_USDC)]
    );

    let PoolResponse { pool } = t
        .app
        .wrap()
        .query_wasm_smart(t.contract.clone(), &QueryMsg::Pool {})
        .unwrap();

    assert_eq!(
        pool,
        TransmuterPool {
            pool_assets: vec![
                Coin::new(0, ETH_USDC),
                Coin::new(200_000 - 1500 - 99_000, COSMOS_USDC)
            ]
        }
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
        &ContractError::InsufficientFund {
            required: Coin::new(1, ETH_USDC),
            available: Coin::new(0, ETH_USDC)
        }
    );
}
