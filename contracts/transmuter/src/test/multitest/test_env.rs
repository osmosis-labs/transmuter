use std::{collections::HashMap, path::PathBuf};

use crate::{
    contract::{ContractExecMsg, ContractQueryMsg, ExecMsg, InstantiateMsg, QueryMsg},
    entry_points,
    sudo::SudoMsg,
    ContractError,
};
use anyhow::{bail, Result as AnyResult};
use cosmwasm_std::{from_slice, to_binary, Addr, Coin, Empty};
use osmosis_std::types::osmosis::cosmwasmpool::v1beta1::MsgCreateCosmWasmPool;
use osmosis_test_tube::cosmrs;

use cw_multi_test::{App, AppBuilder, Contract, Executor};
use osmosis_test_tube::{
    cosmrs::proto::{
        cosmos::bank::v1beta1::QueryAllBalancesRequest,
        cosmwasm::wasm::v1::MsgExecuteContractResponse,
    },
    Account, Bank, Module, OsmosisTestApp, RunnerError, RunnerExecuteResult, RunnerResult,
    SigningAccount, Wasm,
};
use serde::de::DeserializeOwned;

use crate::contract::Transmuter;

use super::modules::cosmwasm_pool::{self, CosmwasmPool};

impl Contract<Empty> for Transmuter<'_> {
    fn execute(
        &self,
        deps: cosmwasm_std::DepsMut<Empty>,
        env: cosmwasm_std::Env,
        info: cosmwasm_std::MessageInfo,
        msg: Vec<u8>,
    ) -> AnyResult<cosmwasm_std::Response<Empty>> {
        let msg = from_slice::<ContractExecMsg>(&msg)?;
        entry_points::execute(deps, env, info, msg).map_err(Into::into)
    }

    fn instantiate(
        &self,
        deps: cosmwasm_std::DepsMut<Empty>,
        env: cosmwasm_std::Env,
        info: cosmwasm_std::MessageInfo,
        msg: Vec<u8>,
    ) -> AnyResult<cosmwasm_std::Response<Empty>> {
        let msg = from_slice::<InstantiateMsg>(&msg)?;
        entry_points::instantiate(deps, env, info, msg).map_err(Into::into)
    }

    fn query(
        &self,
        deps: cosmwasm_std::Deps<Empty>,
        env: cosmwasm_std::Env,
        msg: Vec<u8>,
    ) -> AnyResult<cosmwasm_std::Binary> {
        let msg = from_slice::<ContractQueryMsg>(&msg)?;
        entry_points::query(deps, env, msg).map_err(Into::into)
    }

    fn sudo(
        &self,
        deps: cosmwasm_std::DepsMut<Empty>,
        env: cosmwasm_std::Env,
        msg: Vec<u8>,
    ) -> AnyResult<cosmwasm_std::Response<Empty>> {
        let msg = from_slice::<SudoMsg>(&msg)?;
        entry_points::sudo(deps, env, msg).map_err(Into::into)
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

pub struct TestEnv {
    pub app: App,
    pub creator: Addr,
    pub contract: Addr,
    pub accounts: HashMap<String, Addr>,
}

pub struct TestEnv2<'a> {
    pub app: &'a OsmosisTestApp,
    pub creator: SigningAccount,
    pub contract: TransmuterContract<'a>,
    pub accounts: HashMap<String, SigningAccount>,
}

impl<'a> TestEnv2<'a> {
    pub fn assert_account_balances(&self, account: &str, expected_balances: Vec<Coin>) {
        let account_balances: Vec<Coin> = Bank::new(self.app)
            .query_all_balances(&QueryAllBalancesRequest {
                address: self.accounts.get(account).unwrap().address(),
                pagination: None,
            })
            .unwrap()
            .balances
            .into_iter()
            .map(|coin| Coin::new(coin.amount.parse().unwrap(), coin.denom))
            .collect();

        assert_eq!(account_balances, expected_balances);
    }

    pub fn assert_contract_balances(&self, expected_balances: &[Coin]) {
        let contract_balances: Vec<Coin> = Bank::new(self.app)
            .query_all_balances(&QueryAllBalancesRequest {
                address: self.contract.contract_addr.clone(),
                pagination: None,
            })
            .unwrap()
            .balances
            .into_iter()
            .map(|coin| Coin::new(coin.amount.parse().unwrap(), coin.denom))
            .collect();

        assert_eq!(contract_balances, expected_balances);
    }
}

pub struct TestEnvBuilder {
    account_balances: HashMap<String, Vec<Coin>>,
    instantiate_msg: Option<InstantiateMsg>,
}

impl TestEnvBuilder {
    pub fn new() -> Self {
        Self {
            account_balances: HashMap::new(),
            instantiate_msg: None,
        }
    }

    pub fn with_instantiate_msg(mut self, msg: InstantiateMsg) -> Self {
        self.instantiate_msg = Some(msg);
        self
    }

    pub fn with_account(mut self, account: &str, balance: Vec<Coin>) -> Self {
        self.account_balances.insert(account.to_string(), balance);
        self
    }

    pub fn _build(self) -> TestEnv {
        let mut app = AppBuilder::default().build(|router, _, storage| {
            for (account, balance) in self.account_balances.clone() {
                router
                    .bank
                    .init_balance(storage, &Addr::unchecked(account), balance)
                    .unwrap();
            }
        });

        let creator = Addr::unchecked("creator");
        let code_id = app.store_code(Box::new(Transmuter::new()));
        let contract = app
            .instantiate_contract(
                code_id,
                creator.clone(),
                &self.instantiate_msg.expect("instantiate msg not set"),
                &[],
                "transmuter",
                None,
            )
            .unwrap();

        TestEnv {
            app,
            creator,
            contract,
            accounts: self
                .account_balances
                .keys()
                .map(|k| (k.clone(), Addr::unchecked(k.clone())))
                .collect(),
        }
    }

    pub fn build(self, app: &'_ OsmosisTestApp) -> TestEnv2<'_> {
        let accounts: HashMap<_, _> = self
            .account_balances
            .into_iter()
            .map(|(account, balance)| {
                let balance: Vec<_> = balance
                    .into_iter()
                    .chain(vec![Coin::new(1000000000000, "uosmo")])
                    .collect();
                (account, app.init_account(&balance).unwrap())
            })
            .collect();

        let creator = app
            .init_account(&[Coin::new(100000000000u128, "uosmo")])
            .unwrap();

        let contract = TransmuterContract::deploy(
            app,
            &self.instantiate_msg.expect("instantiate msg not set"),
            &creator,
        )
        .unwrap();

        TestEnv2 {
            app,
            creator,
            contract,
            accounts,
        }
    }
}

pub struct TransmuterContract<'a> {
    app: &'a OsmosisTestApp,
    pub code_id: u64,
    pub contract_addr: String,
}

impl<'a> TransmuterContract<'a> {
    pub fn deploy(
        app: &'a OsmosisTestApp,
        instantiate_msg: &InstantiateMsg,
        signer: &SigningAccount,
    ) -> Result<Self, RunnerError> {
        let wasm = Wasm::new(app);
        let cp = CosmwasmPool::new(app);

        let code_id = wasm
            .store_code(&Self::get_wasm_byte_code(), None, signer)?
            .data
            .code_id;

        // TODO: wait for roman's PR to be merged and use this instead
        // let res = cp.create_cosmwasm_pool(
        //     MsgCreateCosmWasmPool {
        //         code_id,
        //         instantiate_msg: to_binary(instantiate_msg).unwrap().to_vec(),
        //         sender: signer.address(),
        //     },
        //     signer,
        // )?;

        // dbg!(res.events);
        // let pool_id = res.data.pool_id;

        let contract_addr = wasm
            .instantiate(
                code_id,
                instantiate_msg,
                None,
                None,
                // denom creation fee
                &[Coin::new(10000000u128, "uosmo")],
                signer,
            )?
            .data
            .address;

        Ok(Self {
            app,
            code_id,
            contract_addr,
        })
    }

    pub fn execute(
        &self,
        msg: &ExecMsg,
        funds: &[Coin],
        signer: &SigningAccount,
    ) -> RunnerExecuteResult<MsgExecuteContractResponse> {
        let wasm = Wasm::new(self.app);
        wasm.execute(&self.contract_addr, msg, funds, signer)
    }

    pub fn query<Res>(&self, msg: &QueryMsg) -> RunnerResult<Res>
    where
        Res: ?Sized + DeserializeOwned,
    {
        let wasm = Wasm::new(self.app);
        wasm.query(&self.contract_addr, msg)
    }

    fn get_wasm_byte_code() -> Vec<u8> {
        let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        std::fs::read(
            manifest_path
                .join("..")
                .join("..")
                .join("target")
                .join("wasm32-unknown-unknown")
                .join("release")
                .join("transmuter.wasm"),
        )
        .unwrap()
    }
}

pub fn assert_contract_err(expected: ContractError, actual: RunnerError) {
    match actual {
        RunnerError::ExecuteError { msg } => {
            assert_eq!(
                format!(
                    "failed to execute message; message index: 0: {}: execute wasm contract failed",
                    expected
                ),
                msg
            )
        }
        _ => panic!("unexpected error, expect execute error but got: {}", actual),
    };
}

pub fn to_proto_coin(c: &cosmwasm_std::Coin) -> cosmrs::proto::cosmos::base::v1beta1::Coin {
    cosmrs::proto::cosmos::base::v1beta1::Coin {
        denom: c.denom.parse().unwrap(),
        amount: format!("{}", c.amount.u128()),
    }
}
