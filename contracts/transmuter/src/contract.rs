use cosmwasm_std::{BankMsg, Coin, DepsMut, Env, MessageInfo, Response};
use cw_storage_plus::Item;
use sylvia::contract;

use crate::error::ContractError;

// version info for migration info
const CONTRACT_NAME: &str = "crates.io:transmuter";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Transmuter<'a> {
    pub(crate) asset_a: Item<'a, Coin>,
    pub(crate) asset_b: Item<'a, Coin>,
}

#[contract]
impl Transmuter<'_> {
    /// Create a new counter with the given initial count
    pub const fn new() -> Self {
        Self {
            asset_a: Item::new("asset_a"),
            asset_b: Item::new("asset_b"),
        }
    }

    /// Instantiate the contract with the initial count
    #[msg(instantiate)]
    pub fn instantiate(
        &self,
        ctx: (DepsMut, Env, MessageInfo),
        asset_a: String,
        asset_b: String,
    ) -> Result<Response, ContractError> {
        let (deps, _env, _info) = ctx;

        // store contract version for migration info
        cw2::set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

        // store asset_a and asset_b
        self.asset_a
            .save(deps.storage, &Coin::new(0u128, asset_a))?;
        self.asset_b
            .save(deps.storage, &Coin::new(0u128, asset_b))?;

        Ok(Response::new()
            .add_attribute("method", "instantiate")
            .add_attribute("contract_name", CONTRACT_NAME)
            .add_attribute("contract_version", CONTRACT_VERSION))
    }

    /// funds the contract with asset_a and/or asset_b
    /// if the funds are not asset_a or asset_b, returns `ContractError::DenomNotAllowed`
    #[msg(exec)]
    fn fund(&self, ctx: (DepsMut, Env, MessageInfo)) -> Result<Response, ContractError> {
        let (deps, _env, info) = ctx;

        // get funds from msg info
        let funds = info.funds;

        // check if all funds are of asset_a or asset_b's denom
        // if not, return `ContractError::DenomNotAllowed`
        for coin in funds.iter() {
            // if coin is has denom either asset_a or asset_b, add to asset_a or asset_b
            // else return DenomNotAllowed

            let asset_a = self.asset_a.load(deps.storage)?;
            let asset_b = self.asset_b.load(deps.storage)?;

            if coin.denom != asset_a.denom && coin.denom != asset_b.denom {
                return Err(ContractError::DenomNotAllowed {
                    denom: coin.denom.clone(),
                });
            }

            // add funds to asset_a or asset_b
            if coin.denom == asset_a.denom {
                self.asset_a.save(
                    deps.storage,
                    &Coin {
                        amount: asset_a.amount + coin.amount,
                        denom: asset_a.denom.clone(),
                    },
                )?;
            } else if coin.denom == asset_b.denom {
                self.asset_b.save(
                    deps.storage,
                    &Coin {
                        amount: asset_b.amount + coin.amount,
                        denom: asset_b.denom.clone(),
                    },
                )?;
            }
        }

        Ok(Response::new().add_attribute("method", "fund"))
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

        // check if the in_coin is of asset_a or asset_b's denom
        // if not, return `ContractError::DenomNotAllowed`
        let asset_a = self.asset_a.load(deps.storage)?;
        let asset_b = self.asset_b.load(deps.storage)?;

        let bank_send_msg = BankMsg::Send {
            to_address: info.sender.to_string(),
            // in: asset_a, out: asset_b and vice versa
            // with the same amount
            amount: vec![Coin {
                amount: in_coin.amount,
                denom: transmute_denom(in_coin.denom, asset_a.denom, asset_b.denom)?,
            }],
        };

        Ok(Response::new()
            .add_attribute("method", "transmute")
            .add_message(bank_send_msg))
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
