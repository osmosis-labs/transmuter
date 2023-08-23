use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Decimal, Storage, Timestamp, Uint64};
use cw_storage_plus::{Deque, Item};

use crate::ContractError;

use super::compressed_sma_division::CompressedSMADivision;

#[cw_serde]
pub struct CompressedSMALimiterConfig {
    pub window_size: Uint64,
    pub division_count: Uint64,
}

impl CompressedSMALimiterConfig {
    fn division_size(&self) -> Result<Uint64, ContractError> {
        self.window_size
            .checked_div(self.division_count)
            .map_err(Into::into)
    }
}
pub struct CompressedSMALimiter<'a> {
    pub config: Item<'a, CompressedSMALimiterConfig>,
    pub divisions: Deque<'a, CompressedSMADivision>,
    pub latest_value: Item<'a, Decimal>,
}

impl<'a> CompressedSMALimiter<'a> {
    const fn new(
        config_namespace: &'a str,
        divisions_namespace: &'a str,
        latest_value_namespace: &'a str,
    ) -> Self {
        Self {
            config: Item::new(config_namespace),
            divisions: Deque::new(divisions_namespace),
            latest_value: Item::new(latest_value_namespace),
        }
    }

    pub fn set_config(
        &self,
        storage: &mut dyn Storage,
        config: &CompressedSMALimiterConfig,
    ) -> Result<(), ContractError> {
        if config.window_size.checked_rem(config.division_count)? != Uint64::zero() {
            return Err(ContractError::UnevenWindowDivision {});
        }

        self.config.save(storage, config).map_err(Into::into)
    }

    fn remove_outdated_division(
        &self,
        storage: &mut dyn Storage,
        block_time: Timestamp,
    ) -> Result<(), ContractError> {
        let config = self.config.load(storage)?;

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

    pub fn update(
        &self,
        storage: &mut dyn Storage,
        block_time: Timestamp,
        value: Decimal,
    ) -> Result<(), ContractError> {
        let config = self.config.load(storage)?;
        let latest_division = self.divisions.back(storage)?;

        // update latest value
        self.latest_value.save(storage, &value)?;

        // match latest_division {
        //     Some(division) => {
        //         // If the division is over, create a new division
        //         if division.elasped_time(block_time)? >= config.division_size()? {
        //             let new_division = CompressedDivision::start(block_time, value);
        //             self.divisions.push_back(storage, &new_division)?;
        //         }
        //         // else update the current division
        //         else {
        //             self.divisions.pop_back(storage)?;
        //             let updated_division = division
        //                 .accum(value)
        //                 .map_err(ContractError::calculation_error)?;

        //             self.divisions.push_back(storage, &updated_division)?;
        //         }
        //     }
        //     None => {
        //         let new_division = CompressedDivision::start(block_time, value);
        //         self.divisions.push_back(storage, &new_division)?;
        //     }
        // };

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

            let limiter = CompressedSMALimiter::new("config", "divisions", "latest_value");

            let err = limiter
                .set_config(
                    &mut deps.storage,
                    &CompressedSMALimiterConfig {
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

            let limiter = CompressedSMALimiter::new("config", "divisions", "latest_value");

            let err = limiter
                .set_config(
                    &mut deps.storage,
                    &CompressedSMALimiterConfig {
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

            let approximated_sma = CompressedSMALimiter::new("config", "divisions", "latest_value");

            approximated_sma
                .set_config(
                    &mut deps.storage,
                    &CompressedSMALimiterConfig {
                        window_size: Uint64::from(604_800_000_000u64),
                        division_count: Uint64::from(12u64),
                    },
                )
                .unwrap();

            let config = approximated_sma.config.load(&deps.storage).unwrap();

            assert_eq!(
                config,
                CompressedSMALimiterConfig {
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

            let limiter = CompressedSMALimiter::new("config", "divisions", "latest_value");
            limiter
                .set_config(
                    &mut deps.storage,
                    &CompressedSMALimiterConfig {
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

            let limiter = CompressedSMALimiter::new("config", "divisions", "latest_value");
            let config = CompressedSMALimiterConfig {
                window_size: Uint64::from(3_600_000_000_000u64),
                division_count: Uint64::from(2u64),
            };
            limiter.set_config(&mut deps.storage, &config).unwrap();

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
            let limiter = CompressedSMALimiter::new("config", "divisions", "latest_value");
            let config = CompressedSMALimiterConfig {
                window_size: Uint64::from(3_600_000_000_000u64),
                division_count: Uint64::from(2u64),
            };
            limiter.set_config(&mut deps.storage, &config).unwrap();

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
            let limiter = CompressedSMALimiter::new("config", "divisions", "latest_value");
            let config = CompressedSMALimiterConfig {
                window_size: Uint64::from(3_600_000_000_000u64),
                division_count: Uint64::from(2u64),
            };
            limiter.set_config(&mut deps.storage, &config).unwrap();

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
            let limiter = CompressedSMALimiter::new("config", "divisions", "latest_value");
            let config = CompressedSMALimiterConfig {
                window_size: Uint64::from(3_600_000_000_000u64),
                division_count: Uint64::from(2u64),
            };

            limiter.set_config(&mut deps.storage, &config).unwrap();

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
            let limiter = CompressedSMALimiter::new("config", "divisions", "latest_value");
            let config = CompressedSMALimiterConfig {
                window_size: Uint64::from(3_600_000_000_000u64),
                division_count: Uint64::from(2u64),
            };

            limiter.set_config(&mut deps.storage, &config).unwrap();

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
            let limiter = CompressedSMALimiter::new("config", "divisions", "latest_value");
            let config = CompressedSMALimiterConfig {
                window_size: Uint64::from(3_600_000_000_000u64),
                division_count: Uint64::from(2u64),
            };

            limiter.set_config(&mut deps.storage, &config).unwrap();

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
