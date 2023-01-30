use crate::contract::{ContractExecMsg, ContractQueryMsg, ExecMsg, InstantiateMsg};
use anyhow::{bail, Result as AnyResult};
use cosmwasm_std::{
    from_slice, Addr, Coin, CosmosMsg, Empty, OverflowError, OverflowOperation, StdError,
};
use cw_multi_test::{AppBuilder, Contract, Executor};

use crate::contract::Transmuter;

impl Contract<Empty> for Transmuter<'_> {
    fn execute(
        &self,
        deps: cosmwasm_std::DepsMut<Empty>,
        env: cosmwasm_std::Env,
        info: cosmwasm_std::MessageInfo,
        msg: Vec<u8>,
    ) -> AnyResult<cosmwasm_std::Response<Empty>> {
        from_slice::<ContractExecMsg>(&msg)?
            .dispatch(self, (deps, env, info))
            .map_err(Into::into)
    }

    fn instantiate(
        &self,
        deps: cosmwasm_std::DepsMut<Empty>,
        env: cosmwasm_std::Env,
        info: cosmwasm_std::MessageInfo,
        msg: Vec<u8>,
    ) -> AnyResult<cosmwasm_std::Response<Empty>> {
        from_slice::<InstantiateMsg>(&msg)?
            .dispatch(self, (deps, env, info))
            .map_err(Into::into)
    }

    fn query(
        &self,
        deps: cosmwasm_std::Deps<Empty>,
        env: cosmwasm_std::Env,
        msg: Vec<u8>,
    ) -> AnyResult<cosmwasm_std::Binary> {
        from_slice::<ContractQueryMsg>(&msg)?
            .dispatch(self, (deps, env))
            .map_err(Into::into)
    }

    fn sudo(
        &self,
        _deps: cosmwasm_std::DepsMut<Empty>,
        _env: cosmwasm_std::Env,
        _msg: Vec<u8>,
    ) -> AnyResult<cosmwasm_std::Response<Empty>> {
        bail!("sudo not implemented for contract")
    }

    fn reply(
        &self,
        _deps: cosmwasm_std::DepsMut<Empty>,
        _env: cosmwasm_std::Env,
        _msg: cosmwasm_std::Reply,
    ) -> AnyResult<cosmwasm_std::Response<Empty>> {
        bail!("reply not implemented for contract")
    }

    fn migrate(
        &self,
        _deps: cosmwasm_std::DepsMut<Empty>,
        _env: cosmwasm_std::Env,
        _msg: Vec<u8>,
    ) -> AnyResult<cosmwasm_std::Response<Empty>> {
        bail!("migrate not implemented for contract")
    }
}

#[test]
fn instantiate_fund_and_transmute() {
    let bob = Addr::unchecked("bob");
    let creator = Addr::unchecked("creator");

    let mut app = AppBuilder::default().build(|router, _, storage| {
        router
            .bank
            .init_balance(
                storage,
                &creator,
                vec![Coin::new(1_000, "uusdc"), Coin::new(1_000, "uusdt")],
            )
            .unwrap();
        router
            .bank
            .init_balance(storage, &bob, vec![Coin::new(1_500, "uusdc")])
            .unwrap();
    });

    let code_id = app.store_code(Box::new(Transmuter::new()));

    // instantiate the contract
    let contract = app
        .instantiate_contract(
            code_id,
            creator.clone(),
            &InstantiateMsg {
                asset_a: "uusdt".to_string(),
                asset_b: "uusdc".to_string(),
            },
            &[],
            "transmuter",
            None,
        )
        .unwrap();

    // fund transmuter
    let funded_amount = vec![Coin::new(1_000, "uusdc"), Coin::new(1_000, "uusdt")];
    app.execute(
        creator,
        CosmosMsg::Bank(cosmwasm_std::BankMsg::Send {
            to_address: contract.clone().into(),
            amount: funded_amount.clone(),
        }),
    )
    .unwrap();

    // check balances
    let contract_balances = app.wrap().query_all_balances(&contract).unwrap();
    assert_eq!(contract_balances, funded_amount);

    // transmute
    app.execute_contract(
        bob.clone(),
        contract.clone(),
        &ExecMsg::Transmute {},
        &[Coin::new(1000, "uusdc")],
    )
    .unwrap();

    // query balance again
    let contract_balances = app.wrap().query_all_balances(&contract).unwrap();
    let bob_balances = app.wrap().query_all_balances(&bob).unwrap();

    assert_eq!(contract_balances, vec![Coin::new(2000, "uusdc")]);
    assert_eq!(
        bob_balances,
        vec![Coin::new(500, "uusdc"), Coin::new(1000, "uusdt")]
    );

    // transmute fail due to no more funds in the contract to transmute
    let err = app
        .execute_contract(
            bob,
            contract,
            &ExecMsg::Transmute {},
            &[Coin::new(500, "uusdc")],
        )
        .unwrap_err();

    assert_eq!(
        err.downcast_ref::<StdError>().unwrap(),
        &StdError::overflow(OverflowError::new(OverflowOperation::Sub, "0", "500"))
    );
}
