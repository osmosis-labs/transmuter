use cosmwasm_std::{Addr, Deps, DepsMut, StdError, Storage};
use cw_storage_plus::Item;

use crate::ContractError;

pub struct Moderator {
    moderator: Item<Addr>,
}

impl Moderator {
    pub const fn new(namespace: &'static str) -> Self {
        Self {
            moderator: Item::new(namespace),
        }
    }

    pub fn init(&self, storage: &mut dyn Storage, address: Addr) -> Result<(), ContractError> {
        self.moderator.save(storage, &address).map_err(Into::into)
    }

    pub fn get(&self, deps: Deps) -> Result<Addr, ContractError> {
        self.moderator
            .may_load(deps.storage)?
            .ok_or(StdError::not_found("moderator"))
            .map_err(Into::into)
    }

    pub(crate) fn unchecked_set(&self, deps: DepsMut, address: Addr) -> Result<(), ContractError> {
        self.moderator
            .save(deps.storage, &address)
            .map_err(Into::into)
    }
}

/// Ensure that the sender is the current moderator
///
/// This macro ensures that the sender is the current moderator. It is used to protect
/// sensitive operations that should only be performed by the moderator.
///
/// If the `sender_address` is not the current moderator, the macro will
/// return an `Err(ContractError::Unauthorized {})`.
#[macro_export]
macro_rules! ensure_moderator_authority {
    ($sender:expr, $moderator: expr, $deps:expr) => {
        let current_moderator = $moderator.get($deps)?;
        if ($sender != current_moderator) {
            return Err($crate::ContractError::Unauthorized {});
        }
    };
}
