use cosmwasm_std::{Addr, DepsMut};

use crate::{ensure_admin_authority, ContractError};

pub mod admin;
pub mod moderator;

pub struct Role {
    pub admin: admin::Admin,
    pub moderator: moderator::Moderator,
}

impl Role {
    pub const fn new(admin_namespace: &'static str, moderator_namespace: &'static str) -> Self {
        Role {
            admin: admin::Admin::new(admin_namespace),
            moderator: moderator::Moderator::new(moderator_namespace),
        }
    }

    /// Only admin can assign moderator
    pub fn assign_moderator(
        &self,
        sender: Addr,
        deps: DepsMut,
        address: Addr,
    ) -> Result<(), ContractError> {
        // ensure that only admin can assign moderator
        ensure_admin_authority!(sender, self.admin, deps.as_ref());

        self.moderator.unchecked_set(deps, address)
    }
}

/// Ensure that the sender is either the current admin or the current moderator
///
/// This macro ensures that the sender is either the current admin or the current moderator.
/// It is used to protect sensitive operations that should be performed by either role.
///
/// If the `sender_address` is neither the current admin nor the current moderator,
/// the macro will return an `Err(ContractError::Unauthorized {})`.
#[macro_export]
macro_rules! ensure_admin_or_moderator_authority {
    ($sender:expr, $admin: expr, $moderator: expr, $deps:expr) => {
        let current_admin = $admin.current($deps)?;
        let current_moderator = $moderator.get($deps).ok();

        if $sender != current_admin && Some($sender.clone()) != current_moderator {
            return Err($crate::ContractError::Unauthorized {});
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmwasm_std::testing::mock_dependencies;
    use cosmwasm_std::Addr;

    #[test]
    fn test_assign_remove_moderator() {
        let mut deps = mock_dependencies();
        let admin = Addr::unchecked("admin");
        let moderator = Addr::unchecked("moderator");
        let non_admin = Addr::unchecked("non_admin");

        let role = Role::new("admin", "moderator");

        role.admin.init(&mut deps.storage, admin.clone()).unwrap();

        // Test assign moderator by admin
        role.assign_moderator(admin.clone(), deps.as_mut(), moderator.clone())
            .unwrap();

        assert_eq!(role.moderator.get(deps.as_ref()).unwrap(), moderator);

        // Test assign moderator by non-admin
        let err = role
            .assign_moderator(non_admin.clone(), deps.as_mut(), moderator)
            .unwrap_err();

        assert_eq!(err, ContractError::Unauthorized {});
    }

    #[test]
    fn test_ensure_admin_or_moderator_authority() {
        let mut deps = mock_dependencies();
        let admin = Addr::unchecked("admin");
        let moderator = Addr::unchecked("moderator");
        let random_user = Addr::unchecked("random_user");

        let role = Role::new("admin", "moderator");

        // Initialize admin and moderator
        role.admin.init(&mut deps.storage, admin.clone()).unwrap();
        role.assign_moderator(admin.clone(), deps.as_mut(), moderator.clone())
            .unwrap();

        fn test_access(
            sender: Addr,
            role: &Role,
            deps: cosmwasm_std::Deps,
        ) -> Result<(), ContractError> {
            ensure_admin_or_moderator_authority!(sender, role.admin, role.moderator, deps);
            Ok(())
        }

        // Admin should have access
        assert!(test_access(admin.clone(), &role, deps.as_ref()).is_ok());

        // Moderator should have access
        assert!(test_access(moderator.clone(), &role, deps.as_ref()).is_ok());

        // Random user should not have access
        let err = test_access(random_user, &role, deps.as_ref()).unwrap_err();
        assert_eq!(err, ContractError::Unauthorized {});
    }
}
