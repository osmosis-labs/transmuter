use cosmwasm_std::{Addr, DepsMut};

use crate::{ensure_admin_authority, ContractError};

pub mod admin;
pub mod moderator;

pub struct Role<'a> {
    pub admin: admin::Admin<'a>,
    pub moderator: moderator::Moderator<'a>,
}

impl<'a> Role<'a> {
    pub const fn new(admin_namespace: &'a str, moderator_namespace: &'a str) -> Self {
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
}
