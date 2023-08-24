use cosmwasm_schema::cw_serde;
use cosmwasm_std::{ensure, Decimal, Storage, Timestamp, Uint64};
use cw_storage_plus::{Deque, Item};

use crate::ContractError;

use super::compressed_sma_division::CompressedSMADivision;

#[cw_serde]
pub struct WindowConfig {
    pub window_size: Uint64,
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
    pub denom: String,
    pub window_config: Item<'a, WindowConfig>,
    pub divisions: Deque<'a, CompressedSMADivision>,
    pub latest_value: Item<'a, Decimal>,
    pub boundary_offset: Item<'a, Decimal>,
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
        // TODO:
        // - clean up all divisions if the config is changed
        // - enforce max divisions
        if config.window_size.checked_rem(config.division_count)? != Uint64::zero() {
            return Err(ContractError::UnevenWindowDivision {});
        }

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

    fn remove_outdated_division(
        &self,
        storage: &mut dyn Storage,
        block_time: Timestamp,
    ) -> Result<(), ContractError> {
        let config = self.window_config.load(storage)?;

        while let Some(division) = self.divisions.front(storage)? {
            // if window completely passed the division, remove the division
            if division.is_outdated(block_time, config.window_size, config.division_size()?)? {
                self.divisions.pop_front(storage)?;
            } else {
                break;
            }
        }

        Ok(())
    }

    /// Check if the value is within the limit and update the divisions
    pub fn check_and_update(
        &self,
        storage: &mut dyn Storage,
        block_time: Timestamp,
        value: Decimal,
    ) -> Result<(), ContractError> {
        let config = self.window_config.load(storage)?;
        let latest_division = self.divisions.back(storage)?;

        // skip the checks if there is no division
        if latest_division.is_some() {
            let avg = CompressedSMADivision::compressed_moving_average(
                self.divisions
                    .iter(storage)?
                    .collect::<Result<Vec<_>, _>>()?,
                config.division_size()?,
                config.window_size,
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
        }

        // update latest value
        let prev_value = self.latest_value.may_load(storage)?.unwrap_or_default();
        self.latest_value.save(storage, &value)?;

        match latest_division {
            Some(division) => {
                // If the division is over, create a new division
                if division.elapsed_time(block_time)? >= config.division_size()? {
                    let new_division =
                        CompressedSMADivision::new(block_time, block_time, value, prev_value)?;
                    self.divisions.push_back(storage, &new_division)?;
                }
                // else update the current division
                else {
                    self.divisions.pop_back(storage)?;
                    let updated_division = division
                        .update(block_time, value)
                        .map_err(ContractError::calculation_error)?;

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
                        window_size: Uint64::from(604_800_000_000u64),
                        division_count: Uint64::from(13u64),
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
                        division_count: Uint64::from(12u64),
                    },
                )
                .unwrap();

            let config = limiter.window_config.load(&deps.storage).unwrap();

            assert_eq!(
                config,
                WindowConfig {
                    window_size: Uint64::from(604_800_000_000u64),
                    division_count: Uint64::from(12u64),
                }
            );
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
                .remove_outdated_division(&mut deps.storage, block_time)
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
                .remove_outdated_division(&mut deps.storage, block_time)
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
                .remove_outdated_division(&mut deps.storage, block_time)
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
                .remove_outdated_division(&mut deps.storage, block_time)
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
                .remove_outdated_division(&mut deps.storage, block_time)
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
            add_compressed_divisions(&mut deps.storage, &limiter, divisions.clone());
            limiter
                .remove_outdated_division(&mut deps.storage, block_time)
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
                .check_and_update(&mut deps.storage, block_time, value)
                .unwrap();

            // now, average should be the same as the value regardless of how time pass
            // 50% + 5% = 55% is the boundary
            let block_time = block_time.plus_minutes(10);
            let value = Decimal::percent(55);
            limiter
                .check_and_update(&mut deps.storage, block_time, value)
                .unwrap();

            // now, average = (50% x 600000000000 + 55% x 300000000000) / 900000000000 = 0.53
            let block_time = block_time.plus_minutes(15);
            let value = Decimal::from_str("0.580000000000000001").unwrap(); // 53% + 5% = 58%
            let err = limiter
                .check_and_update(&mut deps.storage, block_time, value)
                .unwrap_err();

            assert_eq!(
                err,
                ContractError::ChangeUpperLimitExceeded {
                    denom: "denoma".to_string(),
                    upper_limit: Decimal::percent(58),
                    value: Decimal::from_str("0.580000000000000001").unwrap(),
                }
            );

            // test case that time passed is more than current division size
        }

        #[test]
        fn test_with_clean_up_outdated() {}

        #[test]
        fn test_with_skipped_windows() {}
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
