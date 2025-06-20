use cosmwasm_schema::cw_serde;
use cosmwasm_std::{ensure, Decimal, Storage, Uint64};
use cw_storage_plus::Map;

use crate::{scope::Scope, ContractError};

/// Maximum number of limiters allowed per denom.
/// This limited so that the contract can't be abused by setting a large number of limiters,
/// causing high gas usage when checking the limit, cleaning up divisions, etc.
const MAX_LIMITER_COUNT_PER_DENOM: Uint64 = Uint64::new(10u64);

/// Limiter that determines limit by upper bound of the value.
#[cw_serde]
pub struct StaticLimiter {
    /// Upper limit of the value
    upper_limit: Decimal,
}

impl StaticLimiter {
    pub fn new(upper_limit: Decimal) -> Result<Self, ContractError> {
        Self { upper_limit }.ensure_upper_limit_constraint()
    }

    fn ensure_upper_limit_constraint(self) -> Result<Self, ContractError> {
        ensure!(
            self.upper_limit > Decimal::zero(),
            ContractError::ZeroUpperLimit {}
        );

        ensure!(
            self.upper_limit <= Decimal::percent(100),
            ContractError::ExceedHundredPercentUpperLimit {}
        );

        Ok(self)
    }

    fn ensure_upper_limit(self, scope: &Scope, value: Decimal) -> Result<Self, ContractError> {
        ensure!(
            value <= self.upper_limit,
            ContractError::UpperLimitExceeded {
                scope: scope.clone(),
                upper_limit: self.upper_limit,
                value,
            }
        );

        Ok(self)
    }

    fn set_upper_limit(self, upper_limit: Decimal) -> Result<Self, ContractError> {
        Self { upper_limit }.ensure_upper_limit_constraint()
    }
}

#[cw_serde]
pub enum Limiter {
    StaticLimiter(StaticLimiter),
}

#[cw_serde]
pub enum LimiterParams {
    StaticLimiter { upper_limit: Decimal },
}

pub struct Limiters {
    /// Map of (scope, label) -> Limiter
    limiters: Map<(&'static str, &'static str), Limiter>,
}

impl Limiters {
    pub const fn new(limiters_namespace: &'static str) -> Self {
        Self {
            limiters: Map::new(limiters_namespace),
        }
    }

    pub fn register(
        &self,
        storage: &mut dyn Storage,
        scope: Scope,
        label: &str,
        limiter_params: LimiterParams,
    ) -> Result<(), ContractError> {
        let is_registering_limiter_exists = self
            .limiters
            .may_load(storage, (&scope.key(), label))?
            .is_some();

        ensure!(!label.is_empty(), ContractError::EmptyLimiterLabel {});

        ensure!(
            !is_registering_limiter_exists,
            ContractError::LimiterAlreadyExists {
                scope,
                label: label.to_string()
            }
        );

        let limiter = match limiter_params {
            LimiterParams::StaticLimiter { upper_limit } => {
                Limiter::StaticLimiter(StaticLimiter::new(upper_limit)?)
            }
        };

        // ensure limiters for the denom has not yet reached the maximum
        let limiter_count_for_denom = self.list_limiters_by_scope(storage, &scope)?.len() as u64;
        ensure!(
            limiter_count_for_denom < MAX_LIMITER_COUNT_PER_DENOM.u64(),
            ContractError::MaxLimiterCountPerDenomExceeded {
                scope,
                max: MAX_LIMITER_COUNT_PER_DENOM
            }
        );

        self.limiters
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
        let limiters = self.list_limiters_by_scope(storage, &scope)?;

        for (label, _) in limiters {
            self.limiters.remove(storage, (&scope.key(), &label));
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
    ) -> Result<Limiter, ContractError> {
        let scope_key = scope.key();
        match self.limiters.may_load(storage, (&scope_key, label))? {
            Some(limiter) => {
                self.limiters.remove(storage, (&scope_key, label));
                Ok(limiter)
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
    ) -> Result<Limiter, ContractError> {
        let scope_key = scope.key();
        match self.limiters.may_load(storage, (&scope_key, label))? {
            Some(limiter) => {
                let limiter_for_scope_will_not_be_empty =
                    self.list_limiters_by_scope(storage, &scope)?.len() >= 2;

                ensure!(
                    limiter_for_scope_will_not_be_empty,
                    ContractError::EmptyLimiterNotAllowed { scope }
                );

                self.limiters.remove(storage, (&scope_key, label));
                Ok(limiter)
            }
            None => Err(ContractError::LimiterDoesNotExist {
                scope,
                label: label.to_string(),
            }),
        }
    }

    /// Set upper limit for a [`StaticLimiter`] only, otherwise it will fail.
    pub fn set_static_limiter_upper_limit(
        &self,
        storage: &mut dyn Storage,
        scope: Scope,
        label: &str,
        upper_limit: Decimal,
    ) -> Result<(), ContractError> {
        self.limiters.update(
            storage,
            (&scope.key(), label),
            |limiter: Option<Limiter>| -> Result<Limiter, ContractError> {
                let limiter = limiter.ok_or(ContractError::LimiterDoesNotExist {
                    scope,
                    label: label.to_string(),
                })?;

                // check if the limiter is a StaticLimiter
                match limiter {
                    Limiter::StaticLimiter(limiter) => Ok(Limiter::StaticLimiter(
                        limiter.set_upper_limit(upper_limit)?,
                    )),
                }
            },
        )?;
        Ok(())
    }

    pub fn list_limiters_by_scope(
        &self,
        storage: &dyn Storage,
        scope: &Scope,
    ) -> Result<Vec<(String, Limiter)>, ContractError> {
        // there is no need to limit, since the number of limiters is expected to be small
        self.limiters
            .prefix(&scope.key())
            .range(storage, None, None, cosmwasm_std::Order::Ascending)
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    #[allow(clippy::type_complexity)]
    pub fn list_limiters(
        &self,
        storage: &dyn Storage,
    ) -> Result<Vec<((String, String), Limiter)>, ContractError> {
        // there is no need to limit, since the number of limiters is expected to be small
        self.limiters
            .range(storage, None, None, cosmwasm_std::Order::Ascending)
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn check_limits_and_update(
        &self,
        storage: &mut dyn Storage,
        scope_value_pairs: Vec<(Scope, (Decimal, Decimal))>,
    ) -> Result<(), ContractError> {
        for (scope, (prev_value, value)) in scope_value_pairs {
            let limiters = self.list_limiters_by_scope(storage, &scope)?;
            let is_not_decreasing = value >= prev_value;

            for (label, limiter) in limiters {
                // Enforce limiter only if value is increasing, because if the value is decreasing from the previous value,
                // for the specific scope, it is a balancing act to move away from the limit.
                let limiter = match limiter {
                    Limiter::StaticLimiter(limiter) => Limiter::StaticLimiter({
                        if is_not_decreasing {
                            limiter.ensure_upper_limit(&scope, value)?
                        } else {
                            limiter
                        }
                    }),
                };

                // save updated limiter
                self.limiters
                    .save(storage, (&scope.key(), &label), &limiter)?;
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
            let limiter = Limiters::new("limiters");

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
                    Limiter::StaticLimiter(StaticLimiter {
                        upper_limit: Decimal::percent(10)
                    })
                ),]
            );

            // list limiters by denom
            assert_eq!(
                limiter
                    .list_limiters_by_scope(&deps.storage, &Scope::denom("denoma"))
                    .unwrap(),
                vec![(
                    "static".to_string(),
                    Limiter::StaticLimiter(StaticLimiter {
                        upper_limit: Decimal::percent(10)
                    })
                )]
            );
        }

        #[test]
        fn test_register_with_empty_label_fails() {
            let mut deps = mock_dependencies();
            let limiter = Limiters::new("limiters");

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
            let limiter = Limiters::new("limiters");

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
            let limiter = Limiters::new("limiters");

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
            let limiter = Limiters::new("limiters");

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
                    Limiter::StaticLimiter(StaticLimiter {
                        upper_limit: Decimal::percent(10),
                    })
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
                        Limiter::StaticLimiter(StaticLimiter {
                            upper_limit: Decimal::percent(10),
                        })
                    ),
                    (
                        (Scope::denom("denomb").key(), "b10".to_string()),
                        Limiter::StaticLimiter(StaticLimiter {
                            upper_limit: Decimal::percent(10),
                        })
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
                        Limiter::StaticLimiter(StaticLimiter {
                            upper_limit: Decimal::percent(10),
                        })
                    ),
                    (
                        (Scope::denom("denomb").key(), "b10".to_string()),
                        Limiter::StaticLimiter(StaticLimiter {
                            upper_limit: Decimal::percent(10),
                        })
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
                        Limiter::StaticLimiter(StaticLimiter {
                            upper_limit: Decimal::percent(10),
                        })
                    ),
                    (
                        (Scope::denom("denomb").key(), "b10".to_string()),
                        Limiter::StaticLimiter(StaticLimiter {
                            upper_limit: Decimal::percent(10),
                        })
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
                        Limiter::StaticLimiter(StaticLimiter {
                            upper_limit: Decimal::percent(10),
                        })
                    ),
                    (
                        (Scope::denom("denomb").key(), "b10s".to_string()),
                        Limiter::StaticLimiter(StaticLimiter {
                            upper_limit: Decimal::percent(10),
                        })
                    )
                ]
            );
        }

        #[test]
        fn test_unchecked_deregister() {
            let mut deps = mock_dependencies();
            let limiter = Limiters::new("limiters");

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
                Limiter::StaticLimiter(StaticLimiter {
                    upper_limit: Decimal::percent(10),
                })
            );

            // Check that the remaining limiters are correct
            assert_eq!(limiter.list_limiters(&deps.storage).unwrap(), vec![]);
        }
    }

    mod pararm_validation {
        use super::*;

        #[test]
        fn static_limiter_validation() {
            // upper limit is zero
            assert_eq!(
                StaticLimiter::new(Decimal::zero()).unwrap_err(),
                ContractError::ZeroUpperLimit {}
            );

            // set upper limit to zero
            assert_eq!(
                StaticLimiter::new(Decimal::percent(10))
                    .unwrap()
                    .set_upper_limit(Decimal::zero())
                    .unwrap_err(),
                ContractError::ZeroUpperLimit {}
            );

            // upper limit is 100% + Decimal::raw(1)
            assert_eq!(
                StaticLimiter::new(Decimal::percent(100) + Decimal::raw(1u128)).unwrap_err(),
                ContractError::ExceedHundredPercentUpperLimit {}
            );

            // set upper limit to 100% + Decimal::raw(1)
            assert_eq!(
                StaticLimiter::new(Decimal::percent(10))
                    .unwrap()
                    .set_upper_limit(Decimal::percent(100) + Decimal::raw(1u128))
                    .unwrap_err(),
                ContractError::ExceedHundredPercentUpperLimit {}
            );
        }
    }
}
