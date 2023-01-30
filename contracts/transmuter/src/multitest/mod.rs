mod test_env;

use crate::contract::{ExecMsg, InstantiateMsg};
use cosmwasm_std::{Coin, OverflowError, OverflowOperation, StdError};
use cw_multi_test::Executor;
use test_env::*;

#[test]
fn test_transmute() {
    let eth_usdc = "ibc/AXLETHUSDC";
    let cosmos_usdc = "ibc/COSMOSUSDC";

    let mut t = TestEnvBuilder::new()
        .with_account("user", vec![Coin::new(1_500, eth_usdc)])
        .with_account(
            "provider",
            vec![Coin::new(1_000, eth_usdc), Coin::new(1_000, cosmos_usdc)],
        )
        .with_instantiate_msg(InstantiateMsg {
            asset_a: eth_usdc.to_string(),
            asset_b: cosmos_usdc.to_string(),
        })
        .build();

    // fund transmuter
    let funded_amount = vec![Coin::new(1_000, eth_usdc), Coin::new(1_000, cosmos_usdc)];
    t.app
        .execute_contract(
            t.accounts["provider"].clone(),
            t.contract.clone(),
            &ExecMsg::Fund {},
            &funded_amount,
        )
        .unwrap();

    // check balances
    let contract_balances = t.app.wrap().query_all_balances(&t.contract).unwrap();
    assert_eq!(contract_balances, funded_amount);

    // transmute
    t.app
        .execute_contract(
            t.accounts["user"].clone(),
            t.contract.clone(),
            &ExecMsg::Transmute {},
            &[Coin::new(1000, eth_usdc)],
        )
        .unwrap();

    // query balance again
    let contract_balances = t.app.wrap().query_all_balances(&t.contract).unwrap();
    let user_balances = t
        .app
        .wrap()
        .query_all_balances(&t.accounts["user"])
        .unwrap();

    assert_eq!(contract_balances, vec![Coin::new(2000, eth_usdc)]);
    assert_eq!(
        user_balances,
        vec![Coin::new(500, eth_usdc), Coin::new(1000, cosmos_usdc)]
    );

    // transmute fail due to no more funds in the contract to transmute
    let err = t
        .app
        .execute_contract(
            t.accounts["user"].clone(),
            t.contract,
            &ExecMsg::Transmute {},
            &[Coin::new(500, eth_usdc)],
        )
        .unwrap_err();

    assert_eq!(
        err.downcast_ref::<StdError>().unwrap(),
        &StdError::overflow(OverflowError::new(OverflowOperation::Sub, "0", "500"))
    );
}
