mod admin;
pub mod contract;
mod error;
mod shares;
mod sudo;
mod transmuter_pool;
pub use crate::error::ContractError;

#[cfg(test)]
mod test;

#[cfg(not(feature = "library"))]
mod entry_points {
    use cosmwasm_std::{
        ensure, entry_point, Binary, Deps, DepsMut, Env, MessageInfo, Reply, Response,
    };

    use crate::contract::{ContractExecMsg, ContractQueryMsg, InstantiateMsg, Transmuter};
    use crate::error::ContractError;
    use crate::sudo::SudoMsg;

    const CONTRACT: Transmuter = Transmuter::new();

    #[entry_point]
    pub fn instantiate(
        deps: DepsMut,
        env: Env,
        info: MessageInfo,
        msg: InstantiateMsg,
    ) -> Result<Response, ContractError> {
        msg.dispatch(&CONTRACT, (deps, env, info))
    }

    #[entry_point]
    pub fn reply(deps: DepsMut, env: Env, msg: Reply) -> Result<Response, ContractError> {
        CONTRACT.reply((deps, env), msg)
    }

    #[entry_point]
    pub fn execute(
        deps: DepsMut,
        env: Env,
        info: MessageInfo,
        msg: ContractExecMsg,
    ) -> Result<Response, ContractError> {
        ensure!(
            CONTRACT.is_active((deps.as_ref(), env.clone()))?.is_active,
            ContractError::InactivePool {}
        );

        msg.dispatch(&CONTRACT, (deps, env, info))
    }

    #[entry_point]
    pub fn query(deps: Deps, env: Env, msg: ContractQueryMsg) -> Result<Binary, ContractError> {
        msg.dispatch(&CONTRACT, (deps, env))
    }

    #[entry_point]
    pub fn sudo(deps: DepsMut, env: Env, msg: SudoMsg) -> Result<Response, ContractError> {
        match msg {
            // allow setting active/inactive state without being active
            SudoMsg::SetActive { .. } => msg.dispatch(&CONTRACT, (deps, env)),

            // the rest of the sudo messages require the contract to be active
            _ => {
                ensure!(
                    CONTRACT.is_active((deps.as_ref(), env.clone()))?.is_active,
                    ContractError::InactivePool {}
                );

                msg.dispatch(&CONTRACT, (deps, env))
            }
        }
    }
}

#[cfg(not(feature = "library"))]
pub use crate::entry_points::*;
