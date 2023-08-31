use std::collections::VecDeque;

use cosmwasm_schema::cw_serde;
use cosmwasm_std::{ensure, Decimal, Storage, Timestamp, Uint64};
use cw_storage_plus::Map;

use crate::ContractError;

use super::compressed_sma_division::CompressedSMADivision;

/// Maximum number of divisions allowed in a window.
/// This limited so that the contract can't be abused by setting a large division count,
/// which will cause high gas usage when checking the limit, cleaning up divisions, etc.
const MAX_DIVISION_COUNT: Uint64 = Uint64::new(10u64);

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

pub struct Limiters<'a> {
    /// Map of (denom, human_readable_window) -> CompressedSMALimiter
    limiters: Map<'a, (&'a str, &'a str), CompressedSMALimiter>,
}
#[cw_serde]
pub struct CompressedSMALimiter {
    /// Divisions in the window, divisions are ordered from oldest to newest.
    /// Kept divisions must exist within or overlap with the window, else
    /// they will be cleaned up.
    divisions: Vec<CompressedSMADivision>,

    /// Latest updated value.
    latest_value: Decimal,

    /// Config for window and divisions
    window_config: WindowConfig,

    /// Offset from the moving average that the value is allowed to be updated to.
    boundary_offset: Decimal,
}

impl CompressedSMALimiter {
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
        .ensure_window_config_constraint()
    }

    fn ensure_window_config_constraint(self) -> Result<Self, ContractError> {
        let config = &self.window_config;
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
        denom: &str,
        value: Decimal,
    ) -> Result<Self, ContractError> {
        let (latest_removed_division, updated_limiter) =
            self.clean_up_outdated_divisions(block_time)?;

        // Check for upper limit if there is any existing division or there is any removed divisions
        let has_any_prev_data_points =
            !updated_limiter.divisions.is_empty() || latest_removed_division.is_some();

        if has_any_prev_data_points {
            let avg = CompressedSMADivision::compressed_moving_average(
                latest_removed_division,
                &updated_limiter.divisions,
                updated_limiter.window_config.division_size()?,
                updated_limiter.window_config.window_size,
                block_time,
            )?;

            // using saturating_add/sub since the overflowed value can't be exceeded anyway
            let upper_limit = avg.saturating_add(updated_limiter.boundary_offset);

            ensure!(
                value <= upper_limit,
                ContractError::ChangeUpperLimitExceeded {
                    denom: denom.to_string(),
                    upper_limit,
                    value,
                }
            );
        }

        Ok(updated_limiter)
    }

    fn update(self, block_time: Timestamp, value: Decimal) -> Result<Self, ContractError> {
        let mut updated_limiter = self;

        let division_size = updated_limiter.window_config.division_size()?;
        let prev_value = updated_limiter.latest_value;
        updated_limiter.latest_value = value;

        updated_limiter.divisions = if updated_limiter.divisions.is_empty() {
            vec![CompressedSMADivision::new(
                block_time, block_time, value, value,
            )?]
        } else {
            // If the division is over, create a new division
            let mut divisions = VecDeque::from(updated_limiter.divisions);
            let latest_division = divisions.pop_back().expect("divisions must not be empty");

            if latest_division.elapsed_time(block_time)? >= division_size {
                let started_at = latest_division.next_started_at(division_size, block_time)?;

                let new_division =
                    CompressedSMADivision::new(started_at, block_time, value, prev_value)?;
                divisions.push_back(latest_division);
                divisions.push_back(new_division);
                divisions.into()
            }
            // else update the current division
            else {
                divisions.push_back(latest_division.update(block_time, value)?);
                divisions.into()
            }
        };

        Ok(updated_limiter)
    }

    fn clean_up_outdated_divisions(
        self,
        block_time: Timestamp,
    ) -> Result<(Option<CompressedSMADivision>, Self), ContractError> {
        let mut latest_removed_division = None;

        let mut divisions = VecDeque::from(self.divisions);

        while let Some(division) = divisions.front() {
            // if window completely passed the division, remove the division

            if division.is_outdated(
                block_time,
                self.window_config.window_size,
                self.window_config.division_size()?,
            )? {
                latest_removed_division = divisions.pop_front();
            } else {
                break;
            }
        }

        Ok((
            latest_removed_division,
            Self {
                divisions: divisions.into(),
                ..self
            },
        ))
    }
}

impl<'a> Limiters<'a> {
    pub const fn new(limiters_namespace: &'a str) -> Self {
        Self {
            limiters: Map::new(limiters_namespace),
        }
    }

    pub fn register(
        &self,
        storage: &mut dyn Storage,
        denom: &str,
        human_readable_window: &str,
        window_config: WindowConfig,
        boundary_offset: Decimal,
    ) -> Result<(), ContractError> {
        let is_registering_limiter_exists = self
            .limiters
            .may_load(storage, (denom, human_readable_window))?
            .is_some();

        ensure!(
            !is_registering_limiter_exists,
            ContractError::LimiterAlreadyExists {
                denom: denom.to_string(),
                human_readable_window: human_readable_window.to_string()
            }
        );

        let limiter = CompressedSMALimiter::new(window_config, boundary_offset)?;
        self.limiters
            .save(storage, (denom, human_readable_window), &limiter)
            .map_err(Into::into)
    }

    pub fn deregister(&self, storage: &mut dyn Storage, denom: &str, human_readable_window: &str) {
        self.limiters
            .remove(storage, (denom, human_readable_window))
    }

    pub fn set_boundary_offset(
        &self,
        storage: &mut dyn Storage,
        denom: &str,
        human_readable_window: &str,
        boundary_offset: Decimal,
    ) -> Result<(), ContractError> {
        self.limiters.update(
            storage,
            (denom, human_readable_window),
            |limiter: Option<CompressedSMALimiter>| -> Result<CompressedSMALimiter, ContractError> {
                let limiter = limiter.ok_or(ContractError::LimiterDoesNotExist {
                    denom: denom.to_string(),
                    human_readable_window: human_readable_window.to_string(),
                })?;

                Ok(CompressedSMALimiter {
                    boundary_offset,
                    ..limiter
                })
            },
        )?;
        Ok(())
    }

    pub fn list_limiters_by_denom(
        &self,
        storage: &dyn Storage,
        denom: &str,
    ) -> Result<Vec<(String, CompressedSMALimiter)>, ContractError> {
        // there is no need to limit, since the number of limiters is expected to be small
        self.limiters
            .prefix(denom)
            .range(storage, None, None, cosmwasm_std::Order::Ascending)
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    #[allow(clippy::type_complexity)]
    pub fn list_limiters(
        &self,
        storage: &dyn Storage,
    ) -> Result<Vec<((String, String), CompressedSMALimiter)>, ContractError> {
        // there is no need to limit, since the number of limiters is expected to be small
        self.limiters
            .range(storage, None, None, cosmwasm_std::Order::Ascending)
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn check_limits_and_update(
        &self,
        storage: &mut dyn Storage,
        denom_value_pairs: Vec<(String, Decimal)>,
        block_time: Timestamp,
    ) -> Result<(), ContractError> {
        for (denom, value) in denom_value_pairs {
            let limiters = self.list_limiters_by_denom(storage, denom.as_str())?;

            for (human_readable_window, limiter) in limiters {
                dbg!(denom.as_str(), human_readable_window.as_str(), value);
                let limiter = limiter
                    .ensure_upper_limit(block_time, denom.as_str(), value)?
                    .update(block_time, value)?;

                // save updated limiter
                self.limiters
                    .save(storage, (denom.as_str(), &human_readable_window), &limiter)?;
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

            limiter
                .register(
                    &mut deps.storage,
                    "denoma",
                    "1m",
                    WindowConfig {
                        window_size: Uint64::from(604_800_000_000u64),
                        division_count: Uint64::from(5u64),
                    },
                    Decimal::percent(10),
                )
                .unwrap();

            assert_eq!(
                limiter.list_limiters(&deps.storage).unwrap(),
                vec![(
                    ("denoma".to_string(), "1m".to_string()),
                    CompressedSMALimiter {
                        divisions: vec![],
                        latest_value: Decimal::zero(),
                        window_config: WindowConfig {
                            window_size: Uint64::from(604_800_000_000u64),
                            division_count: Uint64::from(5u64),
                        },
                        boundary_offset: Decimal::percent(10)
                    }
                )]
            );

            limiter
                .register(
                    &mut deps.storage,
                    "denoma",
                    "1h",
                    WindowConfig {
                        window_size: Uint64::from(3_600_000_000_000u64),
                        division_count: Uint64::from(2u64),
                    },
                    Decimal::percent(10),
                )
                .unwrap();

            assert_eq!(
                limiter.list_limiters(&deps.storage).unwrap(),
                vec![
                    (
                        ("denoma".to_string(), "1h".to_string()),
                        CompressedSMALimiter {
                            divisions: vec![],
                            latest_value: Decimal::zero(),
                            window_config: WindowConfig {
                                window_size: Uint64::from(3_600_000_000_000u64),
                                division_count: Uint64::from(2u64),
                            },
                            boundary_offset: Decimal::percent(10)
                        }
                    ),
                    (
                        ("denoma".to_string(), "1m".to_string()),
                        CompressedSMALimiter {
                            divisions: vec![],
                            latest_value: Decimal::zero(),
                            window_config: WindowConfig {
                                window_size: Uint64::from(604_800_000_000u64),
                                division_count: Uint64::from(5u64),
                            },
                            boundary_offset: Decimal::percent(10)
                        }
                    )
                ]
            );

            limiter
                .register(
                    &mut deps.storage,
                    "denomb",
                    "1m",
                    WindowConfig {
                        window_size: Uint64::from(604_800_000_000u64),
                        division_count: Uint64::from(5u64),
                    },
                    Decimal::percent(10),
                )
                .unwrap();

            assert_eq!(
                limiter.list_limiters(&deps.storage).unwrap(),
                vec![
                    (
                        ("denoma".to_string(), "1h".to_string()),
                        CompressedSMALimiter {
                            divisions: vec![],
                            latest_value: Decimal::zero(),
                            window_config: WindowConfig {
                                window_size: Uint64::from(3_600_000_000_000u64),
                                division_count: Uint64::from(2u64),
                            },
                            boundary_offset: Decimal::percent(10)
                        }
                    ),
                    (
                        ("denoma".to_string(), "1m".to_string()),
                        CompressedSMALimiter {
                            divisions: vec![],
                            latest_value: Decimal::zero(),
                            window_config: WindowConfig {
                                window_size: Uint64::from(604_800_000_000u64),
                                division_count: Uint64::from(5u64),
                            },
                            boundary_offset: Decimal::percent(10)
                        }
                    ),
                    (
                        ("denomb".to_string(), "1m".to_string()),
                        CompressedSMALimiter {
                            divisions: vec![],
                            latest_value: Decimal::zero(),
                            window_config: WindowConfig {
                                window_size: Uint64::from(604_800_000_000u64),
                                division_count: Uint64::from(5u64),
                            },
                            boundary_offset: Decimal::percent(10)
                        }
                    )
                ]
            );

            // list limiters by denom
            assert_eq!(
                limiter
                    .list_limiters_by_denom(&deps.storage, "denoma")
                    .unwrap(),
                vec![
                    (
                        "1h".to_string(),
                        CompressedSMALimiter {
                            divisions: vec![],
                            latest_value: Decimal::zero(),
                            window_config: WindowConfig {
                                window_size: Uint64::from(3_600_000_000_000u64),
                                division_count: Uint64::from(2u64),
                            },
                            boundary_offset: Decimal::percent(10)
                        }
                    ),
                    (
                        "1m".to_string(),
                        CompressedSMALimiter {
                            divisions: vec![],
                            latest_value: Decimal::zero(),
                            window_config: WindowConfig {
                                window_size: Uint64::from(604_800_000_000u64),
                                division_count: Uint64::from(5u64),
                            },
                            boundary_offset: Decimal::percent(10)
                        }
                    )
                ]
            );

            assert_eq!(
                limiter
                    .list_limiters_by_denom(&deps.storage, "denomb")
                    .unwrap(),
                vec![(
                    "1m".to_string(),
                    CompressedSMALimiter {
                        divisions: vec![],
                        latest_value: Decimal::zero(),
                        window_config: WindowConfig {
                            window_size: Uint64::from(604_800_000_000u64),
                            division_count: Uint64::from(5u64),
                        },
                        boundary_offset: Decimal::percent(10)
                    }
                )]
            );
        }

        #[test]
        fn test_register_same_key_fail() {
            let mut deps = mock_dependencies();
            let limiter = Limiters::new("limiters");

            limiter
                .register(
                    &mut deps.storage,
                    "denoma",
                    "1m",
                    WindowConfig {
                        window_size: Uint64::from(604_800_000_000u64),
                        division_count: Uint64::from(5u64),
                    },
                    Decimal::percent(10),
                )
                .unwrap();

            let err = limiter
                .register(
                    &mut deps.storage,
                    "denoma",
                    "1m",
                    WindowConfig {
                        window_size: Uint64::from(604_800_000_000u64),
                        division_count: Uint64::from(10u64),
                    },
                    Decimal::percent(10),
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::LimiterAlreadyExists {
                    denom: "denoma".to_string(),
                    human_readable_window: "1m".to_string()
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
                    "denoma",
                    "1m",
                    WindowConfig {
                        window_size: Uint64::from(604_800_000_000u64),
                        division_count: Uint64::from(5u64),
                    },
                    Decimal::percent(10),
                )
                .unwrap();

            assert_eq!(
                limiter.list_limiters(&deps.storage).unwrap(),
                vec![(
                    ("denoma".to_string(), "1m".to_string()),
                    CompressedSMALimiter {
                        divisions: vec![],
                        latest_value: Decimal::zero(),
                        window_config: WindowConfig {
                            window_size: Uint64::from(604_800_000_000u64),
                            division_count: Uint64::from(5u64),
                        },
                        boundary_offset: Decimal::percent(10)
                    }
                )]
            );

            limiter
                .register(
                    &mut deps.storage,
                    "denoma",
                    "1h",
                    WindowConfig {
                        window_size: Uint64::from(3_600_000_000_000u64),
                        division_count: Uint64::from(2u64),
                    },
                    Decimal::percent(10),
                )
                .unwrap();

            assert_eq!(
                limiter.list_limiters(&deps.storage).unwrap(),
                vec![
                    (
                        ("denoma".to_string(), "1h".to_string()),
                        CompressedSMALimiter {
                            divisions: vec![],
                            latest_value: Decimal::zero(),
                            window_config: WindowConfig {
                                window_size: Uint64::from(3_600_000_000_000u64),
                                division_count: Uint64::from(2u64),
                            },
                            boundary_offset: Decimal::percent(10)
                        }
                    ),
                    (
                        ("denoma".to_string(), "1m".to_string()),
                        CompressedSMALimiter {
                            divisions: vec![],
                            latest_value: Decimal::zero(),
                            window_config: WindowConfig {
                                window_size: Uint64::from(604_800_000_000u64),
                                division_count: Uint64::from(5u64),
                            },
                            boundary_offset: Decimal::percent(10)
                        }
                    )
                ]
            );

            limiter.deregister(&mut deps.storage, "denoma", "1m");

            assert_eq!(
                limiter.list_limiters(&deps.storage).unwrap(),
                vec![(
                    ("denoma".to_string(), "1h".to_string()),
                    CompressedSMALimiter {
                        divisions: vec![],
                        latest_value: Decimal::zero(),
                        window_config: WindowConfig {
                            window_size: Uint64::from(3_600_000_000_000u64),
                            division_count: Uint64::from(2u64),
                        },
                        boundary_offset: Decimal::percent(10)
                    }
                )]
            );

            limiter.deregister(&mut deps.storage, "denoma", "1h");

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
                    "denoma",
                    "1m",
                    WindowConfig {
                        window_size: Uint64::from(604_800_000_001u64),
                        division_count: Uint64::from(9u64),
                    },
                    Decimal::percent(10),
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
                    "denoma",
                    "1m",
                    WindowConfig {
                        window_size: Uint64::from(604_800_000_000u64),
                        division_count: Uint64::from(0u64),
                    },
                    Decimal::percent(10),
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::DivideByZeroError(DivideByZeroError::new(Uint64::from(
                    604_800_000_000u64
                )))
            );
        }

        #[test]
        fn test_fail_due_to_max_division_count_exceeded() {
            let mut deps = mock_dependencies();

            let limiter = Limiters::new("limiters");

            let err = limiter
                .register(
                    &mut deps.storage,
                    "denoma",
                    "1m",
                    WindowConfig {
                        window_size: Uint64::from(660_000_000_000u64),
                        division_count: Uint64::from(11u64),
                    },
                    Decimal::percent(10),
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
                    "denoma",
                    "1m",
                    WindowConfig {
                        window_size: Uint64::from(604_800_000_000u64),
                        division_count: Uint64::from(5u64),
                    },
                    Decimal::percent(10),
                )
                .unwrap();

            let limiters = limiter.list_limiters(&deps.storage).unwrap();

            assert_eq!(
                limiters,
                vec![(
                    ("denoma".to_string(), "1m".to_string()),
                    CompressedSMALimiter {
                        divisions: vec![],
                        latest_value: Decimal::zero(),
                        window_config: WindowConfig {
                            window_size: Uint64::from(604_800_000_000u64),
                            division_count: Uint64::from(5u64),
                        },
                        boundary_offset: Decimal::percent(10)
                    }
                )]
            );
        }
    }

    mod remove_outdated_division {
        use super::*;

        #[test]
        fn test_empty_divisions() {
            let limiter = CompressedSMALimiter {
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
                CompressedSMADivision::new(
                    block_time.minus_nanos(config.window_size.u64()),
                    block_time
                        .minus_nanos(config.window_size.u64())
                        .plus_minutes(10),
                    Decimal::percent(20),
                    Decimal::percent(10),
                )
                .unwrap(),
                CompressedSMADivision::new(
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
            let limiter = CompressedSMALimiter {
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
                CompressedSMADivision::new(
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
                CompressedSMADivision::new(
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
                CompressedSMADivision::new(
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
            let limiter = CompressedSMALimiter {
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
                CompressedSMADivision::new(
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
                CompressedSMADivision::new(
                    block_time.minus_nanos(config.window_size.u64()),
                    block_time
                        .minus_nanos(config.window_size.u64())
                        .plus_minutes(20),
                    Decimal::percent(20),
                    Decimal::percent(10),
                )
                .unwrap(),
                CompressedSMADivision::new(
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
            let limiter = CompressedSMALimiter {
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
                CompressedSMADivision::new(
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
                CompressedSMADivision::new(
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
                CompressedSMADivision::new(
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
            let limiter = CompressedSMALimiter {
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
                CompressedSMADivision::new(
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
                CompressedSMADivision::new(
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
                CompressedSMADivision::new(
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

            let limiter = CompressedSMALimiter {
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
                CompressedSMADivision::new(
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
                CompressedSMADivision::new(
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
                CompressedSMADivision::new(
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
            let limiter = CompressedSMALimiter {
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
        fn test_no_clean_up_outdated() {
            let mut deps = mock_dependencies();
            let limiter = Limiters::new("limiters");
            let config = WindowConfig {
                window_size: Uint64::from(3_600_000_000_000u64), // 1 hrs
                division_count: Uint64::from(2u64),              // 30 mins each
            };

            limiter
                .register(
                    &mut deps.storage,
                    "denoma",
                    "1h",
                    config,
                    Decimal::percent(5),
                )
                .unwrap();

            // divs are clean, there will set no limit to it
            let block_time = Timestamp::from_nanos(1661231280000000000);
            let value = Decimal::percent(50);
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![("denoma".to_string(), value)],
                    block_time,
                )
                .unwrap();

            // check divs count
            assert_eq!(
                list_divisions(&limiter, "denoma", "1h", &deps.storage).len(),
                1
            );

            // now, average should be the same as the value regardless of how time pass
            // 50% + 5% = 55% is the boundary
            let block_time = block_time.plus_minutes(10);
            let value = Decimal::percent(55);
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![("denoma".to_string(), value)],
                    block_time,
                )
                .unwrap();

            assert_eq!(
                list_divisions(&limiter, "denoma", "1h", &deps.storage).len(),
                1
            );

            // now, average = (50% x 600000000000 + 55% x 300000000000) / 900000000000 = 0.53
            let block_time = block_time.plus_minutes(15);
            let value = Decimal::from_str("0.580000000000000001").unwrap(); // 53% + 5% = 58%
            let err = limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![("denoma".to_string(), value)],
                    block_time,
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::ChangeUpperLimitExceeded {
                    denom: "denoma".to_string(),
                    upper_limit: Decimal::percent(58),
                    value: Decimal::from_str("0.580000000000000001").unwrap(),
                }
            );

            assert_eq!(
                list_divisions(&limiter, "denoma", "1h", &deps.storage).len(),
                1
            );

            // pass the first division
            let block_time = block_time.plus_minutes(15); // -> + 40 mins
            let value = Decimal::from_str("0.587500000000000001").unwrap();

            let err = limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![("denoma".to_string(), value)],
                    block_time,
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::ChangeUpperLimitExceeded {
                    denom: "denoma".to_string(),
                    upper_limit: Decimal::from_str("0.5875").unwrap(),
                    value: Decimal::from_str("0.587500000000000001").unwrap(),
                }
            );

            assert_eq!(
                list_divisions(&limiter, "denoma", "1h", &deps.storage).len(),
                1
            );

            let value = Decimal::percent(40);
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![("denoma".to_string(), value)],
                    block_time,
                )
                .unwrap();

            assert_eq!(
                list_divisions(&limiter, "denoma", "1h", &deps.storage).len(),
                2
            );

            let block_time = block_time.plus_minutes(10); // -> + 50 mins
            let value = Decimal::from_str("0.560000000000000001").unwrap();

            let err = limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![("denoma".to_string(), value)],
                    block_time,
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::ChangeUpperLimitExceeded {
                    denom: "denoma".to_string(),
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
                    vec![("denoma".to_string(), value)],
                    block_time,
                )
                .unwrap_err();

            assert_eq!(
                list_divisions(&limiter, "denoma", "1h", &deps.storage).len(),
                2
            );

            assert_eq!(
                err,
                ContractError::ChangeUpperLimitExceeded {
                    denom: "denoma".to_string(),
                    upper_limit: Decimal::from_str("0.525").unwrap(),
                    value: Decimal::from_str("0.525000000000000001").unwrap(),
                }
            );

            let value = Decimal::from_str("0.525").unwrap();
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![("denoma".to_string(), value)],
                    block_time,
                )
                .unwrap();

            assert_eq!(
                list_divisions(&limiter, "denoma", "1h", &deps.storage).len(),
                3
            );
        }

        #[test]
        fn test_with_clean_up_outdated() {
            let mut deps = mock_dependencies();
            let limiter = Limiters::new("limiters");
            let config = WindowConfig {
                window_size: Uint64::from(3_600_000_000_000u64), // 1 hrs
                division_count: Uint64::from(4u64),              // 15 mins each
            };
            limiter
                .register(&mut deps.storage, "denomb", "1h", config, Decimal::zero())
                .unwrap();

            limiter
                .set_boundary_offset(&mut deps.storage, "denomb", "1h", Decimal::percent(5))
                .unwrap();

            // divs are clean, there will set no limit to it
            let block_time = Timestamp::from_nanos(1661231280000000000);
            let value = Decimal::percent(40);
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![("denomb".to_string(), value)],
                    block_time,
                )
                .unwrap();

            assert_eq!(
                list_divisions(&limiter, "denomb", "1h", &deps.storage).len(),
                1
            );

            let block_time = block_time.plus_minutes(10); // -> + 10 mins
            let value = Decimal::percent(45);
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![("denomb".to_string(), value)],
                    block_time,
                )
                .unwrap();

            assert_eq!(
                list_divisions(&limiter, "denomb", "1h", &deps.storage).len(),
                1
            );

            let block_time = block_time.plus_minutes(60); // -> + 70 mins
            let value = Decimal::from_str("0.500000000000000001").unwrap();
            let err = limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![("denomb".to_string(), value)],
                    block_time,
                )
                .unwrap_err();

            assert_eq!(
                list_divisions(&limiter, "denomb", "1h", &deps.storage).len(),
                1
            );

            assert_eq!(
                err,
                ContractError::ChangeUpperLimitExceeded {
                    denom: "denomb".to_string(),
                    upper_limit: Decimal::from_str("0.5").unwrap(),
                    value: Decimal::from_str("0.500000000000000001").unwrap(),
                }
            );

            let value = Decimal::percent(40);
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![("denomb".to_string(), value)],
                    block_time,
                )
                .unwrap();

            // 1st division stiil there
            assert_eq!(
                list_divisions(&limiter, "denomb", "1h", &deps.storage).len(),
                2
            );

            let block_time = block_time.plus_minutes(10); // -> + 80 mins
            let value = Decimal::from_str("0.491666666666666667").unwrap();
            let err = limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![("denomb".to_string(), value)],
                    block_time,
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::ChangeUpperLimitExceeded {
                    denom: "denomb".to_string(),
                    upper_limit: Decimal::from_str("0.491666666666666666").unwrap(),
                    value: Decimal::from_str("0.491666666666666667").unwrap(),
                }
            );

            // 1st division is not removed yet since limit exceeded first
            assert_eq!(
                list_divisions(&limiter, "denomb", "1h", &deps.storage).len(),
                2
            );

            let old_divs = list_divisions(&limiter, "denomb", "1h", &deps.storage);
            let value = Decimal::from_str("0.491666666666666666").unwrap();
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![("denomb".to_string(), value)],
                    block_time,
                )
                .unwrap();

            // 1st division is removed, and add new division
            assert_eq!(
                list_divisions(&limiter, "denomb", "1h", &deps.storage),
                vec![
                    old_divs[1..].to_vec(),
                    vec![CompressedSMADivision::new(
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
        fn test_with_skipped_windows() {
            let mut deps = mock_dependencies();
            let limiter = Limiters::new("limiters");
            let config = WindowConfig {
                window_size: Uint64::from(3_600_000_000_000u64), // 1 hrs
                division_count: Uint64::from(4u64),              // 15 mins each
            };
            limiter
                .register(&mut deps.storage, "denomb", "1h", config, Decimal::zero())
                .unwrap();

            limiter
                .set_boundary_offset(&mut deps.storage, "denomb", "1h", Decimal::percent(5))
                .unwrap();

            let block_time = Timestamp::from_nanos(1661231280000000000);
            let value = Decimal::percent(40);
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![("denomb".to_string(), value)],
                    block_time,
                )
                .unwrap();

            let block_time = block_time.plus_minutes(20); // -> + 20 mins
            let value = Decimal::percent(45);
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![("denomb".to_string(), value)],
                    block_time,
                )
                .unwrap();

            let block_time = block_time.plus_minutes(30); // -> + 50 mins
            let value = Decimal::percent(46);
            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![("denomb".to_string(), value)],
                    block_time,
                )
                .unwrap();

            let block_time = block_time.plus_minutes(70); // -> + 120 mins
            let value = Decimal::from_str("0.510000000000000001").unwrap();

            let err = limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![("denomb".to_string(), value)],
                    block_time,
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::ChangeUpperLimitExceeded {
                    denom: String::from("denomb"),
                    upper_limit: Decimal::percent(51),
                    value
                }
            );
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
                    "denoma",
                    "1h",
                    config_1h.clone(),
                    Decimal::percent(10),
                )
                .unwrap();

            limiter
                .register(
                    &mut deps.storage,
                    "denoma",
                    "1w",
                    config_1w.clone(),
                    Decimal::percent(5),
                )
                .unwrap();

            limiter
                .register(
                    &mut deps.storage,
                    "denomb",
                    "1h",
                    config_1h,
                    Decimal::percent(10),
                )
                .unwrap();

            limiter
                .register(
                    &mut deps.storage,
                    "denomb",
                    "1w",
                    config_1w,
                    Decimal::percent(5),
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
                        ("denoma".to_string(), value_a),
                        ("denomb".to_string(), value_b),
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
                        ("denoma".to_string(), value),
                        ("denomb".to_string(), Decimal::one() - value),
                    ],
                    block_time,
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::ChangeUpperLimitExceeded {
                    denom: "denoma".to_string(),
                    upper_limit: Decimal::from_str("0.6").unwrap(),
                    value: Decimal::from_str("0.600000000000000001").unwrap(),
                }
            );

            let value_a = Decimal::from_str("0.45").unwrap();
            let value_b = Decimal::one() - value_a;

            limiter
                .check_limits_and_update(
                    &mut deps.storage,
                    vec![
                        ("denoma".to_string(), value_a),
                        ("denomb".to_string(), value_b),
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
                        ("denoma".to_string(), value_a),
                        ("denomb".to_string(), value_b),
                    ],
                    block_time,
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::ChangeUpperLimitExceeded {
                    denom: "denoma".to_string(),
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
                        ("denoma".to_string(), value_a),
                        ("denomb".to_string(), value_b),
                    ],
                    block_time,
                )
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::ChangeUpperLimitExceeded {
                    denom: "denoma".to_string(),
                    upper_limit: Decimal::from_str("0.55").unwrap(),
                    value: Decimal::from_str("0.550000000000000001").unwrap(),
                }
            );
        }
    }

    fn list_divisions(
        limiter: &Limiters,
        denom: &str,
        humanized_window_size: &str,
        storage: &dyn Storage,
    ) -> Vec<CompressedSMADivision> {
        limiter
            .limiters
            .load(storage, (denom, humanized_window_size))
            .unwrap()
            .divisions
    }
}
