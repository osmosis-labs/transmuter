use cosmwasm_schema::cw_serde;
use cosmwasm_std::{ensure, Decimal, Storage, Timestamp, Uint64};
use cw_storage_plus::{Deque, Item};

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

pub struct CompressedSMALimiter<'a> {
    /// Denom of the token that this limiter keeps track the limit of.
    denom: String,

    /// Config for window and divisions
    window_config: Item<'a, WindowConfig>,

    /// Divisions in the window, divisions are ordered from oldest to newest.
    /// Kept divisions must exist within or overlap with the window, else
    /// they will be cleaned up.
    divisions: Deque<'a, CompressedSMADivision>,

    /// Latest updated value.
    latest_value: Item<'a, Decimal>,

    /// Offset from the moving average that the value is allowed to be updated to.
    boundary_offset: Item<'a, Decimal>,
}

impl<'a> CompressedSMALimiter<'a> {
    const fn new(
        denom: String,
        window_config_namespace: &'a str,
        divisions_namespace: &'a str,
        boundary_offset_namespace: &'a str,
        latest_value_namespace: &'a str,
    ) -> Self {
        Self {
            denom,
            window_config: Item::new(window_config_namespace),
            divisions: Deque::new(divisions_namespace),
            boundary_offset: Item::new(boundary_offset_namespace),
            latest_value: Item::new(latest_value_namespace),
        }
    }

    pub fn set_window_config(
        &self,
        storage: &mut dyn Storage,
        config: &WindowConfig,
    ) -> Result<(), ContractError> {
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

        // clean up all existing divisions
        self.clean_up_all_divisions(storage)?;

        // update config
        self.window_config.save(storage, config).map_err(Into::into)
    }

    pub fn set_boundary_offset(
        &self,
        storage: &mut dyn Storage,
        boundary_offset: Decimal,
    ) -> Result<(), ContractError> {
        self.boundary_offset
            .save(storage, &boundary_offset)
            .map_err(Into::into)
    }

    /// Check if the value is within the limit and update the divisions
    pub fn check_limit_and_update(
        &self,
        storage: &mut dyn Storage,
        block_time: Timestamp,
        value: Decimal,
    ) -> Result<(), ContractError> {
        // clean up old divisions that are not in the window anymore
        let latest_removed_division = self.clean_up_outdated_divisions(storage, block_time)?;

        let config = self.window_config.load(storage)?;
        let latest_division = self.divisions.back(storage)?;
        let division_size = config.division_size()?;

        // Check for upper limit if there is any existing division or there is any removed divisions
        let has_any_prev_data_points =
            latest_division.is_some() || latest_removed_division.is_some();
        if has_any_prev_data_points {
            self.ensure_value_within_upper_limit(
                storage,
                division_size,
                config.window_size,
                block_time,
                latest_removed_division,
                value,
            )?;
        }

        // update latest value
        let prev_value = self.latest_value.may_load(storage)?.unwrap_or_default();
        self.latest_value.save(storage, &value)?;

        match latest_division {
            Some(division) => {
                // If the division is over, create a new division
                if division.elapsed_time(block_time)? >= division_size {
                    let started_at = division.next_started_at(division_size, block_time)?;

                    let new_division =
                        CompressedSMADivision::new(started_at, block_time, value, prev_value)?;
                    self.divisions.push_back(storage, &new_division)?;
                }
                // else update the current division
                else {
                    self.divisions.pop_back(storage)?;
                    let updated_division = division.update(block_time, value)?;

                    self.divisions.push_back(storage, &updated_division)?;
                }
            }
            None => {
                let new_division =
                    CompressedSMADivision::new(block_time, block_time, value, value)?;
                self.divisions.push_back(storage, &new_division)?;
            }
        };

        Ok(())
    }

    fn clean_up_all_divisions(&self, storage: &mut dyn Storage) -> Result<(), ContractError> {
        while (self.divisions.pop_back(storage)?).is_some() {}
        Ok(())
    }

    fn clean_up_outdated_divisions(
        &self,
        storage: &mut dyn Storage,
        block_time: Timestamp,
    ) -> Result<Option<CompressedSMADivision>, ContractError> {
        let config = self.window_config.load(storage)?;

        let mut latest_removed_division = None;

        while let Some(division) = self.divisions.front(storage)? {
            // if window completely passed the division, remove the division
            if division.is_outdated(block_time, config.window_size, config.division_size()?)? {
                latest_removed_division = self.divisions.pop_front(storage)?;
            } else {
                break;
            }
        }

        Ok(latest_removed_division)
    }

    fn ensure_value_within_upper_limit(
        &self,
        storage: &dyn Storage,
        division_size: Uint64,
        window_size: Uint64,
        block_time: Timestamp,
        latest_removed_division: Option<CompressedSMADivision>,
        value: Decimal,
    ) -> Result<(), ContractError> {
        let avg = CompressedSMADivision::compressed_moving_average(
            latest_removed_division,
            self.divisions
                .iter(storage)?
                .collect::<Result<Vec<_>, _>>()?,
            division_size,
            window_size,
            block_time,
        )?;

        // using saturating_add/sub since the overflowed value can't be exceeded anyway
        let upper_limit = avg.saturating_add(self.boundary_offset.load(storage)?);

        ensure!(
            value <= upper_limit,
            ContractError::ChangeUpperLimitExceeded {
                denom: self.denom.clone(),
                upper_limit,
                value,
            }
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use cosmwasm_std::testing::mock_dependencies;

    use super::*;

    mod set_config {
        use cosmwasm_std::DivideByZeroError;

        use super::*;

        #[test]
        fn test_fail_due_to_div_count_does_not_evenly_divide_the_window() {
            let mut deps = mock_dependencies();

            let limiter = CompressedSMALimiter::new(
                String::from("denoma"),
                "config",
                "divisions",
                "boundary_offset",
                "latest_value",
            );

            let err = limiter
                .set_window_config(
                    &mut deps.storage,
                    &WindowConfig {
                        window_size: Uint64::from(604_800_000_001u64),
                        division_count: Uint64::from(9u64),
                    },
                )
                .unwrap_err();

            assert_eq!(err, ContractError::UnevenWindowDivision {});
        }

        #[test]
        fn test_fail_due_to_div_size_is_zero() {
            let mut deps = mock_dependencies();

            let limiter = CompressedSMALimiter::new(
                String::from("denoma"),
                "config",
                "divisions",
                "boundary_offset",
                "latest_value",
            );

            let err = limiter
                .set_window_config(
                    &mut deps.storage,
                    &WindowConfig {
                        window_size: Uint64::from(604_800_000_000u64),
                        division_count: Uint64::from(0u64),
                    },
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

            let limiter = CompressedSMALimiter::new(
                String::from("denoma"),
                "config",
                "divisions",
                "boundary_offset",
                "latest_value",
            );

            let err = limiter
                .set_window_config(
                    &mut deps.storage,
                    &WindowConfig {
                        window_size: Uint64::from(660_000_000_000u64),
                        division_count: Uint64::from(11u64),
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

            let limiter = CompressedSMALimiter::new(
                String::from("denoma"),
                "config",
                "divisions",
                "boundary_offset",
                "latest_value",
            );

            limiter
                .set_window_config(
                    &mut deps.storage,
                    &WindowConfig {
                        window_size: Uint64::from(604_800_000_000u64),
                        division_count: Uint64::from(5u64),
                    },
                )
                .unwrap();

            let config = limiter.window_config.load(&deps.storage).unwrap();

            assert_eq!(
                config,
                WindowConfig {
                    window_size: Uint64::from(604_800_000_000u64),
                    division_count: Uint64::from(5u64),
                }
            );
        }

        #[test]
        fn test_clean_up_old_divisions_if_update_config() {
            let mut deps = mock_dependencies();

            let limiter = CompressedSMALimiter::new(
                String::from("denoma"),
                "config",
                "divisions",
                "boundary_offset",
                "latest_value",
            );
            let config = WindowConfig {
                window_size: Uint64::from(3_600_000_000_000u64),
                division_count: Uint64::from(2u64),
            };
            limiter
                .set_window_config(&mut deps.storage, &config)
                .unwrap();

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
            add_compressed_divisions(&mut deps.storage, &limiter, divisions.clone());
            assert_eq!(list_divisions(&limiter, &deps.storage), divisions);

            let config = WindowConfig {
                window_size: Uint64::from(3_600_000_000_000u64),
                division_count: Uint64::from(4u64),
            };
            limiter
                .set_window_config(&mut deps.storage, &config)
                .unwrap();

            assert_eq!(list_divisions(&limiter, &deps.storage), vec![]);
        }
    }

    mod remove_outdated_division {
        use super::*;

        #[test]
        fn test_empty_divisions() {
            let mut deps = mock_dependencies();

            let limiter = CompressedSMALimiter::new(
                String::from("denoma"),
                "config",
                "divisions",
                "boundary_offset",
                "latest_value",
            );
            limiter
                .set_window_config(
                    &mut deps.storage,
                    &WindowConfig {
                        window_size: Uint64::from(3_600_000_000_000u64),
                        division_count: Uint64::from(5u64),
                    },
                )
                .unwrap();

            let block_time = Timestamp::from_nanos(1661231280000000000);
            limiter
                .clean_up_outdated_divisions(&mut deps.storage, block_time)
                .unwrap();

            assert_eq!(list_divisions(&limiter, &deps.storage), vec![]);
        }

        #[test]
        fn test_no_outdated_divisions() {
            let mut deps = mock_dependencies();

            let limiter = CompressedSMALimiter::new(
                String::from("denoma"),
                "config",
                "divisions",
                "boundary_offset",
                "latest_value",
            );
            let config = WindowConfig {
                window_size: Uint64::from(3_600_000_000_000u64),
                division_count: Uint64::from(2u64),
            };
            limiter
                .set_window_config(&mut deps.storage, &config)
                .unwrap();

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
            add_compressed_divisions(&mut deps.storage, &limiter, divisions.clone());
            limiter
                .clean_up_outdated_divisions(&mut deps.storage, block_time)
                .unwrap();
            assert_eq!(list_divisions(&limiter, &deps.storage), divisions);

            // with overlapping divisions
            let mut deps = mock_dependencies();
            let limiter = CompressedSMALimiter::new(
                String::from("denoma"),
                "config",
                "divisions",
                "boundary_offset",
                "latest_value",
            );
            let config = WindowConfig {
                window_size: Uint64::from(3_600_000_000_000u64),
                division_count: Uint64::from(2u64),
            };
            limiter
                .set_window_config(&mut deps.storage, &config)
                .unwrap();

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
            add_compressed_divisions(&mut deps.storage, &limiter, divisions.clone());
            assert_eq!(list_divisions(&limiter, &deps.storage), divisions);
        }

        #[test]
        fn test_with_single_outdated_divisions() {
            let mut deps = mock_dependencies();
            let limiter = CompressedSMALimiter::new(
                String::from("denoma"),
                "config",
                "divisions",
                "boundary_offset",
                "latest_value",
            );
            let config = WindowConfig {
                window_size: Uint64::from(3_600_000_000_000u64),
                division_count: Uint64::from(2u64),
            };
            limiter
                .set_window_config(&mut deps.storage, &config)
                .unwrap();

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
            add_compressed_divisions(&mut deps.storage, &limiter, divisions.clone());
            limiter
                .clean_up_outdated_divisions(&mut deps.storage, block_time)
                .unwrap();
            assert_eq!(
                list_divisions(&limiter, &deps.storage),
                divisions[1..].to_vec()
            );
        }

        #[test]
        fn test_with_multiple_outdated_division() {
            // with no overlapping divisions
            let mut deps = mock_dependencies();
            let limiter = CompressedSMALimiter::new(
                String::from("denoma"),
                "config",
                "divisions",
                "boundary_offset",
                "latest_value",
            );
            let config = WindowConfig {
                window_size: Uint64::from(3_600_000_000_000u64),
                division_count: Uint64::from(2u64),
            };

            limiter
                .set_window_config(&mut deps.storage, &config)
                .unwrap();

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
            add_compressed_divisions(&mut deps.storage, &limiter, divisions.clone());
            limiter
                .clean_up_outdated_divisions(&mut deps.storage, block_time)
                .unwrap();
            assert_eq!(
                list_divisions(&limiter, &deps.storage),
                divisions[2..].to_vec()
            );

            // with some overlapping divisions
            let mut deps = mock_dependencies();
            let limiter = CompressedSMALimiter::new(
                String::from("denoma"),
                "config",
                "divisions",
                "boundary_offset",
                "latest_value",
            );
            let config = WindowConfig {
                window_size: Uint64::from(3_600_000_000_000u64),
                division_count: Uint64::from(2u64),
            };

            limiter
                .set_window_config(&mut deps.storage, &config)
                .unwrap();

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
            add_compressed_divisions(&mut deps.storage, &limiter, divisions.clone());
            limiter
                .clean_up_outdated_divisions(&mut deps.storage, block_time)
                .unwrap();
            assert_eq!(
                list_divisions(&limiter, &deps.storage),
                divisions[1..].to_vec()
            );

            // with all outdated divisions
            let mut deps = mock_dependencies();
            let limiter = CompressedSMALimiter::new(
                String::from("denoma"),
                "config",
                "divisions",
                "boundary_offset",
                "latest_value",
            );
            let config = WindowConfig {
                window_size: Uint64::from(3_600_000_000_000u64),
                division_count: Uint64::from(2u64),
            };

            limiter
                .set_window_config(&mut deps.storage, &config)
                .unwrap();

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
            add_compressed_divisions(&mut deps.storage, &limiter, divisions);
            limiter
                .clean_up_outdated_divisions(&mut deps.storage, block_time)
                .unwrap();
            assert_eq!(list_divisions(&limiter, &deps.storage), vec![]);
        }
    }

    mod check_and_update {
        use std::str::FromStr;

        use super::*;

        #[test]
        fn test_no_clean_up_outdated() {
            let mut deps = mock_dependencies();
            let limiter = CompressedSMALimiter::new(
                String::from("denoma"),
                "config",
                "divisions",
                "boundary_offset",
                "latest_value",
            );
            let config = WindowConfig {
                window_size: Uint64::from(3_600_000_000_000u64), // 1 hrs
                division_count: Uint64::from(2u64),              // 30 mins each
            };
            limiter
                .set_window_config(&mut deps.storage, &config)
                .unwrap();

            limiter
                .set_boundary_offset(&mut deps.storage, Decimal::percent(5))
                .unwrap();

            // divs are clean, there will set no limit to it
            let block_time = Timestamp::from_nanos(1661231280000000000);
            let value = Decimal::percent(50);
            limiter
                .check_limit_and_update(&mut deps.storage, block_time, value)
                .unwrap();

            // check divs count
            assert_eq!(list_divisions(&limiter, &deps.storage).len(), 1);

            // now, average should be the same as the value regardless of how time pass
            // 50% + 5% = 55% is the boundary
            let block_time = block_time.plus_minutes(10);
            let value = Decimal::percent(55);
            limiter
                .check_limit_and_update(&mut deps.storage, block_time, value)
                .unwrap();

            assert_eq!(list_divisions(&limiter, &deps.storage).len(), 1);

            // now, average = (50% x 600000000000 + 55% x 300000000000) / 900000000000 = 0.53
            let block_time = block_time.plus_minutes(15);
            let value = Decimal::from_str("0.580000000000000001").unwrap(); // 53% + 5% = 58%
            let err = limiter
                .check_limit_and_update(&mut deps.storage, block_time, value)
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::ChangeUpperLimitExceeded {
                    denom: "denoma".to_string(),
                    upper_limit: Decimal::percent(58),
                    value: Decimal::from_str("0.580000000000000001").unwrap(),
                }
            );

            assert_eq!(list_divisions(&limiter, &deps.storage).len(), 1);

            // pass the first division
            let block_time = block_time.plus_minutes(15); // -> + 40 mins
            let value = Decimal::from_str("0.587500000000000001").unwrap();

            let err = limiter
                .check_limit_and_update(&mut deps.storage, block_time, value)
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::ChangeUpperLimitExceeded {
                    denom: "denoma".to_string(),
                    upper_limit: Decimal::from_str("0.5875").unwrap(),
                    value: Decimal::from_str("0.587500000000000001").unwrap(),
                }
            );

            assert_eq!(list_divisions(&limiter, &deps.storage).len(), 1);

            let value = Decimal::percent(40);
            limiter
                .check_limit_and_update(&mut deps.storage, block_time, value)
                .unwrap();

            assert_eq!(list_divisions(&limiter, &deps.storage).len(), 2);

            let block_time = block_time.plus_minutes(10); // -> + 50 mins
            let value = Decimal::from_str("0.560000000000000001").unwrap();

            let err = limiter
                .check_limit_and_update(&mut deps.storage, block_time, value)
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
                .check_limit_and_update(&mut deps.storage, block_time, value)
                .unwrap_err();

            assert_eq!(list_divisions(&limiter, &deps.storage).len(), 2);

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
                .check_limit_and_update(&mut deps.storage, block_time, value)
                .unwrap();

            assert_eq!(list_divisions(&limiter, &deps.storage).len(), 3);
        }

        #[test]
        fn test_with_clean_up_outdated() {
            let mut deps = mock_dependencies();
            let limiter = CompressedSMALimiter::new(
                String::from("denomb"),
                "config",
                "divisions",
                "boundary_offset",
                "latest_value",
            );
            let config = WindowConfig {
                window_size: Uint64::from(3_600_000_000_000u64), // 1 hrs
                division_count: Uint64::from(4u64),              // 15 mins each
            };
            limiter
                .set_window_config(&mut deps.storage, &config)
                .unwrap();

            limiter
                .set_boundary_offset(&mut deps.storage, Decimal::percent(5))
                .unwrap();

            // divs are clean, there will set no limit to it
            let block_time = Timestamp::from_nanos(1661231280000000000);
            let value = Decimal::percent(40);
            limiter
                .check_limit_and_update(&mut deps.storage, block_time, value)
                .unwrap();

            assert_eq!(list_divisions(&limiter, &deps.storage).len(), 1);

            let block_time = block_time.plus_minutes(10); // -> + 10 mins
            let value = Decimal::percent(45);
            limiter
                .check_limit_and_update(&mut deps.storage, block_time, value)
                .unwrap();

            assert_eq!(list_divisions(&limiter, &deps.storage).len(), 1);

            let block_time = block_time.plus_minutes(60); // -> + 70 mins
            let value = Decimal::from_str("0.500000000000000001").unwrap();
            let err = limiter
                .check_limit_and_update(&mut deps.storage, block_time, value)
                .unwrap_err();

            assert_eq!(list_divisions(&limiter, &deps.storage).len(), 1);

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
                .check_limit_and_update(&mut deps.storage, block_time, value)
                .unwrap();

            // 1st division stiil there
            assert_eq!(list_divisions(&limiter, &deps.storage).len(), 2);

            let block_time = block_time.plus_minutes(10); // -> + 80 mins
            let value = Decimal::from_str("0.491666666666666667").unwrap();
            let err = limiter
                .check_limit_and_update(&mut deps.storage, block_time, value)
                .unwrap_err();

            // 1st division is gone
            assert_eq!(list_divisions(&limiter, &deps.storage).len(), 1);

            assert_eq!(
                err,
                ContractError::ChangeUpperLimitExceeded {
                    denom: "denomb".to_string(),
                    upper_limit: Decimal::from_str("0.491666666666666666").unwrap(),
                    value: Decimal::from_str("0.491666666666666667").unwrap(),
                }
            );
        }

        #[test]
        fn test_with_skipped_windows() {
            let mut deps = mock_dependencies();
            let limiter = CompressedSMALimiter::new(
                String::from("denomb"),
                "config",
                "divisions",
                "boundary_offset",
                "latest_value",
            );
            let config = WindowConfig {
                window_size: Uint64::from(3_600_000_000_000u64), // 1 hrs
                division_count: Uint64::from(4u64),              // 15 mins each
            };
            limiter
                .set_window_config(&mut deps.storage, &config)
                .unwrap();

            limiter
                .set_boundary_offset(&mut deps.storage, Decimal::percent(5))
                .unwrap();

            let block_time = Timestamp::from_nanos(1661231280000000000);
            let value = Decimal::percent(40);
            limiter
                .check_limit_and_update(&mut deps.storage, block_time, value)
                .unwrap();

            let block_time = block_time.plus_minutes(20); // -> + 20 mins
            let value = Decimal::percent(45);
            limiter
                .check_limit_and_update(&mut deps.storage, block_time, value)
                .unwrap();

            let block_time = block_time.plus_minutes(30); // -> + 50 mins
            let value = Decimal::percent(46);
            limiter
                .check_limit_and_update(&mut deps.storage, block_time, value)
                .unwrap();

            let block_time = block_time.plus_minutes(70); // -> + 120 mins
            let value = Decimal::from_str("0.510000000000000001").unwrap();

            let err = limiter
                .check_limit_and_update(&mut deps.storage, block_time, value)
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
    }

    fn list_divisions(
        approximated_sma: &CompressedSMALimiter,
        storage: &dyn Storage,
    ) -> Vec<CompressedSMADivision> {
        approximated_sma
            .divisions
            .iter(storage)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn add_compressed_divisions(
        storage: &mut dyn Storage,
        limiter: &CompressedSMALimiter,
        divisions: impl IntoIterator<Item = CompressedSMADivision>,
    ) {
        for division in divisions {
            limiter.divisions.push_back(storage, &division).unwrap();
        }
    }
}