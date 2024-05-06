use cosmwasm_schema::cw_serde;
use cosmwasm_std::{ensure, Addr, Deps, DepsMut, StdError, Storage};
use cw_storage_plus::Item;

use crate::ContractError;

pub struct Admin<'a> {
    state: Item<'a, AdminState>,
}

/// State of the admin to be stored in the contract storage
#[cw_serde]
pub enum AdminState {
    Claimed(Addr),
    Transferring { current: Addr, candidate: Addr },
}

impl<'a> Admin<'a> {
    pub const fn new(namespace: &'a str) -> Self {
        Self {
            state: Item::new(namespace),
        }
    }

    /// Initialize the admin state
    pub fn init(&self, storage: &mut dyn Storage, address: Addr) -> Result<(), ContractError> {
        self.state
            .save(storage, &AdminState::Claimed(address))
            .map_err(Into::into)
    }

    /// Get current admin address
    pub fn current(&self, deps: Deps) -> Result<Addr, ContractError> {
        let admin = self
            .state
            .may_load(deps.storage)?
            .ok_or(StdError::not_found("admin"))?;

        match admin {
            AdminState::Claimed(address) => Ok(address),
            AdminState::Transferring { current, .. } => Ok(current),
        }
    }

    /// Get candidate admin address. Returns None if there is no candidate.
    pub fn candidate(&self, deps: Deps) -> Result<Option<Addr>, ContractError> {
        let admin = self
            .state
            .may_load(deps.storage)?
            .ok_or(StdError::not_found("admin"))?;

        match admin {
            AdminState::Claimed(_) => Ok(None),
            AdminState::Transferring { candidate, .. } => Ok(Some(candidate)),
        }
    }

    /// Transfer admin rights to a new candidate
    pub fn transfer(
        &self,
        deps: DepsMut,
        sender: Addr,
        candidate: Addr,
    ) -> Result<(), ContractError> {
        // Make sure that the sender is the current admin
        let current_admin = self.current(deps.as_ref())?;
        ensure!(sender == current_admin, ContractError::Unauthorized {});

        // Set the candidate admin address
        self.state
            .save(
                deps.storage,
                &AdminState::Transferring {
                    current: current_admin,
                    candidate,
                },
            )
            .map_err(Into::into)
    }

    /// Claim admin rights
    pub fn claim(&self, deps: DepsMut, sender: Addr) -> Result<(), ContractError> {
        // Make sure that the sender is the candidate
        let candidate = self
            .candidate(deps.as_ref())?
            .ok_or(ContractError::Unauthorized {})?;

        ensure!(candidate == sender, ContractError::Unauthorized {});

        // Set the current admin to the candidate
        self.state
            .save(deps.storage, &AdminState::Claimed(sender))
            .map_err(Into::into)
    }

    /// Cancel admin transfer
    pub fn cancel_transfer(&self, deps: DepsMut, sender: Addr) -> Result<(), ContractError> {
        match self.state(deps.as_ref())? {
            AdminState::Claimed(_) => Err(ContractError::InoperableAdminTransferringState {}),
            AdminState::Transferring { current, .. } => {
                // Make sure that the sender is the current admin
                ensure!(sender == current, ContractError::Unauthorized {});

                // Cancel the transfer
                self.state
                    .save(deps.storage, &AdminState::Claimed(current))
                    .map_err(Into::into)
            }
        }
    }

    /// Reject admin transfer
    pub fn reject_transfer(&self, deps: DepsMut, sender: Addr) -> Result<(), ContractError> {
        match self.state(deps.as_ref())? {
            AdminState::Claimed(_) => Err(ContractError::InoperableAdminTransferringState {}),
            AdminState::Transferring { current, candidate } => {
                // Make sure that the sender is the candidate
                ensure!(candidate == sender, ContractError::Unauthorized {});

                // Cancel the transfer
                self.state
                    .save(deps.storage, &AdminState::Claimed(current))
                    .map_err(Into::into)
            }
        }
    }

    fn state(&self, deps: Deps) -> Result<AdminState, ContractError> {
        self.state
            .may_load(deps.storage)?
            .ok_or(StdError::not_found("admin"))
            .map_err(Into::into)
    }
}

/// Ensure that the sender is the current admin
///
/// This macro ensures that the sender is the current admin. It is used to protect
/// sensitive operations that should only be performed by the admin.
///
/// If the `sender_address` is not the current admin, the macro will
/// return an `Err(ContractError::Unauthorized {})`.
#[macro_export]
macro_rules! ensure_admin_authority {
    ($sender:expr, $admin: expr, $deps:expr) => {
        let current_admin = $admin.current($deps)?;
        if ($sender != current_admin) {
            return Err($crate::ContractError::Unauthorized {});
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmwasm_std::testing::mock_dependencies;

    #[test]
    fn test_admin() {
        let mut deps = mock_dependencies();

        let admin = Admin::new("admin");
        let admin_addr = Addr::unchecked("admin");
        let random_addr = Addr::unchecked("random");
        let candidate_addr = Addr::unchecked("candidate");

        // Initialize admin
        assert_eq!(
            admin.init(deps.as_mut().storage, admin_addr.clone()),
            Ok(())
        );

        // Initial state
        assert_eq!(admin.current(deps.as_ref()), Ok(admin_addr.clone()));
        assert_eq!(admin.candidate(deps.as_ref()), Ok(None));

        // Transfer admin rights with unauthorized sender
        assert_eq!(
            admin.transfer(
                deps.as_mut(),
                candidate_addr.clone(),
                candidate_addr.clone()
            ),
            Err(ContractError::Unauthorized {})
        );

        // Transfer admin rights
        assert_eq!(
            admin.transfer(deps.as_mut(), admin_addr.clone(), candidate_addr.clone()),
            Ok(())
        );

        // New state
        assert_eq!(admin.current(deps.as_ref()), Ok(admin_addr.clone()));
        assert_eq!(
            admin.candidate(deps.as_ref()),
            Ok(Some(candidate_addr.clone()))
        );

        // Claim admin rights with unauthorized sender
        assert_eq!(
            admin.claim(deps.as_mut(), admin_addr.clone()),
            Err(ContractError::Unauthorized {})
        );

        assert_eq!(
            admin.claim(deps.as_mut(), random_addr.clone()),
            Err(ContractError::Unauthorized {})
        );

        // Claim admin rights
        assert_eq!(admin.claim(deps.as_mut(), candidate_addr.clone()), Ok(()));

        // New state
        assert_eq!(admin.current(deps.as_ref()), Ok(candidate_addr.clone()));
        assert_eq!(admin.candidate(deps.as_ref()), Ok(None));

        let new_admin_addr = candidate_addr;
        let old_admin_addr = admin_addr;

        // Transfer admin rights by new admin
        assert_eq!(
            admin.transfer(deps.as_mut(), new_admin_addr.clone(), random_addr.clone()),
            Ok(())
        );

        // Cancel transfer by non-admin
        assert_eq!(
            admin
                .cancel_transfer(deps.as_mut(), old_admin_addr.clone())
                .unwrap_err(),
            ContractError::Unauthorized {}
        );

        // Cancel transfer by admin
        assert_eq!(
            admin.cancel_transfer(deps.as_mut(), new_admin_addr.clone()),
            Ok(())
        );

        assert_eq!(
            admin.state.load(&deps.storage).unwrap(),
            AdminState::Claimed(new_admin_addr.clone())
        );

        // Cancel transfer by admin when not transferring
        assert_eq!(
            admin.cancel_transfer(deps.as_mut(), new_admin_addr.clone()),
            Err(ContractError::InoperableAdminTransferringState {})
        );

        // Transfer admin rights by new admin again
        assert_eq!(
            admin.transfer(deps.as_mut(), new_admin_addr.clone(), random_addr.clone()),
            Ok(())
        );

        // reject by non-candidate
        assert_eq!(
            admin
                .reject_transfer(deps.as_mut(), old_admin_addr.clone())
                .unwrap_err(),
            ContractError::Unauthorized {}
        );

        // reject by candidate
        assert_eq!(admin.reject_transfer(deps.as_mut(), random_addr), Ok(()));

        // reject by admin when not transferring
        assert_eq!(
            admin.reject_transfer(deps.as_mut(), new_admin_addr.clone()),
            Err(ContractError::InoperableAdminTransferringState {})
        );

        assert_eq!(
            admin.state.load(&deps.storage).unwrap(),
            AdminState::Claimed(new_admin_addr.clone())
        );
    }
}
