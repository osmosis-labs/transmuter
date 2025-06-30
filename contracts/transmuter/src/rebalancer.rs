use cosmwasm_std::{ensure, Decimal, Storage};
use cw_storage_plus::Map;

use transmuter_math::rebalancing::config::RebalancingConfig;

use crate::{scope::Scope, ContractError};

pub struct Rebalancer {
    /// Map of scope -> RebalancingConfig
    configs: Map<&'static str, RebalancingConfig>,
}

impl Rebalancer {
    pub const fn new(rebalancing_config_namespace: &'static str) -> Self {
        Self {
            configs: Map::new(rebalancing_config_namespace),
        }
    }

    pub fn add_config(
        &self,
        storage: &mut dyn Storage,
        scope: Scope,
        rebalancing_config: RebalancingConfig,
    ) -> Result<(), ContractError> {
        let is_config_exists = self.configs.may_load(storage, &scope.key())?.is_some();

        ensure!(
            !is_config_exists,
            ContractError::ConfigAlreadyExists { scope }
        );

        self.configs
            .save(storage, &scope.key(), &rebalancing_config)
            .map_err(Into::into)
    }

    /// Remove a rebalancing config without checking if it will be empty.
    /// This is useful when the scope is being removed, so that configs for the scope are no longer needed.
    pub fn unchecked_remove_config(
        &self,
        storage: &mut dyn Storage,
        scope: Scope,
    ) -> Result<RebalancingConfig, ContractError> {
        let scope_key = scope.key();
        match self.configs.may_load(storage, &scope_key)? {
            Some(rebalancing_params) => {
                self.configs.remove(storage, &scope_key);
                Ok(rebalancing_params)
            }
            None => Err(ContractError::ConfigDoesNotExist { scope }),
        }
    }

    pub fn remove_config(
        &self,
        storage: &mut dyn Storage,
        scope: Scope,
    ) -> Result<RebalancingConfig, ContractError> {
        let scope_key = scope.key();
        match self.configs.may_load(storage, &scope_key)? {
            Some(rebalancing_params) => {
                self.configs.remove(storage, &scope_key);
                Ok(rebalancing_params)
            }
            None => Err(ContractError::ConfigDoesNotExist { scope }),
        }
    }

    pub fn update_config(
        &self,
        storage: &mut dyn Storage,
        scope: Scope,
        rebalancing_config: &RebalancingConfig,
    ) -> Result<(), ContractError> {
        // check if it exists, if not, return an error
        let is_config_exists = self.configs.may_load(storage, &scope.key())?.is_some();

        ensure!(
            is_config_exists,
            ContractError::ConfigDoesNotExist { scope }
        );

        self.configs
            .save(storage, &scope.key(), rebalancing_config)
            .map_err(Into::into)
    }

    pub fn get_config_by_scope(
        &self,
        storage: &dyn Storage,
        scope: &Scope,
    ) -> Result<Option<RebalancingConfig>, ContractError> {
        self.configs
            .may_load(storage, &scope.key())
            .map_err(Into::into)
    }

    pub fn list_configs(
        &self,
        storage: &dyn Storage,
    ) -> Result<Vec<(String, RebalancingConfig)>, ContractError> {
        // there is no need to limit, since the number of configs is expected to be small
        self.configs
            .range(storage, None, None, cosmwasm_std::Order::Ascending)
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn check_limits(
        &self,
        storage: &mut dyn Storage,
        scope_value_pairs: Vec<(Scope, (Decimal, Decimal))>,
    ) -> Result<(), ContractError> {
        for (scope, (prev_value, value)) in scope_value_pairs {
            let Some(rebalancing_params) = self.get_config_by_scope(storage, &scope)? else {
                continue;
            };

            let is_not_decreasing = value >= prev_value;

            // Enforce limit only if value is increasing, because if the value is decreasing from the previous value,
            // for the specific scope, it is a balancing act to move away from the limit.
            if is_not_decreasing && value > rebalancing_params.limit {
                return Err(ContractError::UpperLimitExceeded {
                    scope: scope.clone(),
                    upper_limit: rebalancing_params.limit,
                    value,
                });
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use cosmwasm_std::testing::mock_dependencies;

    use super::*;

    mod registration {
        use super::*;

        #[test]
        fn test_add_config_works() {
            let mut deps = mock_dependencies();
            let rebalancer = Rebalancer::new("rebalancing_configs");

            // register static rebalancing config
            rebalancer
                .add_config(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    RebalancingConfig::limit_only(Decimal::percent(10)).unwrap(),
                )
                .unwrap();

            assert_eq!(
                rebalancer.list_configs(&deps.storage).unwrap(),
                vec![(
                    Scope::denom("denoma").key(),
                    RebalancingConfig::limit_only(Decimal::percent(10)).unwrap()
                )]
            );

            // list rebalancing configs by denom
            assert_eq!(
                rebalancer
                    .get_config_by_scope(&deps.storage, &Scope::denom("denoma"))
                    .unwrap(),
                Some(RebalancingConfig::limit_only(Decimal::percent(10)).unwrap())
            );
        }

        #[test]
        fn test_add_same_key_fail() {
            let mut deps = mock_dependencies();
            let rebalancer = Rebalancer::new("rebalancer");

            rebalancer
                .add_config(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    RebalancingConfig::limit_only(Decimal::percent(10)).unwrap(),
                )
                .unwrap();

            let err = rebalancer
                .add_config(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    RebalancingConfig::limit_only(Decimal::percent(10)).unwrap(),
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::ConfigAlreadyExists {
                    scope: Scope::denom("denoma"),
                }
            );
        }

        #[test]
        fn test_remove_config() {
            let mut deps = mock_dependencies();
            let rebalancer = Rebalancer::new("rebalancer");

            rebalancer
                .add_config(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    RebalancingConfig::limit_only(Decimal::percent(10)).unwrap(),
                )
                .unwrap();

            assert_eq!(
                rebalancer.list_configs(&deps.storage).unwrap(),
                vec![(
                    Scope::denom("denoma").key(),
                    RebalancingConfig::limit_only(Decimal::percent(10)).unwrap()
                )]
            );

            // Remove the config
            rebalancer
                .remove_config(&mut deps.storage, Scope::denom("denoma"))
                .unwrap();

            // Now it should be gone
            assert_eq!(rebalancer.list_configs(&deps.storage).unwrap(), vec![]);

            // Removing again should return ConfigDoesNotExist
            let err = rebalancer
                .remove_config(&mut deps.storage, Scope::denom("denoma"))
                .unwrap_err();
            assert_eq!(
                err,
                ContractError::ConfigDoesNotExist {
                    scope: Scope::denom("denoma"),
                }
            );
        }

        #[test]
        fn test_unchecked_remove_config() {
            let mut deps = mock_dependencies();
            let rebalancer = Rebalancer::new("rebalancer");

            // Register two rebalancing configs for denoma and one for denomb
            rebalancer
                .add_config(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    RebalancingConfig::limit_only(Decimal::percent(10)).unwrap(),
                )
                .unwrap();

            // Unchecked deregister one rebalancing config from denoma
            let removed_config = rebalancer
                .unchecked_remove_config(&mut deps.storage, Scope::denom("denoma"))
                .unwrap();

            // Check that the removed rebalancing config is correct
            assert_eq!(
                removed_config,
                RebalancingConfig::limit_only(Decimal::percent(10)).unwrap()
            );

            // Check that the remaining rebalancing configs are correct
            assert_eq!(rebalancer.list_configs(&deps.storage).unwrap(), vec![]);
        }
    }
}
