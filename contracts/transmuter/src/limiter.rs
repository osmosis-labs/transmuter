use cosmwasm_schema::cw_serde;
use cosmwasm_std::{ensure, Decimal, Storage, Uint64};
use cw_storage_plus::Map;

use transmuter_math::rebalancing::params::RebalancingParams;

use crate::{scope::Scope, ContractError};

/// Maximum number of limiters allowed per denom.
/// This limited so that the contract can't be abused by setting a large number of limiters,
/// causing high gas usage when checking the limit, cleaning up divisions, etc.
const MAX_LIMITER_COUNT_PER_DENOM: Uint64 = Uint64::new(10u64);

#[cw_serde]
pub enum LimiterParams {
    StaticLimiter { upper_limit: Decimal },
}

pub struct RebalancingConfig {
    /// Map of (scope, label) -> Limiter
    config: Map<(&'static str, &'static str), RebalancingParams>,
}

impl RebalancingConfig {
    pub const fn new(rebalancing_config_namespace: &'static str) -> Self {
        Self {
            config: Map::new(rebalancing_config_namespace),
        }
    }

    pub fn register(
        &self,
        storage: &mut dyn Storage,
        scope: Scope,
        label: &str,
        limiter_params: LimiterParams,
    ) -> Result<(), ContractError> {
        let is_registering_entry_exists = self
            .config
            .may_load(storage, (&scope.key(), label))?
            .is_some();

        ensure!(!label.is_empty(), ContractError::EmptyLimiterLabel {});

        ensure!(
            !is_registering_entry_exists,
            ContractError::LimiterAlreadyExists {
                scope,
                label: label.to_string()
            }
        );

        let limiter = match limiter_params {
            LimiterParams::StaticLimiter { upper_limit } => {
                RebalancingParams::limit_only(upper_limit)?
            }
        };

        // ensure limiters for the denom has not yet reached the maximum
        let limiter_count_for_denom = self.list_by_scope(storage, &scope)?.len() as u64;
        ensure!(
            limiter_count_for_denom < MAX_LIMITER_COUNT_PER_DENOM.u64(),
            ContractError::MaxLimiterCountPerDenomExceeded {
                scope,
                max: MAX_LIMITER_COUNT_PER_DENOM
            }
        );

        self.config
            .save(storage, (&scope.key(), label), &limiter)
            .map_err(Into::into)
    }

    /// Deregsiter all limiters for the denom without checking if it will be empty.
    /// This is useful when the asset is being removed, so that limiters for the asset are no longer needed.
    pub fn uncheck_deregister_all_for_scope(
        &self,
        storage: &mut dyn Storage,
        scope: Scope,
    ) -> Result<(), ContractError> {
        let limiters = self.list_by_scope(storage, &scope)?;

        for (label, _) in limiters {
            self.config.remove(storage, (&scope.key(), &label));
        }

        Ok(())
    }

    /// Deregister a limiter without checking if it will be empty.
    /// This is useful when the scope is being removed, so that limiters for the scope are no longer needed.
    pub fn unchecked_deregister(
        &self,
        storage: &mut dyn Storage,
        scope: Scope,
        label: &str,
    ) -> Result<RebalancingParams, ContractError> {
        let scope_key = scope.key();
        match self.config.may_load(storage, (&scope_key, label))? {
            Some(rebalancing_params) => {
                self.config.remove(storage, (&scope_key, label));
                Ok(rebalancing_params)
            }
            None => Err(ContractError::LimiterDoesNotExist {
                scope,
                label: label.to_string(),
            }),
        }
    }

    pub fn deregister(
        &self,
        storage: &mut dyn Storage,
        scope: Scope,
        label: &str,
    ) -> Result<RebalancingParams, ContractError> {
        let scope_key = scope.key();
        match self.config.may_load(storage, (&scope_key, label))? {
            Some(rebalancing_params) => {
                let limiter_for_scope_will_not_be_empty =
                    self.list_by_scope(storage, &scope)?.len() >= 2;

                ensure!(
                    limiter_for_scope_will_not_be_empty,
                    ContractError::EmptyLimiterNotAllowed { scope }
                );

                self.config.remove(storage, (&scope_key, label));
                Ok(rebalancing_params)
            }
            None => Err(ContractError::LimiterDoesNotExist {
                scope,
                label: label.to_string(),
            }),
        }
    }

    // TODO: make this an update of the whole thing instead of just the limit
    pub fn set_limit(
        &self,
        storage: &mut dyn Storage,
        scope: Scope,
        label: &str,
        limit: Decimal,
    ) -> Result<(), ContractError> {
        dbg!(self.config.update(
            storage,
            (&scope.key(), label),
            |rebalancing_params: Option<RebalancingParams>| -> Result<RebalancingParams, ContractError> {
                let rebalancing_params = rebalancing_params.ok_or(ContractError::LimiterDoesNotExist {
                    scope,
                    label: label.to_string(),
                })?;

                Ok(RebalancingParams {
                    ideal_upper: rebalancing_params.ideal_upper.min(limit),
                    ideal_lower: rebalancing_params.ideal_lower.min(limit),
                    critical_upper: rebalancing_params.critical_upper.min(limit),
                    critical_lower: rebalancing_params.critical_lower.min(limit),
                    limit,
                    ..rebalancing_params
                })
            },
        )?);
        Ok(())
    }

    pub fn list_by_scope(
        &self,
        storage: &dyn Storage,
        scope: &Scope,
    ) -> Result<Vec<(String, RebalancingParams)>, ContractError> {
        // there is no need to limit, since the number of limiters is expected to be small
        self.config
            .prefix(&scope.key())
            .range(storage, None, None, cosmwasm_std::Order::Ascending)
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    #[allow(clippy::type_complexity)]
    pub fn list_limiters(
        &self,
        storage: &dyn Storage,
    ) -> Result<Vec<((String, String), RebalancingParams)>, ContractError> {
        // there is no need to limit, since the number of limiters is expected to be small
        self.config
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
                // Enforce limiter only if value is increasing, because if the value is decreasing from the previous value,
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
        fn test_register_limiter_works() {
            let mut deps = mock_dependencies();
            let limiter = RebalancingConfig::new("limiters");

            // register static limiter
            limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "static",
                    LimiterParams::StaticLimiter {
                        upper_limit: Decimal::percent(10),
                    },
                )
                .unwrap();

            assert_eq!(
                limiter.list_limiters(&deps.storage).unwrap(),
                vec![(
                    (Scope::denom("denoma").key(), "static".to_string()),
                    RebalancingParams::limit_only(Decimal::percent(10)).unwrap()
                ),]
            );

            // list limiters by denom
            assert_eq!(
                limiter
                    .list_by_scope(&deps.storage, &Scope::denom("denoma"))
                    .unwrap(),
                vec![(
                    "static".to_string(),
                    RebalancingParams::limit_only(Decimal::percent(10)).unwrap()
                )]
            );
        }

        #[test]
        fn test_register_with_empty_label_fails() {
            let mut deps = mock_dependencies();
            let limiter = RebalancingConfig::new("limiters");

            let err = limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "",
                    LimiterParams::StaticLimiter {
                        upper_limit: Decimal::percent(10),
                    },
                )
                .unwrap_err();

            assert_eq!(err, ContractError::EmptyLimiterLabel {});
        }

        #[test]
        fn test_register_same_key_fail() {
            let mut deps = mock_dependencies();
            let limiter = RebalancingConfig::new("limiters");

            limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "1m",
                    LimiterParams::StaticLimiter {
                        upper_limit: Decimal::percent(10),
                    },
                )
                .unwrap();

            let err = limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "1m",
                    LimiterParams::StaticLimiter {
                        upper_limit: Decimal::percent(10),
                    },
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::LimiterAlreadyExists {
                    scope: Scope::denom("denoma"),
                    label: "1m".to_string()
                }
            );
        }

        #[test]
        fn test_register_limiter_exceed_max_limiter_per_denom() {
            let mut deps = mock_dependencies();
            let limiter = RebalancingConfig::new("limiters");

            for h in 1..=10u64 {
                let label = format!("{}h", h);
                let result = limiter.register(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    &label,
                    LimiterParams::StaticLimiter {
                        upper_limit: Decimal::percent(10),
                    },
                );

                if h <= 10 {
                    assert!(result.is_ok());
                } else {
                    assert_eq!(
                        result.unwrap_err(),
                        ContractError::MaxLimiterCountPerDenomExceeded {
                            scope: Scope::denom("denoma"),
                            max: MAX_LIMITER_COUNT_PER_DENOM
                        }
                    );
                }
            }

            // deregister to register should work
            limiter
                .deregister(&mut deps.storage, Scope::denom("denoma"), "1h")
                .unwrap();

            // register static limiter
            limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "static",
                    LimiterParams::StaticLimiter {
                        upper_limit: Decimal::percent(10),
                    },
                )
                .unwrap();

            // register another one should fail
            let err = limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "static2",
                    LimiterParams::StaticLimiter {
                        upper_limit: Decimal::percent(9),
                    },
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::MaxLimiterCountPerDenomExceeded {
                    scope: Scope::denom("denoma"),
                    max: MAX_LIMITER_COUNT_PER_DENOM
                }
            );
        }

        #[test]
        fn test_deregister() {
            let mut deps = mock_dependencies();
            let limiter = RebalancingConfig::new("limiters");

            limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "a10",
                    LimiterParams::StaticLimiter {
                        upper_limit: Decimal::percent(10),
                    },
                )
                .unwrap();

            assert_eq!(
                limiter.list_limiters(&deps.storage).unwrap(),
                vec![(
                    (Scope::denom("denoma").key(), "a10".to_string()),
                    RebalancingParams::limit_only(Decimal::percent(10)).unwrap()
                )]
            );

            limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denomb"),
                    "b10",
                    LimiterParams::StaticLimiter {
                        upper_limit: Decimal::percent(10),
                    },
                )
                .unwrap();

            assert_eq!(
                limiter.list_limiters(&deps.storage).unwrap(),
                vec![
                    (
                        (Scope::denom("denoma").key(), "a10".to_string()),
                        RebalancingParams::limit_only(Decimal::percent(10)).unwrap()
                    ),
                    (
                        (Scope::denom("denomb").key(), "b10".to_string()),
                        RebalancingParams::limit_only(Decimal::percent(10)).unwrap()
                    )
                ]
            );

            let err = limiter
                .deregister(&mut deps.storage, Scope::denom("denoma"), "nonexistent")
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::LimiterDoesNotExist {
                    scope: Scope::denom("denoma"),
                    label: "nonexistent".to_string(),
                }
            );

            // Should fail: cannot remove the last limiter for denoma
            let err = limiter
                .deregister(&mut deps.storage, Scope::denom("denoma"), "a10")
                .unwrap_err();
            assert_eq!(
                err,
                ContractError::EmptyLimiterNotAllowed {
                    scope: Scope::denom("denoma")
                }
            );

            assert_eq!(
                limiter.list_limiters(&deps.storage).unwrap(),
                vec![
                    (
                        (Scope::denom("denoma").key(), "a10".to_string()),
                        RebalancingParams::limit_only(Decimal::percent(10)).unwrap()
                    ),
                    (
                        (Scope::denom("denomb").key(), "b10".to_string()),
                        RebalancingParams::limit_only(Decimal::percent(10)).unwrap()
                    )
                ]
            );

            // Add a second limiter to denoma, now removal is allowed
            limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "a10s",
                    LimiterParams::StaticLimiter {
                        upper_limit: Decimal::percent(10),
                    },
                )
                .unwrap();

            limiter
                .deregister(&mut deps.storage, Scope::denom("denoma"), "a10")
                .unwrap();

            assert_eq!(
                limiter.list_limiters(&deps.storage).unwrap(),
                vec![
                    (
                        (Scope::denom("denoma").key(), "a10s".to_string()),
                        RebalancingParams::limit_only(Decimal::percent(10)).unwrap()
                    ),
                    (
                        (Scope::denom("denomb").key(), "b10".to_string()),
                        RebalancingParams::limit_only(Decimal::percent(10)).unwrap()
                    )
                ]
            );

            // Should fail: cannot remove the last limiter for denomb
            let err = limiter
                .deregister(&mut deps.storage, Scope::denom("denomb"), "b10")
                .unwrap_err();
            assert_eq!(
                err,
                ContractError::EmptyLimiterNotAllowed {
                    scope: Scope::denom("denomb")
                }
            );

            // Add a second limiter to denomb, now removal is allowed
            limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denomb"),
                    "b10s",
                    LimiterParams::StaticLimiter {
                        upper_limit: Decimal::percent(10),
                    },
                )
                .unwrap();

            limiter
                .deregister(&mut deps.storage, Scope::denom("denomb"), "b10")
                .unwrap();

            assert_eq!(
                limiter.list_limiters(&deps.storage).unwrap(),
                vec![
                    (
                        (Scope::denom("denoma").key(), "a10s".to_string()),
                        RebalancingParams::limit_only(Decimal::percent(10)).unwrap()
                    ),
                    (
                        (Scope::denom("denomb").key(), "b10s".to_string()),
                        RebalancingParams::limit_only(Decimal::percent(10)).unwrap()
                    )
                ]
            );
        }

        #[test]
        fn test_unchecked_deregister() {
            let mut deps = mock_dependencies();
            let limiter = RebalancingConfig::new("limiters");

            // Register two limiters for denoma and one for denomb
            limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "1h",
                    LimiterParams::StaticLimiter {
                        upper_limit: Decimal::percent(10),
                    },
                )
                .unwrap();

            // Unchecked deregister one limiter from denoma
            let removed_limiter = limiter
                .unchecked_deregister(&mut deps.storage, Scope::denom("denoma"), "1h")
                .unwrap();

            // Check that the removed limiter is correct
            assert_eq!(
                removed_limiter,
                RebalancingParams::limit_only(Decimal::percent(10)).unwrap()
            );

            // Check that the remaining limiters are correct
            assert_eq!(limiter.list_limiters(&deps.storage).unwrap(), vec![]);
        }
    }
}
