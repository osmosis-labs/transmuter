use cosmwasm_std::{ensure_eq, BankMsg, Coin, Deps, DepsMut, Env, MessageInfo, Response, StdError};
use cw_controllers::{Admin, AdminResponse};
use cw_storage_plus::Item;
use sylvia::contract;

use crate::{error::ContractError, transmuter_pool::TransmuterPool};

// version info for migration info
const CONTRACT_NAME: &str = "crates.io:transmuter";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Transmuter<'a> {
    pub(crate) pool: Item<'a, TransmuterPool>,
    pub(crate) admin: Admin<'a>,
}

#[contract]
impl Transmuter<'_> {
    /// Create a new counter with the given initial count
    pub const fn new() -> Self {
        Self {
            pool: Item::new("pool"),
            admin: Admin::new("admin"),
        }
    }

    /// Instantiate the contract with the initial count
    #[msg(instantiate)]
    pub fn instantiate(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        in_denom: String,
        out_denom: String,
        admin: String,
    ) -> Result<Response, ContractError> {
        let (deps, _env, _info) = ctx;

        // store contract version for migration info
        cw2::set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

        // store pool
        self.pool
            .save(deps.storage, &TransmuterPool::new(&in_denom, &out_denom))?;

        // store admin
        let admin = deps.api.addr_validate(&admin)?;
        self.admin.set(deps, Some(admin))?;

        Ok(Response::new()
            .add_attribute("method", "instantiate")
            .add_attribute("contract_name", CONTRACT_NAME)
            .add_attribute("contract_version", CONTRACT_VERSION))
    }

    /// supply the contract with coin that matches out_coin's denom
    #[msg(exec)]
    fn supply(&self, ctx: (DepsMut, Env, MessageInfo)) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;

        // check if funds length == 1
        ensure_eq!(
            info.funds.len(),
            1,
            ContractError::Std(StdError::generic_err(
                "supply requires funds to have exactly one denom"
            ))
        );

        // update pool
        self.pool
            .update(deps.storage, |mut pool| -> Result<_, ContractError> {
                pool.supply(&info.funds[0])?;
                Ok(pool)
            })?;

        Ok(Response::new().add_attribute("method", "supply"))
    }

    #[msg(exec)]
    fn transmute(&self, ctx: (DepsMut, Env, MessageInfo)) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;

        // ensure funds length == 1
        ensure_eq!(info.funds.len(), 1, ContractError::SingleCoinExpected {});

        // transmute
        let mut pool = self.pool.load(deps.storage)?;
        let in_coin = info.funds[0].clone();
        let out_coin = pool.transmute(&in_coin)?;

        // save pool
        self.pool.save(deps.storage, &pool)?;

        let bank_send_msg = BankMsg::Send {
            to_address: info.sender.to_string(),
            amount: vec![out_coin],
        };

        Ok(Response::new()
            .add_attribute("method", "transmute")
            .add_message(bank_send_msg))
    }

    #[msg(exec)]
    fn update_admin(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        new_admin: String,
    ) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;
        let new_admin = deps.api.addr_validate(&new_admin)?;

        self.admin
            .execute_update_admin(deps, info, Some(new_admin))
            .map_err(|_| ContractError::Unauthorized {})
    }

    #[msg(exec)]
    fn withdraw(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        coins: Vec<Coin>,
    ) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;

        // allow only admin to withdraw
        self.admin
            .assert_admin(deps.as_ref(), &info.sender)
            .map_err(|_| ContractError::Unauthorized {})?;

        // withdraw
        let mut pool = self.pool.load(deps.storage)?;
        pool.withdraw(&coins)?;

        // save pool
        self.pool.save(deps.storage, &pool)?;

        let bank_send_msg = BankMsg::Send {
            to_address: info.sender.to_string(),
            amount: coins,
        };

        Ok(Response::new()
            .add_attribute("method", "withdraw")
            .add_message(bank_send_msg))
    }

    #[msg(query)]
    fn admin(&self, ctx: (Deps, Env)) -> Result<AdminResponse, StdError> {
        let (deps, _env) = ctx;
        self.admin.query_admin(deps)
    }

    #[msg(query)]
    fn pool(&self, ctx: (Deps, Env)) -> Result<TransmuterPool, ContractError> {
        let (deps, _env) = ctx;
        Ok(self.pool.load(deps.storage)?)
    }
}
