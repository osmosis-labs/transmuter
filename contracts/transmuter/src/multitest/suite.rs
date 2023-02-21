use std::vec;

use super::test_env::*;
use crate::{
    contract::{ExecMsg, InstantiateMsg, PoolResponse, QueryMsg, SharesResponse},
    transmuter_pool::TransmuterPool,
    ContractError,
};
use cosmwasm_std::{Addr, Coin, Uint128};
use cw_controllers::AdminResponse;
use cw_multi_test::Executor;

const ETH_USDC: &str = "ibc/AXLETHUSDC";
const COSMOS_USDC: &str = "ibc/COSMOSUSDC";

#[test]
fn test_supply() {
    let mut t = TestEnvBuilder::new()
        .with_account(
            "provider",
            vec![Coin::new(1_000, COSMOS_USDC), Coin::new(1_000, ETH_USDC)],
        )
        .with_instantiate_msg(InstantiateMsg {
            in_denom: ETH_USDC.to_string(),
            out_denom: COSMOS_USDC.to_string(),
            admin: "admin".to_string(),
        })
        .build();

    // failed when supply more than 1 denom
    let supplied_amount = vec![Coin::new(1_000, COSMOS_USDC), Coin::new(1_000, ETH_USDC)];
    let err = t
        .app
        .execute_contract(
            t.accounts["provider"].clone(),
            t.contract.clone(),
            &ExecMsg::Supply {},
            &supplied_amount,
        )
        .unwrap_err();

    assert_eq!(
        err.downcast_ref::<ContractError>().unwrap(),
        &ContractError::SingleCoinExpected {}
    );

    // failed to supply 0 denom
    let err = t
        .app
        .execute_contract(
            t.accounts["provider"].clone(),
            t.contract.clone(),
            &ExecMsg::Supply {},
            &[],
        )
        .unwrap_err();

    assert_eq!(
        err.downcast_ref::<ContractError>().unwrap(),
        &ContractError::SingleCoinExpected {}
    );

    // fail to supply with non out_coin's denom
    let supplied_amount = vec![Coin::new(1_000, ETH_USDC)];
    let err = t
        .app
        .execute_contract(
            t.accounts["provider"].clone(),
            t.contract.clone(),
            &ExecMsg::Supply {},
            &supplied_amount,
        )
        .unwrap_err();

    assert_eq!(
        err.downcast_ref::<ContractError>().unwrap(),
        &ContractError::InvalidSupplyDenom {
            denom: ETH_USDC.to_string(),
            expected_denom: COSMOS_USDC.to_string()
        }
    );

    // supply with out_coin should added to the contract's balance and update state
    let supplied_amount = vec![Coin::new(1_000, COSMOS_USDC)];
    t.app
        .execute_contract(
            t.accounts["provider"].clone(),
            t.contract.clone(),
            &ExecMsg::Supply {},
            &supplied_amount,
        )
        .unwrap();

    // check contract balances
    let contract_balances = t.app.wrap().query_all_balances(&t.contract).unwrap();
    assert_eq!(contract_balances, supplied_amount);

    // check pool balance
    let PoolResponse { pool } = t
        .app
        .wrap()
        .query_wasm_smart(t.contract.clone(), &QueryMsg::Pool {})
        .unwrap();

    assert_eq!(
        pool,
        TransmuterPool {
            pool_assets: vec![Coin::new(0, ETH_USDC), supplied_amount[0].clone()]
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

    assert_eq!(shares, supplied_amount[0].amount);
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
            admin: "admin".to_string(),
        })
        .build();

    // supply transmuter
    let supply_amount = vec![Coin::new(100_000, COSMOS_USDC)];

    // supplying with send tokens should not update pool balance
    t.app
        .send_tokens(
            t.accounts["provider"].clone(),
            t.contract.clone(),
            &supply_amount,
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

    // supply pool properly
    t.app
        .execute_contract(
            t.accounts["provider"].clone(),
            t.contract.clone(),
            &ExecMsg::Supply {},
            &supply_amount,
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
        &ContractError::SingleCoinExpected {}
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
fn test_admin() {
    let mut t = TestEnvBuilder::new()
        .with_instantiate_msg(InstantiateMsg {
            in_denom: ETH_USDC.to_string(),
            out_denom: COSMOS_USDC.to_string(),
            admin: "old_admin".to_string(),
        })
        .build();

    let AdminResponse { admin } = t
        .app
        .wrap()
        .query_wasm_smart(&t.contract, &QueryMsg::Admin {})
        .unwrap();

    assert_eq!(admin.unwrap(), "old_admin".to_string());

    // admin can update admin
    t.app
        .execute_contract(
            Addr::unchecked("old_admin"),
            t.contract.clone(),
            &ExecMsg::UpdateAdmin {
                new_admin: "new_admin".to_string(),
            },
            &[],
        )
        .unwrap();

    let AdminResponse { admin } = t
        .app
        .wrap()
        .query_wasm_smart(&t.contract, &QueryMsg::Admin {})
        .unwrap();

    assert_eq!(admin.unwrap(), "new_admin".to_string());

    // non-admin cannot update admin
    let err = t
        .app
        .execute_contract(
            Addr::unchecked("old_admin"),
            t.contract.clone(),
            &ExecMsg::UpdateAdmin {
                new_admin: "new_admin".to_string(),
            },
            &[],
        )
        .unwrap_err();

    assert_eq!(
        err.downcast_ref::<ContractError>().unwrap(),
        &ContractError::Unauthorized {}
    );
}

#[test]
fn test_withdraw() {
    let mut t = TestEnvBuilder::new()
        .with_account("user", vec![Coin::new(1_500, ETH_USDC)])
        .with_account("provider_1", vec![Coin::new(100_000, COSMOS_USDC)])
        .with_account("provider_2", vec![Coin::new(100_000, COSMOS_USDC)])
        .with_instantiate_msg(InstantiateMsg {
            in_denom: ETH_USDC.to_string(),
            out_denom: COSMOS_USDC.to_string(),
            admin: "admin".to_string(),
        })
        .build();

    // supply
    t.app
        .execute_contract(
            t.accounts["provider_1"].clone(),
            t.contract.clone(),
            &ExecMsg::Supply {},
            &[Coin::new(100_000, COSMOS_USDC)],
        )
        .unwrap();

    t.app
        .execute_contract(
            t.accounts["provider_2"].clone(),
            t.contract.clone(),
            &ExecMsg::Supply {},
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

    // non-provider cannot withdraw
    let err = t
        .app
        .execute_contract(
            t.accounts["user"].clone(),
            t.contract.clone(),
            &ExecMsg::Withdraw {
                coins: vec![Coin::new(1_500, ETH_USDC)],
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

    // provider can withdraw
    t.app
        .execute_contract(
            t.accounts["provider_1"].clone(),
            t.contract.clone(),
            &ExecMsg::Withdraw {
                coins: vec![Coin::new(500, ETH_USDC)],
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

    // provider can withdraw both sides
    t.app
        .execute_contract(
            t.accounts["provider_2"].clone(),
            t.contract.clone(),
            &ExecMsg::Withdraw {
                coins: vec![Coin::new(1_000, ETH_USDC), Coin::new(99_000, COSMOS_USDC)],
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

    // withdrawing excess shares fails
    let err = t
        .app
        .execute_contract(
            Addr::unchecked("provider_2"),
            t.contract.clone(),
            &ExecMsg::Withdraw {
                coins: vec![Coin::new(1, ETH_USDC)],
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
            &ExecMsg::Withdraw {
                coins: vec![Coin::new(1, ETH_USDC), Coin::new(1, COSMOS_USDC)],
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
