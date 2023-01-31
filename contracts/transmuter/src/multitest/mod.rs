mod test_env;

use crate::{
    contract::{ExecMsg, InstantiateMsg, QueryMsg},
    transmuter_pool::TransmuterPool,
    ContractError,
};
use cosmwasm_std::{Coin, OverflowError, OverflowOperation, StdError};
use cw_multi_test::Executor;
use test_env::*;

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
        &ContractError::Std(StdError::generic_err(
            "supply requires funds to have exactly one denom"
        ))
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
        &ContractError::Std(StdError::generic_err(
            "supply requires funds to have exactly one denom"
        ))
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
    let pool: TransmuterPool = t
        .app
        .wrap()
        .query_wasm_smart(t.contract.clone(), &QueryMsg::Pool {})
        .unwrap();

    assert_eq!(
        pool,
        TransmuterPool {
            in_coin: Coin::new(0, ETH_USDC),
            out_coin_reserve: supplied_amount[0].clone()
        }
    );
}

#[test]
fn test_transmute() {
    let mut t = TestEnvBuilder::new()
        .with_account("user", vec![Coin::new(1_500, ETH_USDC)])
        .with_account(
            "provider",
            vec![Coin::new(1_000, ETH_USDC), Coin::new(1_000, COSMOS_USDC)],
        )
        .with_instantiate_msg(InstantiateMsg {
            in_denom: ETH_USDC.to_string(),
            out_denom: COSMOS_USDC.to_string(),
        })
        .build();

    // supply transmuter
    let supply_amount = vec![Coin::new(1_000, ETH_USDC), Coin::new(1_000, COSMOS_USDC)];
    t.app
        .execute_contract(
            t.accounts["provider"].clone(),
            t.contract.clone(),
            &ExecMsg::Supply {},
            &supply_amount,
        )
        .unwrap();

    // check balances
    let contract_balances = t.app.wrap().query_all_balances(&t.contract).unwrap();
    assert_eq!(contract_balances, supply_amount);

    // transmute
    t.app
        .execute_contract(
            t.accounts["user"].clone(),
            t.contract.clone(),
            &ExecMsg::Transmute {},
            &[Coin::new(1000, ETH_USDC)],
        )
        .unwrap();

    // query balance again
    let contract_balances = t.app.wrap().query_all_balances(&t.contract).unwrap();
    let user_balances = t
        .app
        .wrap()
        .query_all_balances(&t.accounts["user"])
        .unwrap();

    assert_eq!(contract_balances, vec![Coin::new(2000, ETH_USDC)]);
    assert_eq!(
        user_balances,
        vec![Coin::new(500, ETH_USDC), Coin::new(1000, COSMOS_USDC)]
    );

    // transmute fail due to no more funds in the contract to transmute
    let err = t
        .app
        .execute_contract(
            t.accounts["user"].clone(),
            t.contract,
            &ExecMsg::Transmute {},
            &[Coin::new(500, ETH_USDC)],
        )
        .unwrap_err();

    assert_eq!(
        err.downcast_ref::<StdError>().unwrap(),
        &StdError::overflow(OverflowError::new(OverflowOperation::Sub, "0", "500"))
    );

    // bank send to contract should not change the balance
}
