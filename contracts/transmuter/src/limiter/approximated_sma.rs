use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Decimal, StdResult, Storage, Timestamp, Uint64};
use cw_storage_plus::{Deque, Item};

use crate::ContractError;

use super::compressed_sma_division::CompressedSMADivision;

#[cw_serde]
pub struct ApproximatedSMAConfig {
    pub window_size: Uint64,
    pub division_count: Uint64,
}

impl ApproximatedSMAConfig {
    fn division_size(&self) -> Result<Uint64, ContractError> {
        self.window_size
            .checked_div(self.division_count)
            .map_err(ContractError::calculation_error)
    }
}
pub struct ApproximatedSMA<'a> {
    pub config: Item<'a, ApproximatedSMAConfig>,
    pub divisions: Deque<'a, CompressedSMADivision>,
    pub latest_value: Item<'a, Decimal>,
}

impl<'a> ApproximatedSMA<'a> {
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
        config: &ApproximatedSMAConfig,
    ) -> Result<(), ContractError> {
        if config.window_size.checked_rem(config.division_count)? != Uint64::zero() {
            return Err(ContractError::UnevenWindowDivision {});
        }

        self.config.save(storage, config).map_err(Into::into)
    }

    pub fn clean_up_expired_div(
        &self,
        storage: &mut dyn Storage,
        block_time: Timestamp,
    ) -> Result<(), ContractError> {
        let config = self.config.load(storage)?;
        let window_start_time = block_time.nanos().saturating_sub(config.window_size.u64());

        // while let Some(division) = self.divisions.front(storage)? {
        //     // if window completely passed the division, remove the division

        //     if division
        //         .start_time
        //         .plus_nanos(config.division_size()?.u64())
        //         .nanos()
        //         <= window_start_time
        //     {
        //         division
        //             .start_time
        //             .plus_nanos(config.division_size()?.u64())
        //             .nanos();

        //         self.divisions.pop_front(storage)?;
        //     } else {
        //         break;
        //     }
        // }

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

            let approximated_sma = ApproximatedSMA::new("config", "divisions", "latest_value");

            let err = approximated_sma
                .set_config(
                    &mut deps.storage,
                    &ApproximatedSMAConfig {
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

            let approximated_sma = ApproximatedSMA::new("config", "divisions", "latest_value");

            let err = approximated_sma
                .set_config(
                    &mut deps.storage,
                    &ApproximatedSMAConfig {
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

            let approximated_sma = ApproximatedSMA::new("config", "divisions", "latest_value");

            approximated_sma
                .set_config(
                    &mut deps.storage,
                    &ApproximatedSMAConfig {
                        window_size: Uint64::from(604_800_000_000u64),
                        division_count: Uint64::from(12u64),
                    },
                )
                .unwrap();

            let config = approximated_sma.config.load(&deps.storage).unwrap();

            assert_eq!(
                config,
                ApproximatedSMAConfig {
                    window_size: Uint64::from(604_800_000_000u64),
                    division_count: Uint64::from(12u64),
                }
            );
        }
    }

    fn list_divs(
        approximated_sma: &ApproximatedSMA,
        storage: &dyn Storage,
    ) -> Vec<CompressedSMADivision> {
        approximated_sma
            .divisions
            .iter(storage)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }
}
