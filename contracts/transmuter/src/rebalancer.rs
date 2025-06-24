use cosmwasm_std::{ensure, Decimal, Storage, Uint64};
use cw_storage_plus::Map;

use transmuter_math::rebalancing::config::RebalancingConfig;

use crate::{scope::Scope, ContractError};

/// Maximum number of rebalancing configs allowed per denom.
/// This limited so that the contract can't be abused by setting a large number of configs,
/// causing high gas usage when checking the limit, cleaning up divisions, etc.
const MAX_CONFIG_COUNT_PER_DENOM: Uint64 = Uint64::new(10u64);

pub struct Rebalancer {
    /// Map of (scope, label) -> RebalancingConfig
    configs: Map<(&'static str, &'static str), RebalancingConfig>,
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
        label: &str,
        rebalancing_params: RebalancingConfig,
    ) -> Result<(), ContractError> {
        let is_config_exists = self
            .configs
            .may_load(storage, (&scope.key(), label))?
            .is_some();

        ensure!(!label.is_empty(), ContractError::EmptyConfigLabel {});

        ensure!(
            !is_config_exists,
            ContractError::ConfigAlreadyExists {
                scope,
                label: label.to_string()
            }
        );

        // ensure configs for the denom has not yet reached the maximum
        // TODO: remove this when there is only one per denom / asset group
        let config_count_for_denom = self.list_by_scope(storage, &scope)?.len() as u64;
        ensure!(
            config_count_for_denom < MAX_CONFIG_COUNT_PER_DENOM.u64(),
            ContractError::MaxLimiterCountPerDenomExceeded {
                scope,
                max: MAX_CONFIG_COUNT_PER_DENOM
            }
        );

        self.configs
            .save(storage, (&scope.key(), label), &rebalancing_params)
            .map_err(Into::into)
    }

    /// Remove all rebalancing configs for the denom without checking if it will be empty.
    /// This is useful when the asset is being removed, so that configs for the asset are no longer needed.
    pub fn uncheck_remove_all_configs_for_scope(
        &self,
        storage: &mut dyn Storage,
        scope: Scope,
    ) -> Result<(), ContractError> {
        let configs = self.list_by_scope(storage, &scope)?;

        for (label, _) in configs {
            self.configs.remove(storage, (&scope.key(), &label));
        }

        Ok(())
    }

    /// Remove a rebalancing config without checking if it will be empty.
    /// This is useful when the scope is being removed, so that configs for the scope are no longer needed.
    pub fn unchecked_remove_config(
        &self,
        storage: &mut dyn Storage,
        scope: Scope,
        label: &str,
    ) -> Result<RebalancingConfig, ContractError> {
        let scope_key = scope.key();
        match self.configs.may_load(storage, (&scope_key, label))? {
            Some(rebalancing_params) => {
                self.configs.remove(storage, (&scope_key, label));
                Ok(rebalancing_params)
            }
            None => Err(ContractError::ConfigDoesNotExist {
                scope,
                label: label.to_string(),
            }),
        }
    }

    pub fn remove_config(
        &self,
        storage: &mut dyn Storage,
        scope: Scope,
        label: &str,
    ) -> Result<RebalancingConfig, ContractError> {
        let scope_key = scope.key();
        match self.configs.may_load(storage, (&scope_key, label))? {
            Some(rebalancing_params) => {
                let config_for_scope_will_not_be_empty =
                    self.list_by_scope(storage, &scope)?.len() >= 2;

                ensure!(
                    config_for_scope_will_not_be_empty,
                    ContractError::EmptyLimiterNotAllowed { scope }
                );

                self.configs.remove(storage, (&scope_key, label));
                Ok(rebalancing_params)
            }
            None => Err(ContractError::ConfigDoesNotExist {
                scope,
                label: label.to_string(),
            }),
        }
    }

    pub fn update_config(
        &self,
        storage: &mut dyn Storage,
        scope: Scope,
        label: &str,
        rebalancing_config: &RebalancingConfig,
    ) -> Result<(), ContractError> {
        // check if it exists, if not, return an error
        let is_config_exists = self
            .configs
            .may_load(storage, (&scope.key(), label))?
            .is_some();

        ensure!(
            is_config_exists,
            ContractError::ConfigDoesNotExist {
                scope,
                label: label.to_string()
            }
        );

        self.configs
            .save(storage, (&scope.key(), label), rebalancing_config)
            .map_err(Into::into)
    }

    pub fn list_by_scope(
        &self,
        storage: &dyn Storage,
        scope: &Scope,
    ) -> Result<Vec<(String, RebalancingConfig)>, ContractError> {
        // there is no need to limit, since the number of configs is expected to be small
        self.configs
            .prefix(&scope.key())
            .range(storage, None, None, cosmwasm_std::Order::Ascending)
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    #[allow(clippy::type_complexity)]
    pub fn list_configs(
        &self,
        storage: &dyn Storage,
    ) -> Result<Vec<((String, String), RebalancingConfig)>, ContractError> {
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
            let rebalancing_params = self.list_by_scope(storage, &scope)?;
            let is_not_decreasing = value >= prev_value;

            for (_, rebalancing_params) in rebalancing_params {
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
                    "static",
                    RebalancingConfig::limit_only(Decimal::percent(10)).unwrap(),
                )
                .unwrap();

            assert_eq!(
                rebalancer.list_configs(&deps.storage).unwrap(),
                vec![(
                    (Scope::denom("denoma").key(), "static".to_string()),
                    RebalancingConfig::limit_only(Decimal::percent(10)).unwrap()
                ),]
            );

            // list rebalancing configs by denom
            assert_eq!(
                rebalancer
                    .list_by_scope(&deps.storage, &Scope::denom("denoma"))
                    .unwrap(),
                vec![(
                    "static".to_string(),
                    RebalancingConfig::limit_only(Decimal::percent(10)).unwrap()
                )]
            );
        }

        #[test]
        fn test_add_with_empty_label_fails() {
            let mut deps = mock_dependencies();
            let rebalancer = Rebalancer::new("rebalancing_configs");

            let err = rebalancer
                .add_config(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "",
                    RebalancingConfig::limit_only(Decimal::percent(10)).unwrap(),
                )
                .unwrap_err();

            assert_eq!(err, ContractError::EmptyConfigLabel {});
        }

        #[test]
        fn test_add_same_key_fail() {
            let mut deps = mock_dependencies();
            let rebalancer = Rebalancer::new("rebalancer");

            rebalancer
                .add_config(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "1m",
                    RebalancingConfig::limit_only(Decimal::percent(10)).unwrap(),
                )
                .unwrap();

            let err = rebalancer
                .add_config(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "1m",
                    RebalancingConfig::limit_only(Decimal::percent(10)).unwrap(),
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::ConfigAlreadyExists {
                    scope: Scope::denom("denoma"),
                    label: "1m".to_string()
                }
            );
        }

        #[test]
        fn test_add_config_exceed_max_config_per_denom() {
            let mut deps = mock_dependencies();
            let rebalancer = Rebalancer::new("rebalancer");

            for h in 1..=10u64 {
                let label = format!("{}h", h);
                let result = rebalancer.add_config(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    &label,
                    RebalancingConfig::limit_only(Decimal::percent(10)).unwrap(),
                );

                if h <= 10 {
                    assert!(result.is_ok());
                } else {
                    assert_eq!(
                        result.unwrap_err(),
                        ContractError::MaxLimiterCountPerDenomExceeded {
                            scope: Scope::denom("denoma"),
                            max: MAX_CONFIG_COUNT_PER_DENOM
                        }
                    );
                }
            }

            // deregister to register should work
            rebalancer
                .remove_config(&mut deps.storage, Scope::denom("denoma"), "1h")
                .unwrap();

            // register static rebalancing config
            rebalancer
                .add_config(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "static",
                    RebalancingConfig::limit_only(Decimal::percent(10)).unwrap(),
                )
                .unwrap();

            // register another one should fail
            let err = rebalancer
                .add_config(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "static2",
                    RebalancingConfig::limit_only(Decimal::percent(9)).unwrap(),
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::MaxLimiterCountPerDenomExceeded {
                    scope: Scope::denom("denoma"),
                    max: MAX_CONFIG_COUNT_PER_DENOM
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
                    "a10",
                    RebalancingConfig::limit_only(Decimal::percent(10)).unwrap(),
                )
                .unwrap();

            assert_eq!(
                rebalancer.list_configs(&deps.storage).unwrap(),
                vec![(
                    (Scope::denom("denoma").key(), "a10".to_string()),
                    RebalancingConfig::limit_only(Decimal::percent(10)).unwrap()
                )]
            );

            rebalancer
                .add_config(
                    &mut deps.storage,
                    Scope::denom("denomb"),
                    "b10",
                    RebalancingConfig::limit_only(Decimal::percent(10)).unwrap(),
                )
                .unwrap();

            assert_eq!(
                rebalancer.list_configs(&deps.storage).unwrap(),
                vec![
                    (
                        (Scope::denom("denoma").key(), "a10".to_string()),
                        RebalancingConfig::limit_only(Decimal::percent(10)).unwrap()
                    ),
                    (
                        (Scope::denom("denomb").key(), "b10".to_string()),
                        RebalancingConfig::limit_only(Decimal::percent(10)).unwrap()
                    )
                ]
            );

            let err = rebalancer
                .remove_config(&mut deps.storage, Scope::denom("denoma"), "nonexistent")
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::ConfigDoesNotExist {
                    scope: Scope::denom("denoma"),
                    label: "nonexistent".to_string(),
                }
            );

            // Should fail: cannot remove the last rebalancing config for denoma
            let err = rebalancer
                .remove_config(&mut deps.storage, Scope::denom("denoma"), "a10")
                .unwrap_err();
            assert_eq!(
                err,
                ContractError::EmptyLimiterNotAllowed {
                    scope: Scope::denom("denoma")
                }
            );

            assert_eq!(
                rebalancer.list_configs(&deps.storage).unwrap(),
                vec![
                    (
                        (Scope::denom("denoma").key(), "a10".to_string()),
                        RebalancingConfig::limit_only(Decimal::percent(10)).unwrap()
                    ),
                    (
                        (Scope::denom("denomb").key(), "b10".to_string()),
                        RebalancingConfig::limit_only(Decimal::percent(10)).unwrap()
                    )
                ]
            );

            // Add a second rebalancing config to denoma, now removal is allowed
            rebalancer
                .add_config(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "a10s",
                    RebalancingConfig::limit_only(Decimal::percent(10)).unwrap(),
                )
                .unwrap();

            rebalancer
                .remove_config(&mut deps.storage, Scope::denom("denoma"), "a10")
                .unwrap();

            assert_eq!(
                rebalancer.list_configs(&deps.storage).unwrap(),
                vec![
                    (
                        (Scope::denom("denoma").key(), "a10s".to_string()),
                        RebalancingConfig::limit_only(Decimal::percent(10)).unwrap()
                    ),
                    (
                        (Scope::denom("denomb").key(), "b10".to_string()),
                        RebalancingConfig::limit_only(Decimal::percent(10)).unwrap()
                    )
                ]
            );

            // Should fail: cannot remove the last rebalancing config for denomb
            let err = rebalancer
                .remove_config(&mut deps.storage, Scope::denom("denomb"), "b10")
                .unwrap_err();
            assert_eq!(
                err,
                ContractError::EmptyLimiterNotAllowed {
                    scope: Scope::denom("denomb")
                }
            );

            // Add a second rebalancing config to denomb, now removal is allowed
            rebalancer
                .add_config(
                    &mut deps.storage,
                    Scope::denom("denomb"),
                    "b10s",
                    RebalancingConfig::limit_only(Decimal::percent(10)).unwrap(),
                )
                .unwrap();

            rebalancer
                .remove_config(&mut deps.storage, Scope::denom("denomb"), "b10")
                .unwrap();

            assert_eq!(
                rebalancer.list_configs(&deps.storage).unwrap(),
                vec![
                    (
                        (Scope::denom("denoma").key(), "a10s".to_string()),
                        RebalancingConfig::limit_only(Decimal::percent(10)).unwrap()
                    ),
                    (
                        (Scope::denom("denomb").key(), "b10s".to_string()),
                        RebalancingConfig::limit_only(Decimal::percent(10)).unwrap()
                    )
                ]
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
                    "1h",
                    RebalancingConfig::limit_only(Decimal::percent(10)).unwrap(),
                )
                .unwrap();

            // Unchecked deregister one rebalancing config from denoma
            let removed_config = rebalancer
                .unchecked_remove_config(&mut deps.storage, Scope::denom("denoma"), "1h")
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
