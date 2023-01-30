mod suite;

use crate::contract::{ExecMsg, InstantiateMsg};
use cosmwasm_std::{Coin, CosmosMsg, OverflowError, OverflowOperation, StdError};
use cw_multi_test::Executor;
use suite::*;

#[test]
fn instantiate_fund_and_transmute() {
    let mut suite = SuiteBuilder::new()
        .with_account("user", vec![Coin::new(1_500, "uusdc")])
        .with_account(
            "provider",
            vec![Coin::new(1_000, "uusdc"), Coin::new(1_000, "uusdt")],
        )
        .with_instantiate_msg(InstantiateMsg {
            asset_a: "uusdt".to_string(),
            asset_b: "uusdc".to_string(),
        })
        .build();

    // fund transmuter
    let funded_amount = vec![Coin::new(1_000, "uusdc"), Coin::new(1_000, "uusdt")];
    suite
        .app
        .execute(
            suite.accounts["provider"].clone(),
            CosmosMsg::Bank(cosmwasm_std::BankMsg::Send {
                to_address: suite.contract.clone().into(),
                amount: funded_amount.clone(),
            }),
        )
        .unwrap();

    // check balances
    let contract_balances = suite
        .app
        .wrap()
        .query_all_balances(&suite.contract)
        .unwrap();
    assert_eq!(contract_balances, funded_amount);

    // transmute
    suite
        .app
        .execute_contract(
            suite.accounts["user"].clone(),
            suite.contract.clone(),
            &ExecMsg::Transmute {},
            &[Coin::new(1000, "uusdc")],
        )
        .unwrap();

    // query balance again
    let contract_balances = suite
        .app
        .wrap()
        .query_all_balances(&suite.contract)
        .unwrap();
    let bob_balances = suite
        .app
        .wrap()
        .query_all_balances(&suite.accounts["user"])
        .unwrap();

    assert_eq!(contract_balances, vec![Coin::new(2000, "uusdc")]);
    assert_eq!(
        bob_balances,
        vec![Coin::new(500, "uusdc"), Coin::new(1000, "uusdt")]
    );

    // transmute fail due to no more funds in the contract to transmute
    let err = suite
        .app
        .execute_contract(
            suite.accounts["user"].clone(),
            suite.contract,
            &ExecMsg::Transmute {},
            &[Coin::new(500, "uusdc")],
        )
        .unwrap_err();

    assert_eq!(
        err.downcast_ref::<StdError>().unwrap(),
        &StdError::overflow(OverflowError::new(OverflowOperation::Sub, "0", "500"))
    );
}
