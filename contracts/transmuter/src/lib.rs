mod alloyed_asset;
mod asset;
pub mod contract;
mod corruptable;
mod error;
mod incentive_pool;
mod math;
mod migrations;
mod rebalancer;
mod role;
mod scope;
mod sudo;
mod swap;
mod transmuter_pool;
pub use crate::error::ContractError;

#[cfg(test)]
mod test;

#[cfg(not(feature = "library"))]
mod entry_points {
    use cosmwasm_std::{
        ensure, entry_point, Binary, Deps, DepsMut, Env, MessageInfo, Reply, Response,
    };

    use crate::contract::sv::{ContractExecMsg, ContractQueryMsg, ExecMsg, InstantiateMsg};
    use crate::contract::Transmuter;
    use crate::error::ContractError;
    use crate::migrations;
    use crate::sudo::SudoMsg;

    const CONTRACT: Transmuter = Transmuter::new();

    macro_rules! ensure_active_status {
        ($msg:expr, $deps:expr, $env:expr, except: $pattern:pat) => {
            match $msg {
                $pattern => (),
                _ => {
                    ensure!(
                        CONTRACT
                            .is_active(sylvia::ctx::QueryCtx::from(($deps.as_ref(), $env.clone())))?
                            .is_active,
                        ContractError::InactivePool {}
                    );
                }
            }
        };
    }

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
        ensure_active_status!(
            msg,
            deps,
            env,
            except: ContractExecMsg::Transmuter(ExecMsg::SetActiveStatus { .. })
        );

        msg.dispatch(&CONTRACT, (deps, env, info))
    }

    #[entry_point]
    pub fn query(deps: Deps, env: Env, msg: ContractQueryMsg) -> Result<Binary, ContractError> {
        msg.dispatch(&CONTRACT, (deps, env))
    }

    #[entry_point]
    pub fn sudo(deps: DepsMut, env: Env, msg: SudoMsg) -> Result<Response, ContractError> {
        ensure_active_status!(
            msg,
            deps,
            env,
            except: SudoMsg::SetActive { .. }
        );

        msg.dispatch(&CONTRACT, (deps, env))
    }

    #[entry_point]
    pub fn migrate(
        deps: DepsMut,
        _env: Env,
        _msg: migrations::v4_0_0::MigrateMsg,
    ) -> Result<Response, ContractError> {
        migrations::v4_0_0::execute_migration(deps)
    }
}

#[cfg(not(feature = "library"))]
pub use crate::entry_points::*;
