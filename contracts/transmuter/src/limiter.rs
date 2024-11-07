use std::collections::{BTreeMap, HashMap};

use crate::{scope::Scope, ContractError};
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{ensure, Decimal, StdError, Storage, Timestamp, Uint64};
use cw_storage_plus::Map;
use transmuter_math::Division;

/// Maximum number of divisions allowed in a window.
/// This limited so that the contract can't be abused by setting a large division count,
/// which will cause high gas usage when checking the limit, cleaning up divisions, etc.
const MAX_DIVISION_COUNT: Uint64 = Uint64::new(10u64);

/// Maximum number of limiters allowed per denom.
/// This limited so that the contract can't be abused by setting a large number of limiters,
/// causing high gas usage when checking the limit, cleaning up divisions, etc.
const MAX_LIMITER_COUNT_PER_DENOM: Uint64 = Uint64::new(10u64);

#[cw_serde]
pub struct WindowConfig {
    /// Size of the window in nanoseconds
    pub window_size: Uint64,

    /// Number of divisions in the window.
    /// The window size must be evenly divisible by the division count.
    /// While operating, the actual count of divisions is between 0 and division count + 1 inclusively
    /// since window might cover only a part of the division, for example, if division count is 3:
    ///
    /// |   div 1   |   div 2   |   div 3   |   div 4   |
    ///      |------------- window --------------|
    ///
    /// The window size is 3 divisions, but the actutal division needed for SMA is 4.
    pub division_count: Uint64,
}

impl WindowConfig {
    fn division_size(&self) -> Result<Uint64, ContractError> {
        self.window_size
            .checked_div(self.division_count)
            .map_err(Into::into)
    }
}

/// Limiter that determines limit by upper bound of SMA (Simple Moving Average) of the value.
/// The data points used for calculating SMA are divided into divisions, which gets compressed
/// for storage read efficiency, and reduce gas consumption.
///
/// See [`Division`] for more detail on how the division is compressed and
/// how SMA is calculated.
#[cw_serde]
pub struct ChangeLimiter {
    /// Divisions in the window, divisions are ordered from oldest to newest.
    /// Kept divisions must exist within or overlap with the window, else
    /// they will be cleaned up.
    divisions: Vec<Division>,

    /// Latest updated value.
    latest_value: Decimal,

    /// Config for window and divisions
    window_config: WindowConfig,

    /// Offset from the moving average that the value is allowed to be updated to.
    boundary_offset: Decimal,
}

impl ChangeLimiter {
    pub fn new(
        window_config: WindowConfig,
        boundary_offset: Decimal,
    ) -> Result<Self, ContractError> {
        Self {
            divisions: vec![],
            latest_value: Decimal::zero(),
            window_config,
            boundary_offset,
        }
        .ensure_boundary_offset_constrain()?
        .ensure_window_config_constraint()
    }

    pub fn divisions(&self) -> &[Division] {
        &self.divisions
    }

    pub fn latest_value(&self) -> Decimal {
        self.latest_value
    }

    pub fn reset(self) -> Self {
        Self {
            divisions: vec![],
            latest_value: Decimal::zero(),
            window_config: self.window_config,
            boundary_offset: self.boundary_offset,
        }
    }

    fn ensure_boundary_offset_constrain(self) -> Result<Self, ContractError> {
        ensure!(
            self.boundary_offset > Decimal::zero(),
            ContractError::ZeroBoundaryOffset {}
        );

        Ok(self)
    }

    fn ensure_window_config_constraint(self) -> Result<Self, ContractError> {
        let config = &self.window_config;

        // window size must be greater than zero
        ensure!(
            config.window_size > Uint64::zero(),
            ContractError::ZeroWindowSize {}
        );

        // division count must not exceed MAX_DIVISION_COUNT
        ensure!(
            config.division_count <= MAX_DIVISION_COUNT,
            ContractError::DivisionCountExceeded {
                max_division_count: MAX_DIVISION_COUNT
            }
        );

        // division count must evenly divide window size
        let is_window_evenly_dividable =
            config.window_size.checked_rem(config.division_count)? == Uint64::zero();
        ensure!(
            is_window_evenly_dividable,
            ContractError::UnevenWindowDivision {}
        );

        Ok(self)
    }

    fn ensure_upper_limit(
        self,
        block_time: Timestamp,
        scope: &Scope,
        value: Decimal,
    ) -> Result<Self, ContractError> {
        let (updated_limiter, upper_limit) = self.upper_limit(block_time)?;

        if let Some(upper_limit) = upper_limit {
            ensure!(
                value <= upper_limit,
                ContractError::UpperLimitExceeded {
                    scope: scope.clone(),
                    upper_limit,
                    value,
                }
            );
        }

        Ok(updated_limiter)
    }

    fn upper_limit(self, block_time: Timestamp) -> Result<(Self, Option<Decimal>), ContractError> {
        let (latest_removed_division, updated_limiter) =
            self.clean_up_outdated_divisions(block_time)?;

        // Check for upper limit if there is any existing division or there is any removed divisions
        let has_any_prev_data_points =
            !updated_limiter.divisions.is_empty() || latest_removed_division.is_some();

        if has_any_prev_data_points {
            let avg = Division::compressed_moving_average(
                latest_removed_division,
                &updated_limiter.divisions,
                updated_limiter.window_config.division_size()?,
                updated_limiter.window_config.window_size,
                block_time,
            )?;

            // using saturating_add/sub since the overflowed value can't be exceeded anyway
            let upper_limit = avg.saturating_add(updated_limiter.boundary_offset);
            Ok((updated_limiter, Some(upper_limit)))
        } else {
            Ok((updated_limiter, None))
        }
    }

    fn update(self, block_time: Timestamp, value: Decimal) -> Result<Self, ContractError> {
        let mut updated_limiter = self;

        let division_size = updated_limiter.window_config.division_size()?;
        let prev_value = updated_limiter.latest_value;
        updated_limiter.latest_value = value;

        updated_limiter.divisions = if updated_limiter.divisions.is_empty() {
            // no need to ensure time invariant since
            // started_at = updated_at so
            // `updated_at <= started_at + division_size` is always true
            vec![Division::new(block_time, block_time, value, value)?]
        } else {
            // If the division is over, create a new division
            let mut divisions = updated_limiter.divisions;
            let latest_division = divisions
                .last()
                // this error should never occur since we checked if divisions is empty
                .ok_or(StdError::generic_err("divisions must not be empty"))?;

            if latest_division.elapsed_time(block_time)? >= division_size {
                let started_at = latest_division.next_started_at(division_size, block_time)?;
                let updated_at = block_time;
                let ended_at = started_at.plus_nanos(division_size.u64());

                // ensure time invariant
                ensure!(
                    updated_at <= ended_at,
                    ContractError::UpdateAfterDivisionEnded {
                        updated_at,
                        ended_at
                    }
                );

                let new_division = Division::new(started_at, updated_at, value, prev_value)?;
                divisions.push(new_division);
            }
            // else update the current division
            else {
                let last_index = divisions.len() - 1;

                let updated_at = block_time;
                let ended_at =
                    Timestamp::from_nanos(latest_division.ended_at(division_size)?.u64());

                // ensure time invariant
                ensure!(
                    updated_at <= ended_at,
                    ContractError::UpdateAfterDivisionEnded {
                        updated_at,
                        ended_at
                    }
                );

                divisions[last_index] = latest_division.update(updated_at, value)?;
            }

            divisions
        };

        Ok(updated_limiter)
    }

    fn clean_up_outdated_divisions(
        self,
        block_time: Timestamp,
    ) -> Result<(Option<Division>, Self), ContractError> {
        let mut latest_removed_division = None;

        let mut divisions = self.divisions;

        while let Some(division) = divisions.first() {
            // if window completely passed the division, remove the division
            if division.is_outdated(
                block_time,
                self.window_config.window_size,
                self.window_config.division_size()?,
            )? {
                latest_removed_division = Some(divisions.remove(0));
            } else {
                break;
            }
        }

        Ok((latest_removed_division, Self { divisions, ..self }))
    }
}

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
    ChangeLimiter(ChangeLimiter),
    StaticLimiter(StaticLimiter),
}

#[cw_serde]
pub enum LimiterParams {
    ChangeLimiter {
        window_config: WindowConfig,
        boundary_offset: Decimal,
    },
    StaticLimiter {
        upper_limit: Decimal,
    },
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
            LimiterParams::ChangeLimiter {
                window_config,
                boundary_offset,
            } => Limiter::ChangeLimiter(ChangeLimiter::new(window_config, boundary_offset)?),
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

    /// Set boundary offset for a [`ChangeLimiter`] only, otherwise it will fail.
    pub fn set_change_limiter_boundary_offset(
        &self,
        storage: &mut dyn Storage,
        scope: Scope,
        label: &str,
        boundary_offset: Decimal,
    ) -> Result<(), ContractError> {
        self.limiters.update(
            storage,
            (&scope.key(), label),
            |limiter: Option<Limiter>| -> Result<Limiter, ContractError> {
                let limiter = limiter.ok_or(ContractError::LimiterDoesNotExist {
                    scope,
                    label: label.to_string(),
                })?;

                // check if the limiter is a ChangeLimiter
                match limiter {
                    Limiter::ChangeLimiter(limiter) => Ok({
                        let change_limiter = ChangeLimiter {
                            boundary_offset,
                            ..limiter
                        }
                        .ensure_boundary_offset_constrain()?;

                        Limiter::ChangeLimiter(change_limiter)
                    }),
                    Limiter::StaticLimiter(_) => Err(ContractError::WrongLimiterType {
                        expected: "change_limiter".to_string(),
                        actual: "static_limiter".to_string(),
                    }),
                }
            },
        )?;
        Ok(())
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
                    Limiter::ChangeLimiter(_) => Err(ContractError::WrongLimiterType {
                        expected: "static_limiter".to_string(),
                        actual: "change_limiter".to_string(),
                    }),
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
        block_time: Timestamp,
    ) -> Result<(), ContractError> {
        for (scope, (prev_value, value)) in scope_value_pairs {
            let limiters = self.list_limiters_by_scope(storage, &scope)?;
            let is_not_decreasing = value >= prev_value;

            for (label, limiter) in limiters {
                // Enforce limiter only if value is increasing, because if the value is decreasing from the previous value,
                // for the specific scope, it is a balancing act to move away from the limit.
                let limiter = match limiter {
                    Limiter::ChangeLimiter(limiter) => Limiter::ChangeLimiter({
                        if is_not_decreasing {
                            limiter
                                .ensure_upper_limit(block_time, &scope, value)?
                                .update(block_time, value)?
                        } else {
                            limiter.update(block_time, value)?
                        }
                    }),
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

    /// If the normalization factor has a non-uniform update, staled divisions will become invalid.
    /// In case of adding new assets, even if there is nothing wrong with the normalization factor,
    /// the asset composition change required some time to be properly reflected.
    ///
    /// This function cleans up the staled divisions and create new division with updated state,
    /// which is a start over with the new asset composition and normalization factor.
    pub fn reset_change_limiter_states(
        &self,
        storage: &mut dyn Storage,
        block_time: Timestamp,
        weights: impl Iterator<Item = (String, Decimal)>,
    ) -> Result<(), ContractError> {
        // there is no need to limit, since the number of limiters is expected to be small
        let limiters = self.list_limiters(storage)?;
        let weights: HashMap<String, Decimal> = weights.into_iter().collect();

        for ((scope, label), limiter) in limiters {
            match limiter {
                Limiter::ChangeLimiter(limiter) => {
                    self.limiters
                        .save(storage, (scope.as_str(), label.as_str()), {
                            let value = weights.get(scope.as_str()).copied().ok_or_else(|| {
                                StdError::not_found(format!("weight for {}", scope))
                            })?;
                            &Limiter::ChangeLimiter(limiter.reset().update(block_time, value)?)
                        })?
                }
                Limiter::StaticLimiter(_) => {}
            };
        }

        Ok(())
    }

    /// Get the upper limit for each scope. Lowest upper limit for each scope wins.
    pub fn upper_limits(
        &self,
        storage: &dyn Storage,
        block_time: Timestamp,
    ) -> Result<BTreeMap<Scope, Decimal>, ContractError> {
        let mut upper_limits = BTreeMap::new();
        for ((scope_str, _), limiter) in self.list_limiters(storage)?.into_iter() {
            let scope = scope_str.parse::<Scope>()?;
            match limiter {
                Limiter::StaticLimiter(limiter) => {
                    upper_limits
                        .entry(scope)
                        .and_modify(|ul: &mut Decimal| *ul = (*ul).min(limiter.upper_limit))
                        .or_insert(limiter.upper_limit);
                }
                Limiter::ChangeLimiter(change_limiter) => {
                    let upper_limit = match change_limiter.upper_limit(block_time)? {
                        (_, Some(upper_limit)) => upper_limit,
                        (_, None) => Decimal::one(), // if no upper limit, use 100%
                    };

                    upper_limits
                        .entry(scope)
                        .and_modify(|ul: &mut Decimal| *ul = (*ul).min(upper_limit))
                        .or_insert(upper_limit);
                }
            }
        }

        Ok(upper_limits)
    }
}

/// This is used for testing if all change limiters has been newly created or reset.
#[cfg(test)]
#[macro_export]
macro_rules! assert_reset_change_limiters_by_scope {
    ($scope:expr, $reset_at:expr, $transmuter:expr, $storage:expr) => {
        let pool = $transmuter.pool.load($storage).unwrap();
        let asset_weights = pool
            .asset_weights()
            .unwrap()
            .unwrap_or_default()
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>();

        let asset_group_weights = pool.asset_group_weights().unwrap().unwrap_or_default();

        let limiters = $transmuter
            .limiters
            .list_limiters_by_scope($storage, $scope)
            .expect("failed to list limiters");

        for (_label, limiter) in limiters {
            if let $crate::limiter::Limiter::ChangeLimiter(limiter) = limiter {
                let value = match $scope {
                    Scope::Denom(denom) => *asset_weights.get(denom.as_str()).unwrap(),
                    Scope::AssetGroup(label) => *asset_group_weights.get(label.as_str()).unwrap(),
                };
                assert_eq!(
                    limiter.divisions(),
                    &[transmuter_math::Division::new($reset_at, $reset_at, value, value).unwrap()]
                )
            };
        }
    };
}

/// This is used for testing if a change limiters for denom has been updated
#[cfg(test)]
#[macro_export]
macro_rules! assert_dirty_change_limiters_by_scope {
    ($scope:expr, $lim:expr, $storage:expr) => {
        let limiters = $lim
            .list_limiters_by_scope($storage, $scope)
            .expect("failed to list limiters");

        for (label, limiter) in limiters {
            match limiter {
                Limiter::ChangeLimiter(limiter) => {
                    limiter.divisions();
                    assert_ne!(
                        limiter,
                        limiter.clone().reset(),
                        "Change Limiter `{}/{}` is clean but expect dirty",
                        $scope,
                        label
                    );
                }
                Limiter::StaticLimiter(_) => {}
            };
        }
    };
}

#[cfg(test)]
mod tests {
    use cosmwasm_std::testing::mock_dependencies;

    use super::*;

    const EPSILON: Decimal = Decimal::raw(1);

    mod registration {
        use super::*;

        #[test]
        fn test_register_limiter_works() {
            let mut deps = mock_dependencies();
            let limiter = Limiters::new("limiters");

            limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "1m",
                    LimiterParams::ChangeLimiter {
                        window_config: WindowConfig {
                            window_size: Uint64::from(604_800_000_000u64),
                            division_count: Uint64::from(5u64),
                        },
                        boundary_offset: Decimal::percent(10),
                    },
                )
                .unwrap();

            assert_eq!(
                limiter.list_limiters(&deps.storage).unwrap(),
                vec![(
                    (Scope::denom("denoma").key(), "1m".to_string()),
                    Limiter::ChangeLimiter(ChangeLimiter {
                        divisions: vec![],
                        latest_value: Decimal::zero(),
                        window_config: WindowConfig {
                            window_size: Uint64::from(604_800_000_000u64),
                            division_count: Uint64::from(5u64),
                        },
                        boundary_offset: Decimal::percent(10)
                    })
                )]
            );

            limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "1h",
                    LimiterParams::ChangeLimiter {
                        window_config: WindowConfig {
                            window_size: Uint64::from(3_600_000_000_000u64),
                            division_count: Uint64::from(2u64),
                        },
                        boundary_offset: Decimal::percent(10),
                    },
                )
                .unwrap();

            assert_eq!(
                limiter.list_limiters(&deps.storage).unwrap(),
                vec![
                    (
                        (Scope::denom("denoma").key(), "1h".to_string()),
                        Limiter::ChangeLimiter(ChangeLimiter {
                            divisions: vec![],
                            latest_value: Decimal::zero(),
                            window_config: WindowConfig {
                                window_size: Uint64::from(3_600_000_000_000u64),
                                division_count: Uint64::from(2u64),
                            },
                            boundary_offset: Decimal::percent(10)
                        })
                    ),
                    (
                        (Scope::denom("denoma").key(), "1m".to_string()),
                        Limiter::ChangeLimiter(ChangeLimiter {
                            divisions: vec![],
                            latest_value: Decimal::zero(),
                            window_config: WindowConfig {
                                window_size: Uint64::from(604_800_000_000u64),
                                division_count: Uint64::from(5u64),
                            },
                            boundary_offset: Decimal::percent(10)
                        })
                    )
                ]
            );

            limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denomb"),
                    "1m",
                    LimiterParams::ChangeLimiter {
                        window_config: WindowConfig {
                            window_size: Uint64::from(604_800_000_000u64),
                            division_count: Uint64::from(5u64),
                        },
                        boundary_offset: Decimal::percent(10),
                    },
                )
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

            assert_eq!(
                limiter.list_limiters(&deps.storage).unwrap(),
                vec![
                    (
                        (Scope::denom("denoma").key(), "1h".to_string()),
                        Limiter::ChangeLimiter(ChangeLimiter {
                            divisions: vec![],
                            latest_value: Decimal::zero(),
                            window_config: WindowConfig {
                                window_size: Uint64::from(3_600_000_000_000u64),
                                division_count: Uint64::from(2u64),
                            },
                            boundary_offset: Decimal::percent(10)
                        })
                    ),
                    (
                        (Scope::denom("denoma").key(), "1m".to_string()),
                        Limiter::ChangeLimiter(ChangeLimiter {
                            divisions: vec![],
                            latest_value: Decimal::zero(),
                            window_config: WindowConfig {
                                window_size: Uint64::from(604_800_000_000u64),
                                division_count: Uint64::from(5u64),
                            },
                            boundary_offset: Decimal::percent(10)
                        })
                    ),
                    (
                        (Scope::denom("denoma").key(), "static".to_string()),
                        Limiter::StaticLimiter(StaticLimiter {
                            upper_limit: Decimal::percent(10)
                        })
                    ),
                    (
                        (Scope::denom("denomb").key(), "1m".to_string()),
                        Limiter::ChangeLimiter(ChangeLimiter {
                            divisions: vec![],
                            latest_value: Decimal::zero(),
                            window_config: WindowConfig {
                                window_size: Uint64::from(604_800_000_000u64),
                                division_count: Uint64::from(5u64),
                            },
                            boundary_offset: Decimal::percent(10)
                        })
                    )
                ]
            );

            // list limiters by denom
            assert_eq!(
                limiter
                    .list_limiters_by_scope(&deps.storage, &Scope::denom("denoma"))
                    .unwrap(),
                vec![
                    (
                        "1h".to_string(),
                        Limiter::ChangeLimiter(ChangeLimiter {
                            divisions: vec![],
                            latest_value: Decimal::zero(),
                            window_config: WindowConfig {
                                window_size: Uint64::from(3_600_000_000_000u64),
                                division_count: Uint64::from(2u64),
                            },
                            boundary_offset: Decimal::percent(10)
                        })
                    ),
                    (
                        "1m".to_string(),
                        Limiter::ChangeLimiter(ChangeLimiter {
                            divisions: vec![],
                            latest_value: Decimal::zero(),
                            window_config: WindowConfig {
                                window_size: Uint64::from(604_800_000_000u64),
                                division_count: Uint64::from(5u64),
                            },
                            boundary_offset: Decimal::percent(10)
                        })
                    ),
                    (
                        "static".to_string(),
                        Limiter::StaticLimiter(StaticLimiter {
                            upper_limit: Decimal::percent(10)
                        })
                    )
                ]
            );

            assert_eq!(
                limiter
                    .list_limiters_by_scope(&deps.storage, &Scope::denom("denomb"))
                    .unwrap(),
                vec![(
                    "1m".to_string(),
                    Limiter::ChangeLimiter(ChangeLimiter {
                        divisions: vec![],
                        latest_value: Decimal::zero(),
                        window_config: WindowConfig {
                            window_size: Uint64::from(604_800_000_000u64),
                            division_count: Uint64::from(5u64),
                        },
                        boundary_offset: Decimal::percent(10)
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
                    LimiterParams::ChangeLimiter {
                        window_config: WindowConfig {
                            window_size: Uint64::from(604_800_000_000u64),
                            division_count: Uint64::from(5u64),
                        },
                        boundary_offset: Decimal::percent(10),
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
                    LimiterParams::ChangeLimiter {
                        window_config: WindowConfig {
                            window_size: Uint64::from(604_800_000_000u64),
                            division_count: Uint64::from(5u64),
                        },
                        boundary_offset: Decimal::percent(10),
                    },
                )
                .unwrap();

            let err = limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "1m",
                    LimiterParams::ChangeLimiter {
                        window_config: WindowConfig {
                            window_size: Uint64::from(604_800_000_000u64),
                            division_count: Uint64::from(10u64),
                        },
                        boundary_offset: Decimal::percent(10),
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
                    LimiterParams::ChangeLimiter {
                        window_config: WindowConfig {
                            window_size: Uint64::from(3_600_000_000_000u64 * h),
                            division_count: Uint64::from(5u64),
                        },
                        boundary_offset: Decimal::percent(10),
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
                    "1m",
                    LimiterParams::ChangeLimiter {
                        window_config: WindowConfig {
                            window_size: Uint64::from(604_800_000_000u64),
                            division_count: Uint64::from(5u64),
                        },
                        boundary_offset: Decimal::percent(10),
                    },
                )
                .unwrap();

            assert_eq!(
                limiter.list_limiters(&deps.storage).unwrap(),
                vec![(
                    (Scope::denom("denoma").key(), "1m".to_string()),
                    Limiter::ChangeLimiter(ChangeLimiter {
                        divisions: vec![],
                        latest_value: Decimal::zero(),
                        window_config: WindowConfig {
                            window_size: Uint64::from(604_800_000_000u64),
                            division_count: Uint64::from(5u64),
                        },
                        boundary_offset: Decimal::percent(10)
                    })
                )]
            );

            limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "1h",
                    LimiterParams::ChangeLimiter {
                        window_config: WindowConfig {
                            window_size: Uint64::from(3_600_000_000_000u64),
                            division_count: Uint64::from(2u64),
                        },
                        boundary_offset: Decimal::percent(10),
                    },
                )
                .unwrap();

            assert_eq!(
                limiter.list_limiters(&deps.storage).unwrap(),
                vec![
                    (
                        (Scope::denom("denoma").key(), "1h".to_string()),
                        Limiter::ChangeLimiter(ChangeLimiter {
                            divisions: vec![],
                            latest_value: Decimal::zero(),
                            window_config: WindowConfig {
                                window_size: Uint64::from(3_600_000_000_000u64),
                                division_count: Uint64::from(2u64),
                            },
                            boundary_offset: Decimal::percent(10)
                        })
                    ),
                    (
                        (Scope::denom("denoma").key(), "1m".to_string()),
                        Limiter::ChangeLimiter(ChangeLimiter {
                            divisions: vec![],
                            latest_value: Decimal::zero(),
                            window_config: WindowConfig {
                                window_size: Uint64::from(604_800_000_000u64),
                                division_count: Uint64::from(5u64),
                            },
                            boundary_offset: Decimal::percent(10)
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

            limiter
                .deregister(&mut deps.storage, Scope::denom("denoma"), "1m")
                .unwrap();

            assert_eq!(
                limiter.list_limiters(&deps.storage).unwrap(),
                vec![(
                    (Scope::denom("denoma").key(), "1h".to_string()),
                    Limiter::ChangeLimiter(ChangeLimiter {
                        divisions: vec![],
                        latest_value: Decimal::zero(),
                        window_config: WindowConfig {
                            window_size: Uint64::from(3_600_000_000_000u64),
                            division_count: Uint64::from(2u64),
                        },
                        boundary_offset: Decimal::percent(10)
                    })
                )]
            );

            let err = limiter
                .deregister(&mut deps.storage, Scope::denom("denoma"), "1h")
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::EmptyLimiterNotAllowed {
                    scope: Scope::denom("denoma")
                }
            );

            assert_eq!(
                limiter.list_limiters(&deps.storage).unwrap(),
                vec![(
                    (Scope::denom("denoma").key(), "1h".to_string()),
                    Limiter::ChangeLimiter(ChangeLimiter {
                        divisions: vec![],
                        latest_value: Decimal::zero(),
                        window_config: WindowConfig {
                            window_size: Uint64::from(3_600_000_000_000u64),
                            division_count: Uint64::from(2u64),
                        },
                        boundary_offset: Decimal::percent(10)
                    })
                )]
            );
            limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denomb"),
                    "1m",
                    LimiterParams::ChangeLimiter {
                        window_config: WindowConfig {
                            window_size: Uint64::from(604_800_000_000u64),
                            division_count: Uint64::from(5u64),
                        },
                        boundary_offset: Decimal::percent(10),
                    },
                )
                .unwrap();

            let err = limiter
                .deregister(&mut deps.storage, Scope::denom("denomb"), "1m")
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::EmptyLimiterNotAllowed {
                    scope: Scope::denom("denomb")
                }
            );

            limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denomb"),
                    "1h",
                    LimiterParams::ChangeLimiter {
                        window_config: WindowConfig {
                            window_size: Uint64::from(3_600_000_000_000u64),
                            division_count: Uint64::from(2u64),
                        },
                        boundary_offset: Decimal::percent(10),
                    },
                )
                .unwrap();

            let err = limiter
                .deregister(&mut deps.storage, Scope::denom("denoma"), "1h")
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::EmptyLimiterNotAllowed {
                    scope: Scope::denom("denoma")
                }
            );

            limiter
                .deregister(&mut deps.storage, Scope::denom("denomb"), "1m")
                .unwrap();

            assert_eq!(
                limiter.list_limiters(&deps.storage).unwrap(),
                vec![
                    (
                        (Scope::denom("denoma").key(), "1h".to_string()),
                        Limiter::ChangeLimiter(ChangeLimiter {
                            divisions: vec![],
                            latest_value: Decimal::zero(),
                            window_config: WindowConfig {
                                window_size: Uint64::from(3_600_000_000_000u64),
                                division_count: Uint64::from(2u64),
                            },
                            boundary_offset: Decimal::percent(10)
                        })
                    ),
                    (
                        (Scope::denom("denomb").key(), "1h".to_string()),
                        Limiter::ChangeLimiter(ChangeLimiter {
                            divisions: vec![],
                            latest_value: Decimal::zero(),
                            window_config: WindowConfig {
                                window_size: Uint64::from(3_600_000_000_000u64),
                                division_count: Uint64::from(2u64),
                            },
                            boundary_offset: Decimal::percent(10)
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
                    LimiterParams::ChangeLimiter {
                        window_config: WindowConfig {
                            window_size: Uint64::from(3_600_000_000_000u64),
                            division_count: Uint64::from(2u64),
                        },
                        boundary_offset: Decimal::percent(10),
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
                Limiter::ChangeLimiter(ChangeLimiter {
                    divisions: vec![],
                    latest_value: Decimal::zero(),
                    window_config: WindowConfig {
                        window_size: Uint64::from(3_600_000_000_000u64),
                        division_count: Uint64::from(2u64),
                    },
                    boundary_offset: Decimal::percent(10)
                })
            );

            // Check that the remaining limiters are correct
            assert_eq!(limiter.list_limiters(&deps.storage).unwrap(), vec![]);
        }
    }

    mod set_config {
        use cosmwasm_std::DivideByZeroError;

        use super::*;

        #[test]
        fn test_fail_due_to_div_count_does_not_evenly_divide_the_window() {
            let mut deps = mock_dependencies();

            let limiter = Limiters::new("limiters");

            let err = limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "1m",
                    LimiterParams::ChangeLimiter {
                        window_config: WindowConfig {
                            window_size: Uint64::from(604_800_000_001u64),
                            division_count: Uint64::from(9u64),
                        },
                        boundary_offset: Decimal::percent(10),
                    },
                )
                .unwrap_err();

            assert_eq!(err, ContractError::UnevenWindowDivision {});
        }

        #[test]
        fn test_fail_due_to_div_size_is_zero() {
            let mut deps = mock_dependencies();

            let limiter = Limiters::new("limiters");

            let err = limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "1m",
                    LimiterParams::ChangeLimiter {
                        window_config: WindowConfig {
                            window_size: Uint64::from(604_800_000_000u64),
                            division_count: Uint64::from(0u64),
                        },
                        boundary_offset: Decimal::percent(10),
                    },
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::DivideByZeroError(DivideByZeroError::new())
            );
        }

        #[test]
        fn test_fail_due_to_window_size_is_zero() {
            let mut deps = mock_dependencies();

            let limiter = Limiters::new("limiters");

            let err = limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "1m",
                    LimiterParams::ChangeLimiter {
                        window_config: WindowConfig {
                            window_size: Uint64::zero(),
                            division_count: Uint64::from(5u64),
                        },
                        boundary_offset: Decimal::percent(10),
                    },
                )
                .unwrap_err();

            assert_eq!(err, ContractError::ZeroWindowSize {});
        }

        #[test]
        fn test_fail_due_to_max_division_count_exceeded() {
            let mut deps = mock_dependencies();

            let limiter = Limiters::new("limiters");

            let err = limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "1m",
                    LimiterParams::ChangeLimiter {
                        window_config: WindowConfig {
                            window_size: Uint64::from(660_000_000_000u64),
                            division_count: Uint64::from(11u64),
                        },
                        boundary_offset: Decimal::percent(10),
                    },
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::DivisionCountExceeded {
                    max_division_count: MAX_DIVISION_COUNT
                }
            );
        }

        #[test]
        fn test_successful() {
            let mut deps = mock_dependencies();

            let limiter = Limiters::new("limiters");

            limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "1m",
                    LimiterParams::ChangeLimiter {
                        window_config: WindowConfig {
                            window_size: Uint64::from(604_800_000_000u64),
                            division_count: Uint64::from(5u64),
                        },
                        boundary_offset: Decimal::percent(10),
                    },
                )
                .unwrap();

            let limiters = limiter.list_limiters(&deps.storage).unwrap();

            assert_eq!(
                limiters,
                vec![(
                    (Scope::denom("denoma").key(), "1m".to_string()),
                    Limiter::ChangeLimiter(ChangeLimiter {
                        divisions: vec![],
                        latest_value: Decimal::zero(),
                        window_config: WindowConfig {
                            window_size: Uint64::from(604_800_000_000u64),
                            division_count: Uint64::from(5u64),
                        },
                        boundary_offset: Decimal::percent(10)
                    })
                )]
            );
        }
    }

    mod pararm_validation {
        use super::*;

        #[test]
        fn change_limiter_validation() {
            // boundary offset is zero
            assert_eq!(
                ChangeLimiter::new(
                    WindowConfig {
                        window_size: 604_800_000_000u64.into(),
                        division_count: 2u64.into(),
                    },
                    Decimal::zero()
                )
                .unwrap_err(),
                ContractError::ZeroBoundaryOffset {}
            );

            // window size is zero
            assert_eq!(
                ChangeLimiter::new(
                    WindowConfig {
                        window_size: 0u64.into(),
                        division_count: 2u64.into(),
                    },
                    Decimal::percent(10)
                )
                .unwrap_err(),
                ContractError::ZeroWindowSize {}
            );

            // exceed MAX_DIVISION_COUNT
            assert_eq!(
                ChangeLimiter::new(
                    WindowConfig {
                        window_size: 604_800_000_000u64.into(),
                        division_count: MAX_DIVISION_COUNT + Uint64::one(),
                    },
                    Decimal::percent(10)
                )
                .unwrap_err(),
                ContractError::DivisionCountExceeded {
                    max_division_count: MAX_DIVISION_COUNT
                }
            );

            // division count does not evenly divide the window
            assert_eq!(
                ChangeLimiter::new(
                    WindowConfig {
                        window_size: 604_800_000_001u64.into(),
                        division_count: 9u64.into(),
                    },
                    Decimal::percent(10)
                )
                .unwrap_err(),
                ContractError::UnevenWindowDivision {}
            );
        }

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
    mod remove_outdated_division {
        use super::*;

        #[test]
        fn test_empty_divisions() {
            let limiter = ChangeLimiter {
                divisions: vec![],
                latest_value: Decimal::zero(),
                window_config: WindowConfig {
                    window_size: Uint64::from(3_600_000_000_000u64),
                    division_count: Uint64::from(5u64),
                },
                boundary_offset: Decimal::percent(10),
            };

            let block_time = Timestamp::from_nanos(1661231280000000000);
            let (latest_removed_division, limiter) =
                limiter.clean_up_outdated_divisions(block_time).unwrap();

            assert_eq!(latest_removed_division, None);
            assert_eq!(limiter.divisions, vec![]);
        }

        #[test]
        fn test_no_outdated_divisions() {
            let config = WindowConfig {
                window_size: Uint64::from(3_600_000_000_000u64),
                division_count: Uint64::from(2u64),
            };

            let block_time = Timestamp::from_nanos(1661231280000000000);

            let divisions = vec![
                Division::new(
                    block_time.minus_nanos(config.window_size.u64()),
                    block_time
                        .minus_nanos(config.window_size.u64())
                        .plus_minutes(10),
                    Decimal::percent(20),
                    Decimal::percent(10),
                )
                .unwrap(),
                Division::new(
                    block_time.minus_nanos(
                        config.window_size.u64() - config.division_size().unwrap().u64(),
                    ),
                    block_time
                        .minus_nanos(
                            config.window_size.u64() - config.division_size().unwrap().u64(),
                        )
                        .plus_minutes(20),
                    Decimal::percent(30),
                    Decimal::percent(20),
                )
                .unwrap(),
            ];
            let limiter = ChangeLimiter {
                divisions: divisions.clone(),
                latest_value: Decimal::percent(30),
                window_config: config,
                boundary_offset: Decimal::percent(10),
            };
            let (latest_removed_division, limiter) =
                limiter.clean_up_outdated_divisions(block_time).unwrap();

            assert_eq!(latest_removed_division, None);
            assert_eq!(limiter.divisions, divisions);

            // with overlapping divisions
            let config = WindowConfig {
                window_size: Uint64::from(3_600_000_000_000u64),
                division_count: Uint64::from(2u64),
            };

            let offset_mins = 20;
            let divisions = vec![
                Division::new(
                    block_time
                        .minus_nanos(config.window_size.u64())
                        .minus_minutes(offset_mins),
                    block_time
                        .minus_nanos(config.window_size.u64())
                        .minus_minutes(offset_mins)
                        .plus_minutes(10),
                    Decimal::percent(10),
                    Decimal::percent(10),
                )
                .unwrap(),
                Division::new(
                    block_time
                        .minus_nanos(config.window_size.u64())
                        .plus_nanos(config.division_size().unwrap().u64())
                        .minus_minutes(offset_mins),
                    block_time
                        .minus_nanos(config.window_size.u64())
                        .plus_nanos(config.division_size().unwrap().u64())
                        .minus_minutes(offset_mins)
                        .plus_minutes(20),
                    Decimal::percent(20),
                    Decimal::percent(10),
                )
                .unwrap(),
                Division::new(
                    block_time
                        .minus_nanos(config.window_size.u64())
                        .plus_nanos(config.division_size().unwrap().u64() * 2)
                        .minus_minutes(offset_mins),
                    block_time
                        .minus_nanos(config.window_size.u64())
                        .plus_nanos(config.division_size().unwrap().u64() * 2)
                        .minus_minutes(offset_mins)
                        .plus_minutes(40),
                    Decimal::percent(30),
                    Decimal::percent(20),
                )
                .unwrap(),
            ];
            let limiter = ChangeLimiter {
                divisions: divisions.clone(),
                latest_value: Decimal::percent(30),
                window_config: config,
                boundary_offset: Decimal::percent(10),
            };

            let (latest_removed_division, limiter) =
                limiter.clean_up_outdated_divisions(block_time).unwrap();

            assert_eq!(latest_removed_division, None);
            assert_eq!(limiter.divisions, divisions);
        }

        #[test]
        fn test_with_single_outdated_divisions() {
            let config = WindowConfig {
                window_size: Uint64::from(3_600_000_000_000u64),
                division_count: Uint64::from(2u64),
            };

            let block_time = Timestamp::from_nanos(1661231280000000000);

            let divisions = vec![
                Division::new(
                    block_time
                        .minus_nanos(config.window_size.u64())
                        .minus_nanos(config.division_size().unwrap().u64()),
                    block_time
                        .minus_nanos(config.window_size.u64())
                        .minus_nanos(config.division_size().unwrap().u64())
                        .plus_minutes(10),
                    Decimal::percent(10),
                    Decimal::percent(10),
                )
                .unwrap(),
                Division::new(
                    block_time.minus_nanos(config.window_size.u64()),
                    block_time
                        .minus_nanos(config.window_size.u64())
                        .plus_minutes(20),
                    Decimal::percent(20),
                    Decimal::percent(10),
                )
                .unwrap(),
                Division::new(
                    block_time
                        .minus_nanos(config.window_size.u64())
                        .plus_nanos(config.division_size().unwrap().u64()),
                    block_time
                        .minus_nanos(config.window_size.u64())
                        .plus_nanos(config.division_size().unwrap().u64())
                        .plus_minutes(40),
                    Decimal::percent(30),
                    Decimal::percent(20),
                )
                .unwrap(),
            ];
            let limiter = ChangeLimiter {
                divisions: divisions.clone(),
                latest_value: Decimal::percent(30),
                window_config: config,
                boundary_offset: Decimal::percent(10),
            };

            let (latest_removed_division, limiter) =
                limiter.clean_up_outdated_divisions(block_time).unwrap();

            assert_eq!(latest_removed_division, Some(divisions[0].clone()));
            assert_eq!(limiter.divisions, divisions[1..].to_vec());
        }

        #[test]
        fn test_with_multiple_outdated_division() {
            // with no overlapping divisions

            let config = WindowConfig {
                window_size: Uint64::from(3_600_000_000_000u64),
                division_count: Uint64::from(2u64),
            };

            let block_time = Timestamp::from_nanos(1661231280000000000);

            let offset_minutes = 0;

            let divisions = vec![
                Division::new(
                    block_time
                        .minus_nanos(config.window_size.u64() * 2)
                        .plus_minutes(offset_minutes),
                    block_time
                        .minus_nanos(config.window_size.u64() * 2)
                        .plus_minutes(offset_minutes)
                        .plus_minutes(10),
                    Decimal::percent(10),
                    Decimal::percent(10),
                )
                .unwrap(),
                Division::new(
                    block_time
                        .minus_nanos(config.window_size.u64() * 2)
                        .plus_nanos(config.division_size().unwrap().u64())
                        .plus_minutes(offset_minutes),
                    block_time
                        .minus_nanos(config.window_size.u64() * 2)
                        .plus_nanos(config.division_size().unwrap().u64())
                        .plus_minutes(offset_minutes)
                        .plus_minutes(20),
                    Decimal::percent(20),
                    Decimal::percent(10),
                )
                .unwrap(),
                Division::new(
                    block_time
                        .minus_nanos(config.window_size.u64() * 2)
                        .plus_nanos(config.division_size().unwrap().u64() * 2)
                        .plus_minutes(offset_minutes),
                    block_time
                        .minus_nanos(config.window_size.u64() * 2)
                        .plus_nanos(config.division_size().unwrap().u64() * 2)
                        .plus_minutes(offset_minutes)
                        .plus_minutes(40),
                    Decimal::percent(30),
                    Decimal::percent(20),
                )
                .unwrap(),
            ];
            let limiter = ChangeLimiter {
                divisions: divisions.clone(),
                latest_value: Decimal::percent(30),
                window_config: config,
                boundary_offset: Decimal::percent(10),
            };

            let (latest_removed_division, limiter) =
                limiter.clean_up_outdated_divisions(block_time).unwrap();

            assert_eq!(latest_removed_division, Some(divisions[1].clone()));
            assert_eq!(limiter.divisions, divisions[2..].to_vec());

            // with some overlapping divisions

            let config = WindowConfig {
                window_size: Uint64::from(3_600_000_000_000u64),
                division_count: Uint64::from(2u64),
            };

            let block_time = Timestamp::from_nanos(1661231280000000000);

            let offset_minutes = 10;

            let divisions = vec![
                Division::new(
                    block_time
                        .minus_nanos(config.window_size.u64() * 2)
                        .plus_minutes(offset_minutes),
                    block_time
                        .minus_nanos(config.window_size.u64() * 2)
                        .plus_minutes(offset_minutes)
                        .plus_minutes(10),
                    Decimal::percent(10),
                    Decimal::percent(10),
                )
                .unwrap(),
                Division::new(
                    block_time
                        .minus_nanos(config.window_size.u64() * 2)
                        .plus_nanos(config.division_size().unwrap().u64())
                        .plus_minutes(offset_minutes),
                    block_time
                        .minus_nanos(config.window_size.u64() * 2)
                        .plus_nanos(config.division_size().unwrap().u64())
                        .plus_minutes(offset_minutes)
                        .plus_minutes(20),
                    Decimal::percent(20),
                    Decimal::percent(10),
                )
                .unwrap(),
                Division::new(
                    block_time
                        .minus_nanos(config.window_size.u64() * 2)
                        .plus_nanos(config.division_size().unwrap().u64() * 2)
                        .plus_minutes(offset_minutes),
                    block_time
                        .minus_nanos(config.window_size.u64() * 2)
                        .plus_nanos(config.division_size().unwrap().u64() * 2)
                        .plus_minutes(offset_minutes)
                        .plus_minutes(40),
                    Decimal::percent(30),
                    Decimal::percent(20),
                )
                .unwrap(),
            ];

            let limiter = ChangeLimiter {
                divisions: divisions.clone(),
                latest_value: Decimal::percent(30),
                window_config: config,
                boundary_offset: Decimal::percent(10),
            };
            let (latest_removed_division, limiter) =
                limiter.clean_up_outdated_divisions(block_time).unwrap();

            assert_eq!(latest_removed_division, Some(divisions[0].clone()));
            assert_eq!(limiter.divisions, divisions[1..].to_vec());

            // with all outdated divisions
            let config = WindowConfig {
                window_size: Uint64::from(3_600_000_000_000u64),
                division_count: Uint64::from(2u64),
            };

            let block_time = Timestamp::from_nanos(1661231280000000000);

            let offset_minutes = 0;

            let divisions = vec![
                Division::new(
                    block_time
                        .minus_nanos(config.window_size.u64() * 2)
                        .minus_nanos(config.division_size().unwrap().u64())
                        .plus_minutes(offset_minutes),
                    block_time
                        .minus_nanos(config.window_size.u64() * 2)
                        .minus_nanos(config.division_size().unwrap().u64())
                        .plus_minutes(offset_minutes)
                        .plus_minutes(10),
                    Decimal::percent(10),
                    Decimal::percent(10),
                )
                .unwrap(),
                Division::new(
                    block_time
                        .minus_nanos(config.window_size.u64() * 2)
                        .plus_minutes(offset_minutes),
                    block_time
                        .minus_nanos(config.window_size.u64() * 2)
                        .plus_minutes(offset_minutes)
                        .plus_minutes(20),
                    Decimal::percent(20),
                    Decimal::percent(10),
                )
                .unwrap(),
                Division::new(
                    block_time
                        .minus_nanos(config.window_size.u64() * 2)
                        .plus_nanos(config.division_size().unwrap().u64())
                        .plus_minutes(offset_minutes),
                    block_time
                        .minus_nanos(config.window_size.u64() * 2)
                        .plus_nanos(config.division_size().unwrap().u64())
                        .plus_minutes(offset_minutes)
                        .plus_minutes(40),
                    Decimal::percent(30),
                    Decimal::percent(20),
                )
                .unwrap(),
            ];
            let limiter = ChangeLimiter {
                divisions: divisions.clone(),
                latest_value: Decimal::percent(30),
                window_config: config,
                boundary_offset: Decimal::percent(10),
            };

            let (latest_removed_division, limiter) =
                limiter.clean_up_outdated_divisions(block_time).unwrap();

            assert_eq!(latest_removed_division, Some(divisions[2].clone()));
            assert_eq!(limiter.divisions, vec![]);
        }
    }

    mod check_limits_and_update {
        use std::str::FromStr;

        use super::*;

        #[test]
        fn test_change_limiter_no_clean_up_outdated() {
            let mut deps = mock_dependencies();
            let limiter = Limiters::new("limiters");
            let config = WindowConfig {
                window_size: Uint64::from(3_600_000_000_000u64), // 1 hrs
                division_count: Uint64::from(2u64),              // 30 mins each
            };

            limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "1h",
                    LimiterParams::ChangeLimiter {
                        window_config: config,
                        boundary_offset: Decimal::percent(5),
                    },
                )
                .unwrap();

            // divs are clean, there will set no limit to it
            let block_time = Timestamp::from_nanos(1661231280000000000);
            let value = Decimal::percent(50);
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(Scope::denom("denoma"), (value - EPSILON, value))],
                    block_time,
                )
                .unwrap();

            // check divs count
            assert_eq!(
                list_divisions(&limiter, &Scope::denom("denoma"), "1h", &deps.storage).len(),
                1
            );

            // now, average should be the same as the value regardless of how time pass
            // 50% + 5% = 55% is the boundary
            let block_time = block_time.plus_minutes(10);
            let value = Decimal::percent(55);
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(Scope::denom("denoma"), (value - EPSILON, value))],
                    block_time,
                )
                .unwrap();

            assert_eq!(
                list_divisions(&limiter, &Scope::denom("denoma"), "1h", &deps.storage).len(),
                1
            );

            // now, average = (50% x 600000000000 + 55% x 300000000000) / 900000000000 = 0.53
            let block_time = block_time.plus_minutes(15);
            let value = Decimal::from_str("0.580000000000000001").unwrap(); // 53% + 5% = 58%
            let err = limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(Scope::denom("denoma"), (value - EPSILON, value))],
                    block_time,
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::UpperLimitExceeded {
                    scope: Scope::denom("denoma"),
                    upper_limit: Decimal::percent(58),
                    value: Decimal::from_str("0.580000000000000001").unwrap(),
                }
            );

            assert_eq!(
                list_divisions(&limiter, &Scope::denom("denoma"), "1h", &deps.storage).len(),
                1
            );

            // pass the first division
            let block_time = block_time.plus_minutes(15); // -> + 40 mins
            let value = Decimal::from_str("0.587500000000000001").unwrap();

            let err = limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(Scope::denom("denoma"), (value - EPSILON, value))],
                    block_time,
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::UpperLimitExceeded {
                    scope: Scope::denom("denoma"),
                    upper_limit: Decimal::from_str("0.5875").unwrap(),
                    value: Decimal::from_str("0.587500000000000001").unwrap(),
                }
            );

            assert_eq!(
                list_divisions(&limiter, &Scope::denom("denoma"), "1h", &deps.storage).len(),
                1
            );

            let value = Decimal::percent(40);
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(Scope::denom("denoma"), (value - EPSILON, value))],
                    block_time,
                )
                .unwrap();

            assert_eq!(
                list_divisions(&limiter, &Scope::denom("denoma"), "1h", &deps.storage).len(),
                2
            );

            let block_time = block_time.plus_minutes(10); // -> + 50 mins
            let value = Decimal::from_str("0.560000000000000001").unwrap();

            let err = limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(Scope::denom("denoma"), (value - EPSILON, value))],
                    block_time,
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::UpperLimitExceeded {
                    scope: Scope::denom("denoma"),
                    upper_limit: Decimal::from_str("0.56").unwrap(),
                    value: Decimal::from_str("0.560000000000000001").unwrap(),
                }
            );

            // pass 2nd division
            let block_time = block_time.plus_minutes(20); // -> + 70 mins
            let value = Decimal::from_str("0.525000000000000001").unwrap();

            let err = limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(Scope::denom("denoma"), (value - EPSILON, value))],
                    block_time,
                )
                .unwrap_err();

            assert_eq!(
                list_divisions(&limiter, &Scope::denom("denoma"), "1h", &deps.storage).len(),
                2
            );

            assert_eq!(
                err,
                ContractError::UpperLimitExceeded {
                    scope: Scope::denom("denoma"),
                    upper_limit: Decimal::from_str("0.525").unwrap(),
                    value: Decimal::from_str("0.525000000000000001").unwrap(),
                }
            );

            let value = Decimal::from_str("0.525").unwrap();
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(Scope::denom("denoma"), (value - EPSILON, value))],
                    block_time,
                )
                .unwrap();

            assert_eq!(
                list_divisions(&limiter, &Scope::denom("denoma"), "1h", &deps.storage).len(),
                3
            );
        }

        #[test]
        fn test_change_limiter_with_clean_up_outdated() {
            let mut deps = mock_dependencies();
            let limiter = Limiters::new("limiters");
            let config = WindowConfig {
                window_size: Uint64::from(3_600_000_000_000u64), // 1 hrs
                division_count: Uint64::from(4u64),              // 15 mins each
            };
            limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denomb"),
                    "1h",
                    LimiterParams::ChangeLimiter {
                        window_config: config,
                        boundary_offset: Decimal::percent(1),
                    },
                )
                .unwrap();

            limiter
                .set_change_limiter_boundary_offset(
                    &mut deps.storage,
                    Scope::denom("denomb"),
                    "1h",
                    Decimal::percent(5),
                )
                .unwrap();

            // divs are clean, there will set no limit to it
            let block_time = Timestamp::from_nanos(1661231280000000000);
            let value = Decimal::percent(40);
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(Scope::denom("denomb"), (value - EPSILON, value))],
                    block_time,
                )
                .unwrap();

            assert_eq!(
                list_divisions(&limiter, &Scope::denom("denomb"), "1h", &deps.storage).len(),
                1
            );

            let block_time = block_time.plus_minutes(10); // -> + 10 mins
            let value = Decimal::percent(45);
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(Scope::denom("denomb"), (value - EPSILON, value))],
                    block_time,
                )
                .unwrap();

            assert_eq!(
                list_divisions(&limiter, &Scope::denom("denomb"), "1h", &deps.storage).len(),
                1
            );

            let block_time = block_time.plus_minutes(60); // -> + 70 mins
            let value = Decimal::from_str("0.500000000000000001").unwrap();
            let err = limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(Scope::denom("denomb"), (value - EPSILON, value))],
                    block_time,
                )
                .unwrap_err();

            assert_eq!(
                list_divisions(&limiter, &Scope::denom("denomb"), "1h", &deps.storage).len(),
                1
            );

            assert_eq!(
                err,
                ContractError::UpperLimitExceeded {
                    scope: Scope::denom("denomb"),
                    upper_limit: Decimal::from_str("0.5").unwrap(),
                    value: Decimal::from_str("0.500000000000000001").unwrap(),
                }
            );

            let value = Decimal::percent(40);
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(Scope::denom("denomb"), (value - EPSILON, value))],
                    block_time,
                )
                .unwrap();

            // 1st division stiil there
            assert_eq!(
                list_divisions(&limiter, &Scope::denom("denomb"), "1h", &deps.storage).len(),
                2
            );

            let block_time = block_time.plus_minutes(10); // -> + 80 mins
            let value = Decimal::from_str("0.491666666666666667").unwrap();
            let err = limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(Scope::denom("denomb"), (value - EPSILON, value))],
                    block_time,
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::UpperLimitExceeded {
                    scope: Scope::denom("denomb"),
                    upper_limit: Decimal::from_str("0.491666666666666666").unwrap(),
                    value: Decimal::from_str("0.491666666666666667").unwrap(),
                }
            );

            // 1st division is not removed yet since limit exceeded first
            assert_eq!(
                list_divisions(&limiter, &Scope::denom("denomb"), "1h", &deps.storage).len(),
                2
            );

            let old_divs = list_divisions(&limiter, &Scope::denom("denomb"), "1h", &deps.storage);
            let value = Decimal::from_str("0.491666666666666666").unwrap();
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(Scope::denom("denomb"), (value - EPSILON, value))],
                    block_time,
                )
                .unwrap();

            // 1st division is removed, and add new division
            assert_eq!(
                list_divisions(&limiter, &Scope::denom("denomb"), "1h", &deps.storage),
                [
                    old_divs[1..].to_vec(),
                    vec![Division::new(
                        block_time.minus_minutes(5), // @75 (= 15 * 5)
                        block_time,
                        value,
                        Decimal::percent(40),
                    )
                    .unwrap()]
                ]
                .concat(),
            );
        }

        #[test]
        fn test_change_limiter_with_skipped_windows() {
            let mut deps = mock_dependencies();
            let limiter = Limiters::new("limiters");
            let config = WindowConfig {
                window_size: Uint64::from(3_600_000_000_000u64), // 1 hrs
                division_count: Uint64::from(4u64),              // 15 mins each
            };
            limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denomb"),
                    "1h",
                    LimiterParams::ChangeLimiter {
                        window_config: config,
                        boundary_offset: Decimal::percent(1),
                    },
                )
                .unwrap();

            limiter
                .set_change_limiter_boundary_offset(
                    &mut deps.storage,
                    Scope::denom("denomb"),
                    "1h",
                    Decimal::percent(5),
                )
                .unwrap();

            let block_time = Timestamp::from_nanos(1661231280000000000);
            let value = Decimal::percent(40);
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(Scope::denom("denomb"), (value - EPSILON, value))],
                    block_time,
                )
                .unwrap();

            let block_time = block_time.plus_minutes(20); // -> + 20 mins
            let value = Decimal::percent(45);
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(Scope::denom("denomb"), (value - EPSILON, value))],
                    block_time,
                )
                .unwrap();

            let block_time = block_time.plus_minutes(30); // -> + 50 mins
            let value = Decimal::percent(46);
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(Scope::denom("denomb"), (value - EPSILON, value))],
                    block_time,
                )
                .unwrap();

            let block_time = block_time.plus_minutes(70); // -> + 120 mins
            let value = Decimal::from_str("0.510000000000000001").unwrap();

            let err = limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(Scope::denom("denomb"), (value - EPSILON, value))],
                    block_time,
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::UpperLimitExceeded {
                    scope: Scope::denom("denomb"),
                    upper_limit: Decimal::percent(51),
                    value
                }
            );
        }

        #[test]
        fn test_change_limiters_away_from_limit() {
            let mut deps = mock_dependencies();
            let limiter = Limiters::new("limiters");

            limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denom"),
                    "1h",
                    LimiterParams::ChangeLimiter {
                        window_config: WindowConfig {
                            window_size: Uint64::from(3_600_000_000_000u64),
                            division_count: Uint64::from(4u64),
                        },
                        boundary_offset: Decimal::percent(1),
                    },
                )
                .unwrap();

            let block_time = Timestamp::from_nanos(1661231280000000000);

            // Start and set the limit
            let value = Decimal::percent(55); // starting limit = 56
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(Scope::denom("denom"), (Decimal::zero(), value))],
                    block_time,
                )
                .unwrap();

            // Increasing value should fail
            let new_block_time = block_time.plus_nanos(900_000_000_000); // 15 minutes later
            let new_value = Decimal::percent(57);
            let err = limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(Scope::denom("denom"), (value, new_value))],
                    new_block_time,
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::UpperLimitExceeded {
                    scope: Scope::denom("denom"),
                    upper_limit: Decimal::percent(56),
                    value: new_value,
                }
            );

            // Move away from limit but still above limit
            let value = Decimal::percent(58);
            let new_value = Decimal::percent(57);
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(Scope::denom("denom"), (value, new_value))],
                    new_block_time,
                )
                .unwrap();

            // Move away from limit within the window
            let new_block_time = block_time.plus_nanos(900_000_000_000); // 15 minutes later
            let value = Decimal::percent(58);
            let new_value = Decimal::percent(54); // Moving away from the limit

            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(Scope::denom("denom"), (value, new_value))],
                    new_block_time,
                )
                .unwrap();

            // Try to move further away from the limit
            let final_block_time = new_block_time.plus_nanos(900_000_000_000); // Another 15 minutes later
            let value = Decimal::percent(58);
            let final_value = Decimal::percent(52); // Moving even further away from the limit

            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(Scope::denom("denom"), (value, final_value))],
                    final_block_time,
                )
                .unwrap();

            // Increasing the value from there should fail
            let value = Decimal::percent(58);
            let new_value = Decimal::percent(59);
            let err = limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(Scope::denom("denom"), (value, new_value))],
                    final_block_time,
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::UpperLimitExceeded {
                    scope: Scope::denom("denom"),
                    upper_limit: Decimal::from_str("0.555").unwrap(),
                    value: new_value,
                }
            );
        }

        #[test]
        fn test_static_limiter() {
            let mut deps = mock_dependencies();
            let limiter = Limiters::new("limiters");

            limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "1h",
                    LimiterParams::StaticLimiter {
                        upper_limit: Decimal::percent(60),
                    },
                )
                .unwrap();

            limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denomb"),
                    "1h",
                    LimiterParams::StaticLimiter {
                        upper_limit: Decimal::percent(70),
                    },
                )
                .unwrap();

            let block_time = Timestamp::from_nanos(1661231280000000000);
            let value_a = Decimal::percent(40);
            let value_b = Decimal::percent(45);

            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![
                        (Scope::denom("denoma"), (value_a - EPSILON, value_a)),
                        (Scope::denom("denomb"), (value_b + EPSILON, value_b)),
                    ],
                    block_time,
                )
                .unwrap();

            let value_a = Decimal::from_str("0.600000000000000001").unwrap();
            let value_b = Decimal::one() - value_a;

            let err = limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![
                        (Scope::denom("denoma"), (value_a - EPSILON, value_a)),
                        (Scope::denom("denomb"), (value_b + EPSILON, value_b)),
                    ],
                    block_time,
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::UpperLimitExceeded {
                    scope: Scope::denom("denoma"),
                    upper_limit: Decimal::from_str("0.6").unwrap(),
                    value: Decimal::from_str("0.600000000000000001").unwrap(),
                }
            );

            let value_b = Decimal::from_str("0.700000000000000001").unwrap();
            let value_a = Decimal::one() - value_b;

            let err = limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![
                        (Scope::denom("denoma"), (value_a + EPSILON, value_a)),
                        (Scope::denom("denomb"), (value_b - EPSILON, value_b)),
                    ],
                    block_time,
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::UpperLimitExceeded {
                    scope: Scope::denom("denomb"),
                    upper_limit: Decimal::from_str("0.7").unwrap(),
                    value: Decimal::from_str("0.700000000000000001").unwrap(),
                }
            );

            let value_a = Decimal::from_str("0.6").unwrap();
            let value_b = Decimal::one() - value_a;

            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![
                        (Scope::denom("denoma"), (value_a - EPSILON, value_a)),
                        (Scope::denom("denomb"), (value_b + EPSILON, value_b)),
                    ],
                    block_time,
                )
                .unwrap();

            let value_b = Decimal::from_str("0.7").unwrap();
            let value_a = Decimal::one() - value_b;

            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![
                        (Scope::denom("denoma"), (value_a - EPSILON, value_a)),
                        (Scope::denom("denomb"), (value_b + EPSILON, value_b)),
                    ],
                    block_time,
                )
                .unwrap();

            // Test case where start value is over limit but decreasing, even if not yet under limit
            let value_b = Decimal::from_str("0.75").unwrap(); // Start above 0.7 limit
            let value_a = Decimal::one() - value_b;

            let new_value_b = Decimal::from_str("0.72").unwrap(); // Decrease, but still above 0.7 limit
            let new_value_a = Decimal::one() - new_value_b;

            // This should not error, as we're moving in the right direction
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![
                        (Scope::denom("denoma"), (value_a, new_value_a)),
                        (Scope::denom("denomb"), (value_b, new_value_b)),
                    ],
                    block_time,
                )
                .unwrap();

            // Test case where start value is over limit but decreasing for denom a
            let value_a = Decimal::from_str("0.65").unwrap(); // Start above 0.6 limit
            let value_b = Decimal::one() - value_a;

            let new_value_a = Decimal::from_str("0.62").unwrap(); // Decrease, but still above 0.6 limit
            let new_value_b = Decimal::one() - new_value_a;

            // This should not error, as we're moving in the right direction for denom a
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![
                        (Scope::denom("denoma"), (value_a, new_value_a)),
                        (Scope::denom("denomb"), (value_b, new_value_b)),
                    ],
                    block_time,
                )
                .unwrap();
        }

        #[test]
        fn test_multiple_registered_limiters() {
            let mut deps = mock_dependencies();
            let limiter = Limiters::new("limiters");
            let config_1h = WindowConfig {
                window_size: Uint64::from(3_600_000_000_000u64), // 1 hrs
                division_count: Uint64::from(2u64),              // 30 mins each
            };

            let config_1w = WindowConfig {
                window_size: Uint64::from(25_920_000_000_000u64), // 7 days
                division_count: Uint64::from(2u64),               // 3.5 days each
            };

            // Register multiple limiters
            limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "1h",
                    LimiterParams::ChangeLimiter {
                        window_config: config_1h.clone(),
                        boundary_offset: Decimal::percent(10),
                    },
                )
                .unwrap();

            limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "1w",
                    LimiterParams::ChangeLimiter {
                        window_config: config_1w.clone(),
                        boundary_offset: Decimal::percent(5),
                    },
                )
                .unwrap();

            limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denomb"),
                    "1h",
                    LimiterParams::ChangeLimiter {
                        window_config: config_1h,
                        boundary_offset: Decimal::percent(10),
                    },
                )
                .unwrap();

            limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denomb"),
                    "1w",
                    LimiterParams::ChangeLimiter {
                        window_config: config_1w,
                        boundary_offset: Decimal::percent(5),
                    },
                )
                .unwrap();

            limiter
                .register(
                    &mut deps.storage,
                    Scope::denom("denomb"),
                    "static",
                    LimiterParams::StaticLimiter {
                        upper_limit: Decimal::percent(55),
                    },
                )
                .unwrap();

            // Check limits and update for multiple limiters
            let block_time = Timestamp::from_nanos(1661231280000000000);
            let value_a = Decimal::percent(50);
            let value_b = Decimal::percent(50);
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![
                        (Scope::denom("denoma"), (value_a, value_a)),
                        (Scope::denom("denomb"), (value_b, value_b)),
                    ],
                    block_time,
                )
                .unwrap();

            let block_time = block_time.plus_minutes(60); // -> + 60 mins
            let value = Decimal::from_str("0.600000000000000001").unwrap();

            let err = limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![
                        (Scope::denom("denoma"), (value - EPSILON, value)),
                        (
                            Scope::denom("denomb"),
                            (Decimal::one() - value + EPSILON, Decimal::one() - value),
                        ),
                    ],
                    block_time,
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::UpperLimitExceeded {
                    scope: Scope::denom("denoma"),
                    upper_limit: Decimal::from_str("0.6").unwrap(),
                    value: Decimal::from_str("0.600000000000000001").unwrap(),
                }
            );

            let value_b = Decimal::from_str("0.550000000000000001").unwrap();
            let value_a = Decimal::one() - value_b;

            let err = limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![
                        (Scope::denom("denoma"), (value_a + EPSILON, value_a)),
                        (Scope::denom("denomb"), (value_b - EPSILON, value_b)),
                    ],
                    block_time,
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::UpperLimitExceeded {
                    scope: Scope::denom("denomb"),
                    upper_limit: Decimal::from_str("0.55").unwrap(),
                    value: Decimal::from_str("0.550000000000000001").unwrap(),
                }
            );

            let value_a = Decimal::from_str("0.45").unwrap();
            let value_b = Decimal::one() - value_a;

            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![
                        (Scope::denom("denoma"), (value_a, value_a)),
                        (Scope::denom("denomb"), (value_b, value_b)),
                    ],
                    block_time,
                )
                .unwrap();

            let block_time = block_time.plus_minutes(60); // -> + 120 mins

            // denoma limit for  50% + 45% / 2 = 47.5% -> +5% = 52.5%
            let value_a = Decimal::from_str("0.525000000000000001").unwrap();
            let value_b = Decimal::one() - value_a;

            let err = limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![
                        (Scope::denom("denoma"), (value_a - EPSILON, value_a)),
                        (Scope::denom("denomb"), (value_b + EPSILON, value_b)),
                    ],
                    block_time,
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::UpperLimitExceeded {
                    scope: Scope::denom("denoma"),
                    upper_limit: Decimal::from_str("0.525").unwrap(),
                    value: Decimal::from_str("0.525000000000000001").unwrap(),
                }
            );

            let value_a = Decimal::from_str("0.550000000000000001").unwrap();
            let value_b = Decimal::one() - value_a;

            let err = limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![
                        (Scope::denom("denoma"), (value_a - EPSILON, value_a)),
                        (Scope::denom("denomb"), (value_b + EPSILON, value_b)),
                    ],
                    block_time,
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::UpperLimitExceeded {
                    scope: Scope::denom("denoma"),
                    upper_limit: Decimal::from_str("0.55").unwrap(),
                    value: Decimal::from_str("0.550000000000000001").unwrap(),
                }
            );
        }

        mod modifying_limiter {
            use super::*;

            #[test]
            fn test_set_boundary_offset() {
                let mut deps = mock_dependencies();
                let limiters = Limiters::new("limiters");
                let config = WindowConfig {
                    window_size: Uint64::from(3_600_000_000_000u64), // 1 hrs
                    division_count: Uint64::from(4u64),              // 15 mins each
                };
                limiters
                    .register(
                        &mut deps.storage,
                        Scope::denom("denomc"),
                        "1h",
                        LimiterParams::ChangeLimiter {
                            window_config: config,
                            boundary_offset: Decimal::percent(10),
                        },
                    )
                    .unwrap();

                limiters
                    .register(
                        &mut deps.storage,
                        Scope::denom("denomc"),
                        "static",
                        LimiterParams::StaticLimiter {
                            upper_limit: Decimal::percent(60),
                        },
                    )
                    .unwrap();

                limiters
                    .set_change_limiter_boundary_offset(
                        &mut deps.storage,
                        Scope::denom("denomc"),
                        "1h",
                        Decimal::percent(20),
                    )
                    .unwrap();

                let limiter = match limiters
                    .limiters
                    .load(&deps.storage, (&Scope::denom("denomc").key(), "1h"))
                    .unwrap()
                {
                    Limiter::ChangeLimiter(limiter) => limiter,
                    Limiter::StaticLimiter(_) => panic!("not a change limiter"),
                };

                let boundary_offset = limiter.boundary_offset;

                assert_eq!(boundary_offset, Decimal::percent(20));

                let err = limiters
                    .set_change_limiter_boundary_offset(
                        &mut deps.storage,
                        Scope::denom("denomc"),
                        "static",
                        Decimal::percent(20),
                    )
                    .unwrap_err();

                assert_eq!(
                    err,
                    ContractError::WrongLimiterType {
                        expected: "change_limiter".to_string(),
                        actual: "static_limiter".to_string()
                    }
                );

                let err = limiters
                    .set_change_limiter_boundary_offset(
                        &mut deps.storage,
                        Scope::denom("denomc"),
                        "1h",
                        Decimal::zero(),
                    )
                    .unwrap_err();

                assert_eq!(err, ContractError::ZeroBoundaryOffset {});
            }

            #[test]
            fn test_set_upper_limit() {
                let mut deps = mock_dependencies();
                let limiters = Limiters::new("limiters");
                let config = WindowConfig {
                    window_size: Uint64::from(3_600_000_000_000u64), // 1 hrs
                    division_count: Uint64::from(4u64),              // 15 mins each
                };
                limiters
                    .register(
                        &mut deps.storage,
                        Scope::denom("denomc"),
                        "1h",
                        LimiterParams::ChangeLimiter {
                            window_config: config,
                            boundary_offset: Decimal::percent(10),
                        },
                    )
                    .unwrap();

                limiters
                    .register(
                        &mut deps.storage,
                        Scope::denom("denomc"),
                        "static",
                        LimiterParams::StaticLimiter {
                            upper_limit: Decimal::percent(60),
                        },
                    )
                    .unwrap();

                let upper_limit = Decimal::percent(70);
                limiters
                    .set_static_limiter_upper_limit(
                        &mut deps.storage,
                        Scope::denom("denomc"),
                        "static",
                        upper_limit,
                    )
                    .unwrap();

                let limiter = match limiters
                    .limiters
                    .load(&deps.storage, (&Scope::denom("denomc").key(), "static"))
                    .unwrap()
                {
                    Limiter::StaticLimiter(limiter) => limiter,
                    Limiter::ChangeLimiter(_) => panic!("not a static limiter"),
                };

                assert_eq!(limiter.upper_limit, upper_limit);

                let err = limiters
                    .set_static_limiter_upper_limit(
                        &mut deps.storage,
                        Scope::denom("denomc"),
                        "1h",
                        upper_limit,
                    )
                    .unwrap_err();

                assert_eq!(
                    err,
                    ContractError::WrongLimiterType {
                        expected: "static_limiter".to_string(),
                        actual: "change_limiter".to_string()
                    }
                );
            }
        }
    }

    mod reset_change_limiter_states {
        use cosmwasm_std::Order;

        use super::*;

        #[test]
        fn test_reset_change_limiter_states() {
            let mut deps = mock_dependencies();
            let limiters = Limiters::new("limiters");

            // register 2 change limiters
            let config_1h = WindowConfig {
                window_size: Uint64::from(3_600_000_000_000u64), // 1 hrs
                division_count: Uint64::from(2u64),              // 30 mins each
            };

            let config_1w = WindowConfig {
                window_size: Uint64::from(25_920_000_000_000u64), // 7 days
                division_count: Uint64::from(2u64),               // 3.5 days each
            };

            limiters
                .register(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "1h",
                    LimiterParams::ChangeLimiter {
                        window_config: config_1h.clone(),
                        boundary_offset: Decimal::percent(10),
                    },
                )
                .unwrap();

            limiters
                .register(
                    &mut deps.storage,
                    Scope::denom("denoma"),
                    "1w",
                    LimiterParams::ChangeLimiter {
                        window_config: config_1w.clone(),
                        boundary_offset: Decimal::percent(5),
                    },
                )
                .unwrap();

            // update limiters

            let block_time = Timestamp::from_nanos(1661231280000000000);
            let value = Decimal::one();
            limiters
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(Scope::denom("denoma"), (value - EPSILON, value))],
                    block_time,
                )
                .unwrap();

            let keys = limiters
                .limiters
                .keys(deps.as_ref().storage, None, None, Order::Ascending)
                .collect::<Result<Vec<_>, _>>()
                .unwrap();

            for (denom, window) in keys.iter() {
                let divisions = list_divisions(
                    &limiters,
                    &denom.parse().unwrap(),
                    window.as_str(),
                    &deps.storage,
                );

                assert_eq!(
                    divisions,
                    vec![Division::new(block_time, block_time, value, value).unwrap()]
                )
            }

            assert_dirty_change_limiters_by_scope!(
                &Scope::denom("denoma"),
                &limiters,
                &deps.storage
            );

            // reset limiters
            let block_time = block_time.plus_hours(1);
            let value = Decimal::percent(2);
            limiters
                .reset_change_limiter_states(
                    &mut deps.storage,
                    block_time,
                    vec![(Scope::denom("denoma").key(), value)].into_iter(),
                )
                .unwrap();

            for (denom, window) in keys.iter() {
                let divisions = list_divisions(
                    &limiters,
                    &denom.parse().unwrap(),
                    window.as_str(),
                    &deps.storage,
                );

                assert_eq!(
                    divisions,
                    vec![Division::new(block_time, block_time, value, value).unwrap()]
                );
            }
        }
    }

    mod list_upper_limits {
        use super::*;

        #[test]
        fn test_list_upper_limits() {
            let mut deps = mock_dependencies();
            let limiters = Limiters::new("limiters");

            let block_time = Timestamp::from_seconds(1_000_000);

            limiters
                .register(
                    &mut deps.storage,
                    Scope::denom("denom1"),
                    "static",
                    LimiterParams::StaticLimiter {
                        upper_limit: Decimal::percent(40),
                    },
                )
                .unwrap();

            limiters
                .register(
                    &mut deps.storage,
                    Scope::denom("denom2"),
                    "static",
                    LimiterParams::StaticLimiter {
                        upper_limit: Decimal::percent(45),
                    },
                )
                .unwrap();

            limiters
                .register(
                    &mut deps.storage,
                    Scope::denom("denom3"),
                    "static",
                    LimiterParams::StaticLimiter {
                        upper_limit: Decimal::percent(30),
                    },
                )
                .unwrap();

            limiters
                .register(
                    &mut deps.storage,
                    Scope::asset_group("asset_group"),
                    "static",
                    LimiterParams::StaticLimiter {
                        upper_limit: Decimal::percent(60),
                    },
                )
                .unwrap();

            let upper_limits = limiters.upper_limits(&deps.storage, block_time).unwrap();
            assert_eq!(
                upper_limits,
                vec![
                    (Scope::denom("denom1"), Decimal::percent(40)),
                    (Scope::denom("denom2"), Decimal::percent(45)),
                    (Scope::denom("denom3"), Decimal::percent(30)),
                    (Scope::asset_group("asset_group"), Decimal::percent(60)),
                ]
                .into_iter()
                .collect()
            );

            // add tigher limiter for denom1
            limiters
                .set_static_limiter_upper_limit(
                    &mut deps.storage,
                    Scope::denom("denom1"),
                    "static",
                    Decimal::percent(35),
                )
                .unwrap();

            let upper_limits = limiters.upper_limits(&deps.storage, block_time).unwrap();
            assert_eq!(
                upper_limits,
                vec![
                    (Scope::denom("denom1"), Decimal::percent(35)),
                    (Scope::denom("denom2"), Decimal::percent(45)),
                    (Scope::denom("denom3"), Decimal::percent(30)),
                    (Scope::asset_group("asset_group"), Decimal::percent(60)),
                ]
                .into_iter()
                .collect()
            );

            // add laxer limiter for asset_group
            limiters
                .register(
                    &mut deps.storage,
                    Scope::asset_group("asset_group"),
                    "more_static",
                    LimiterParams::StaticLimiter {
                        upper_limit: Decimal::percent(70),
                    },
                )
                .unwrap();

            let upper_limits = limiters.upper_limits(&deps.storage, block_time).unwrap();
            assert_eq!(
                upper_limits,
                vec![
                    (Scope::denom("denom1"), Decimal::percent(35)),
                    (Scope::denom("denom2"), Decimal::percent(45)),
                    (Scope::denom("denom3"), Decimal::percent(30)),
                    (Scope::asset_group("asset_group"), Decimal::percent(60)),
                ]
                .into_iter()
                .collect()
            );

            // add change limiter for denom2 and make it tigher than static
            let config_1h = WindowConfig {
                window_size: Uint64::from(3_600_000_000_000u64), // 1 hrs
                division_count: Uint64::from(2u64),              // 30 mins each
            };
            limiters
                .register(
                    &mut deps.storage,
                    Scope::denom("denom2"),
                    "change",
                    LimiterParams::ChangeLimiter {
                        window_config: config_1h,
                        boundary_offset: Decimal::percent(10),
                    },
                )
                .unwrap();

            let upper_limits = limiters.upper_limits(&deps.storage, block_time).unwrap();
            assert_eq!(
                upper_limits,
                vec![
                    (Scope::denom("denom1"), Decimal::percent(35)),
                    (Scope::denom("denom2"), Decimal::percent(45)),
                    (Scope::denom("denom3"), Decimal::percent(30)),
                    (Scope::asset_group("asset_group"), Decimal::percent(60)),
                ]
                .into_iter()
                .collect()
            );

            limiters
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(
                        Scope::denom("denom2"),
                        (Decimal::percent(2), Decimal::percent(5)),
                    )],
                    block_time,
                )
                .unwrap();

            let upper_limits = limiters
                .upper_limits(&deps.storage, block_time.plus_minutes(10))
                .unwrap();
            assert_eq!(
                upper_limits,
                vec![
                    (Scope::denom("denom1"), Decimal::percent(35)),
                    (Scope::denom("denom2"), Decimal::percent(15)),
                    (Scope::denom("denom3"), Decimal::percent(30)),
                    (Scope::asset_group("asset_group"), Decimal::percent(60)),
                ]
                .into_iter()
                .collect()
            );

            // add even tighter change limiter for denom2
            let config_1w = WindowConfig {
                window_size: Uint64::from(25_920_000_000_000u64), // 7 days
                division_count: Uint64::from(10u64),              // 3.5 days each
            };
            limiters
                .register(
                    &mut deps.storage,
                    Scope::denom("denom2"),
                    "even_tighter",
                    LimiterParams::ChangeLimiter {
                        window_config: config_1w,
                        boundary_offset: Decimal::percent(1),
                    },
                )
                .unwrap();

            limiters
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(
                        Scope::denom("denom2"),
                        (Decimal::percent(10), Decimal::percent(4)),
                    )],
                    block_time,
                )
                .unwrap();

            let upper_limits = limiters
                .upper_limits(&deps.storage, block_time.plus_minutes(10))
                .unwrap();
            assert_eq!(
                upper_limits,
                vec![
                    (Scope::denom("denom1"), Decimal::percent(35)),
                    (Scope::denom("denom2"), Decimal::percent(5)),
                    (Scope::denom("denom3"), Decimal::percent(30)),
                    (Scope::asset_group("asset_group"), Decimal::percent(60)),
                ]
                .into_iter()
                .collect()
            );

            // add only change limiter for denom4
            let config_1d = WindowConfig {
                window_size: Uint64::from(86_400_000_000_000u64), // 1 day
                division_count: Uint64::from(1u64),               // 1 day
            };
            limiters
                .register(
                    &mut deps.storage,
                    Scope::denom("denom4"),
                    "only_change",
                    LimiterParams::ChangeLimiter {
                        window_config: config_1d,
                        boundary_offset: Decimal::percent(5),
                    },
                )
                .unwrap();

            limiters
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![(
                        Scope::denom("denom4"),
                        (Decimal::percent(10), Decimal::percent(5)),
                    )],
                    block_time,
                )
                .unwrap();

            let upper_limits = limiters
                .upper_limits(&deps.storage, block_time.plus_minutes(10))
                .unwrap();
            assert_eq!(
                upper_limits,
                vec![
                    (Scope::denom("denom1"), Decimal::percent(35)),
                    (Scope::denom("denom2"), Decimal::percent(5)),
                    (Scope::denom("denom3"), Decimal::percent(30)),
                    (Scope::denom("denom4"), Decimal::percent(10)),
                    (Scope::asset_group("asset_group"), Decimal::percent(60)),
                ]
                .into_iter()
                .collect()
            );
        }
    }

    fn list_divisions(
        limiters: &Limiters,
        scope: &Scope,
        window: &str,
        storage: &dyn Storage,
    ) -> Vec<Division> {
        match limiters
            .limiters
            .load(storage, (&scope.key(), window))
            .unwrap()
        {
            Limiter::ChangeLimiter(limiter) => limiter.divisions,
            Limiter::StaticLimiter(_) => panic!("not a change limiter"),
        }
    }
}
