use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    ensure, ensure_eq, Addr, BankMsg, Coin, Deps, DepsMut, Env, MessageInfo, Response, StdError,
    Uint128,
};
use cw_controllers::{Admin, AdminResponse};
use cw_storage_plus::{Item, Map};
use sylvia::contract;

use crate::{error::ContractError, transmuter_pool::TransmuterPool};

// version info for migration info
const CONTRACT_NAME: &str = "crates.io:transmuter";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Transmuter<'a> {
    pub(crate) pool: Item<'a, TransmuterPool>,
    pub(crate) shares: Map<'a, &'a Addr, Uint128>,
    pub(crate) admin: Admin<'a>,
}

#[contract]
impl Transmuter<'_> {
    /// Create a Transmuter instance.
    pub const fn new() -> Self {
        Self {
            pool: Item::new("pool"),
            admin: Admin::new("admin"),
            shares: Map::new("shares"),
        }
    }

    /// Instantiate the contract with
    ///   `in_denom`  - the denom of the coin to be transmuted.
    ///   `out_denom` - the denom of the coin that is transmuted to, needs to be supplied to the contract.
    ///   `admin`     - the admin of the contract, can change the admin and withdraw funds.
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

    /// Join pool with tokens that exist in the pool.
    /// Token used to join pool is sent to the contract via `funds` in `MsgExecuteContract`.
    #[msg(exec)]
    fn join_pool(&self, ctx: (DepsMut, Env, MessageInfo)) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;

        // ensure funds not empty
        ensure!(
            !info.funds.is_empty(),
            ContractError::AtLeastSingleTokenExpected {}
        );

        // update shares
        self.shares.update(
            deps.storage,
            &info.sender,
            |shares| -> Result<Uint128, StdError> {
                Ok(shares.unwrap_or_default()
                    + info
                        .funds
                        .iter()
                        .fold(Uint128::zero(), |acc, c| acc + c.amount))
            },
        )?;

        // update pool
        self.pool
            .update(deps.storage, |mut pool| -> Result<_, ContractError> {
                pool.join_pool(&info.funds)?;
                Ok(pool)
            })?;

        Ok(Response::new().add_attribute("method", "join_pool"))
    }

    /// Transmute `in_coin` to `out_coin`.
    /// Recived `in_coin` from `MsgExecuteContract`'s funds and
    /// send `out_coin` back to the msg sender with 1:1 ratio.
    #[msg(exec)]
    fn transmute(&self, ctx: (DepsMut, Env, MessageInfo)) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;

        // ensure funds length == 1
        ensure_eq!(info.funds.len(), 1, ContractError::SingleTokenExpected {});

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

    /// Update the admin of the contract.
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

    /// Withdraw funds from the contract. Both `in_coin` and `out_coin` are withdrawable.
    /// Only admin can withdraw funds.
    #[msg(exec)]
    fn withdraw(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        coins: Vec<Coin>,
    ) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;

        // check if sender's shares is enough
        let sender_shares = self
            .shares
            .may_load(deps.storage, &info.sender)?
            .unwrap_or_default();

        let required_shares = coins
            .iter()
            .fold(Uint128::zero(), |acc, curr| acc + curr.amount);

        ensure!(
            sender_shares >= required_shares,
            ContractError::InsufficientShares {
                required: required_shares,
                available: sender_shares
            }
        );

        // update shares
        self.shares.update(
            deps.storage,
            &info.sender,
            |sender_shares| -> Result<Uint128, StdError> {
                Ok(sender_shares.unwrap_or_default() - required_shares)
            },
        )?;

        // withdraw
        self.pool
            .update(deps.storage, |mut pool| -> Result<_, ContractError> {
                pool.withdraw(&coins)?;
                Ok(pool)
            })?;

        let bank_send_msg = BankMsg::Send {
            to_address: info.sender.to_string(),
            amount: coins,
        };

        Ok(Response::new()
            .add_attribute("method", "withdraw")
            .add_message(bank_send_msg))
    }

    /// Query the admin of the contract.
    #[msg(query)]
    fn admin(&self, ctx: (Deps, Env)) -> Result<AdminResponse, StdError> {
        let (deps, _env) = ctx;
        self.admin.query_admin(deps)
    }

    /// Query the pool information of the contract.
    #[msg(query)]
    fn pool(&self, ctx: (Deps, Env)) -> Result<PoolResponse, ContractError> {
        let (deps, _env) = ctx;
        Ok(PoolResponse {
            pool: self.pool.load(deps.storage)?,
        })
    }

    #[msg(query)]
    fn shares(&self, ctx: (Deps, Env), address: String) -> Result<SharesResponse, ContractError> {
        let (deps, _env) = ctx;
        Ok(SharesResponse {
            shares: self
                .shares
                .may_load(deps.storage, &deps.api.addr_validate(&address)?)?
                .unwrap_or_default(),
        })
    }
}

#[cw_serde]
pub struct SharesResponse {
    pub shares: Uint128,
}

#[cw_serde]
pub struct PoolResponse {
    pub pool: TransmuterPool,
}
