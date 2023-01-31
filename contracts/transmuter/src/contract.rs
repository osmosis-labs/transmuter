use cosmwasm_std::{ensure_eq, BankMsg, Coin, Deps, DepsMut, Env, MessageInfo, Response, StdError};
use cw_storage_plus::Item;
use sylvia::contract;

use crate::{error::ContractError, transmuter_pool::TransmuterPool};

// version info for migration info
const CONTRACT_NAME: &str = "crates.io:transmuter";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Transmuter<'a> {
    pub(crate) pool: Item<'a, TransmuterPool>,
    pub(crate) in_denom: Item<'a, Coin>,
    pub(crate) out_denom: Item<'a, Coin>,
}

#[contract]
impl Transmuter<'_> {
    /// Create a new counter with the given initial count
    pub const fn new() -> Self {
        Self {
            in_denom: Item::new("in_denom"),
            out_denom: Item::new("out_denom"),
            pool: Item::new("pool"),
        }
    }

    /// Instantiate the contract with the initial count
    #[msg(instantiate)]
    pub fn instantiate(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        in_denom: String,
        out_denom: String,
    ) -> Result<Response, ContractError> {
        let (deps, _env, _info) = ctx;

        // store contract version for migration info
        cw2::set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

        // store pool
        self.pool
            .save(deps.storage, &TransmuterPool::new(&in_denom, &out_denom))?;

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

        // check if info.funds.length > 1
        // if yes, return `ContractError::TooManyDenomsToTransmute`
        if info.funds.len() > 1 {
            return Err(ContractError::TooManyCoinsToTransmute {});
        }

        let in_coin = info.funds[0].clone();

        // check if the in_coin is of in_denom or out_denom's denom
        // if not, return `ContractError::DenomNotAllowed`
        let in_denom = self.in_denom.load(deps.storage)?;
        let out_denom = self.out_denom.load(deps.storage)?;

        let bank_send_msg = BankMsg::Send {
            to_address: info.sender.to_string(),
            // in: in_denom, out: out_denom and vice versa
            // with the same amount
            amount: vec![Coin {
                amount: in_coin.amount,
                denom: transmute_denom(in_coin.denom, in_denom.denom, out_denom.denom)?,
            }],
        };

        Ok(Response::new()
            .add_attribute("method", "transmute")
            .add_message(bank_send_msg))
    }

    #[msg(query)]
    fn pool(&self, ctx: (Deps, Env)) -> Result<TransmuterPool, ContractError> {
        let (deps, _env) = ctx;
        Ok(self.pool.load(deps.storage)?)
    }
}

fn transmute_denom(
    in_denom: String,
    a_denom: String,
    b_denom: String,
) -> Result<String, ContractError> {
    if in_denom == a_denom {
        Ok(b_denom)
    } else if in_denom == b_denom {
        Ok(a_denom)
    } else {
        Err(ContractError::DenomNotAllowed { denom: in_denom })
    }
}
