use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Decimal, StdResult, Storage, Timestamp, Uint64};
use cw_storage_plus::{Deque, Item};

use crate::ContractError;

use super::compressed_division::CompressedDivision;

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
    pub divisions: Deque<'a, CompressedDivision>,
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
    ) -> StdResult<()> {
        self.config.save(storage, config)
    }

    pub fn clean_up_expired_div(
        &self,
        storage: &mut dyn Storage,
        block_time: Timestamp,
    ) -> Result<(), ContractError> {
        let config = self.config.load(storage)?;
        let window_start_time = block_time.nanos().saturating_sub(config.window_size.u64());

        while let Some(division) = self.divisions.front(storage)? {
            // if window completely passed the division, remove the division

            if division
                .start_time
                .plus_nanos(config.division_size()?.u64())
                .nanos()
                <= window_start_time
            {
                division
                    .start_time
                    .plus_nanos(config.division_size()?.u64())
                    .nanos();

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

        match latest_division {
            Some(division) => {
                // If the division is over, create a new division
                if division.elasped_time(block_time)? >= config.division_size()? {
                    let new_division = CompressedDivision::start(block_time, value);
                    self.divisions.push_back(storage, &new_division)?;
                }
                // else update the current division
                else {
                    self.divisions.pop_back(storage)?;
                    let updated_division = division
                        .accum(value)
                        .map_err(ContractError::calculation_error)?;

                    self.divisions.push_back(storage, &updated_division)?;
                }
            }
            None => {
                let new_division = CompressedDivision::start(block_time, value);
                self.divisions.push_back(storage, &new_division)?;
            }
        };

        Ok(())
    }

    pub fn average(
        &self,
        storage: &mut dyn Storage,
        block_time: Timestamp,
    ) -> Result<Decimal, ContractError> {
        let mut div_iter = self.divisions.iter(storage)?;
        let mut sum = Decimal::zero();
        let mut total_weight = Decimal::zero();

        let config = self.config.load(storage)?;

        // check if head division is over
        let head_division = div_iter.next().transpose()?;
        if let Some(division) = head_division {
            let elapsed_time = division.elasped_time(block_time)?;
            let division_size = config.division_size()?;

            dbg!(elapsed_time);
            dbg!(config.window_size);

            if elapsed_time > config.window_size {
                // weight = (1 - ((elapsed_time - window_size) / division_size)) * n
                let weight = (Decimal::one()
                    .checked_sub(Decimal::from_ratio(
                        elapsed_time
                            .checked_sub(config.window_size)
                            .map_err(ContractError::calculation_error)?,
                        division_size,
                    ))
                    .map_err(ContractError::calculation_error)?)
                .checked_mul(
                    Decimal::from_atomics(dbg!(division.n), 0)
                        .map_err(ContractError::calculation_error)?,
                )
                .map_err(ContractError::calculation_error)?;

                // sum = sum + (division.average() * weight)
                sum = dbg!(sum
                    .checked_add(
                        division
                            .average()
                            .map_err(ContractError::calculation_error)?
                            .checked_mul(weight)
                            .map_err(ContractError::calculation_error)?,
                    )
                    .map_err(ContractError::calculation_error)?);

                dbg!(weight);
                // total_weight = total_weight + weight
                total_weight = dbg!(total_weight
                    .checked_add(weight)
                    .map_err(ContractError::calculation_error)?);
            } else {
                // sum = sum + division.cumsum
                sum = sum
                    .checked_add(division.cumsum)
                    .map_err(ContractError::calculation_error)?;

                // total_weight = total_weight + division.n
                let n = Decimal::from_atomics(division.n, 0)
                    .map_err(ContractError::calculation_error)?;
                total_weight = total_weight
                    .checked_add(n)
                    .map_err(ContractError::calculation_error)?;
            }
        }
        // if there is no head then that means there is no division
        else {
            let latest_value = self.latest_value.load(storage)?;
            return Ok(latest_value);
        }

        for division in div_iter {
            let division = division?;

            // sum = sum + division.average()
            sum = sum
                .checked_add(division.cumsum)
                .map_err(ContractError::calculation_error)?;

            // total_weight = total_weight + division.n
            let n =
                Decimal::from_atomics(division.n, 0).map_err(ContractError::calculation_error)?;
            total_weight = total_weight
                .checked_add(n)
                .map_err(ContractError::calculation_error)?;
        }

        // average = sum / total_weight
        sum.checked_div(total_weight)
            .map_err(ContractError::calculation_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmwasm_std::testing::mock_dependencies;

    #[test]
    fn test_approximated_sma() {
        let mut deps = mock_dependencies();

        // Create a new ApproximatedSMA
        let config_namespace = "config";
        let divisions_namespace = "divisions";
        let latest_value_namespace = "latest_value";
        let approximated_sma = ApproximatedSMA::new(
            config_namespace,
            divisions_namespace,
            latest_value_namespace,
        );

        // Set the config
        let config = ApproximatedSMAConfig {
            window_size: 1_800_000_000_000u64.into(), // 30 mins
            division_count: 3u64.into(),              // 10 minutes each division
        };

        approximated_sma
            .set_config(&mut deps.storage, &config)
            .unwrap();

        // first 10 minutes

        // first data arrived
        let first_block_time = Timestamp::from_nanos(100_000_000);
        let block_time = first_block_time;
        approximated_sma
            .clean_up_expired_div(&mut deps.storage, block_time)
            .unwrap();
        approximated_sma
            .update(&mut deps.storage, block_time, Decimal::percent(30))
            .unwrap();

        assert_eq!(
            approximated_sma
                .average(&mut deps.storage, block_time)
                .unwrap(),
            Decimal::percent(30)
        );

        assert_eq!(
            list_divs(&approximated_sma, &deps.storage),
            vec![CompressedDivision {
                start_time: first_block_time,
                cumsum: Decimal::percent(30),
                n: 1u64.into()
            }]
        );

        // 1 minute later
        let block_time = block_time.plus_minutes(1);
        approximated_sma
            .clean_up_expired_div(&mut deps.storage, block_time)
            .unwrap();
        approximated_sma
            .update(&mut deps.storage, block_time, Decimal::percent(40))
            .unwrap();

        assert_eq!(
            approximated_sma
                .average(&mut deps.storage, block_time)
                .unwrap(),
            Decimal::percent(35)
        );

        assert_eq!(
            list_divs(&approximated_sma, &deps.storage),
            vec![CompressedDivision {
                start_time: first_block_time,
                cumsum: Decimal::percent(30 + 40),
                n: 2u64.into()
            }]
        );

        // 1:30 minutes later
        let block_time = block_time.plus_minutes(1).plus_seconds(30);
        approximated_sma
            .clean_up_expired_div(&mut deps.storage, block_time)
            .unwrap();
        approximated_sma
            .update(&mut deps.storage, block_time, Decimal::percent(50))
            .unwrap();

        assert_eq!(
            approximated_sma
                .average(&mut deps.storage, block_time)
                .unwrap(),
            Decimal::percent(40)
        );

        assert_eq!(
            list_divs(&approximated_sma, &deps.storage),
            vec![CompressedDivision {
                start_time: first_block_time,
                cumsum: Decimal::percent(30 + 40 + 50),
                n: 3u64.into()
            }]
        );

        // 10 mins - 1ns since first data arrived
        let block_time = first_block_time.plus_minutes(10).minus_nanos(1);
        approximated_sma
            .clean_up_expired_div(&mut deps.storage, block_time)
            .unwrap();
        approximated_sma
            .update(&mut deps.storage, block_time, Decimal::percent(60))
            .unwrap();

        assert_eq!(
            approximated_sma
                .average(&mut deps.storage, block_time)
                .unwrap(),
            Decimal::percent(45)
        );

        assert_eq!(
            list_divs(&approximated_sma, &deps.storage),
            vec![CompressedDivision {
                start_time: first_block_time,
                cumsum: Decimal::percent(30 + 40 + 50 + 60),
                n: 4u64.into()
            }]
        );

        // 10 mins since first data arrived
        let block_time = first_block_time.plus_minutes(10);
        approximated_sma
            .clean_up_expired_div(&mut deps.storage, block_time)
            .unwrap();
        approximated_sma
            .update(&mut deps.storage, block_time, Decimal::percent(70))
            .unwrap();

        assert_eq!(
            approximated_sma
                .average(&mut deps.storage, block_time)
                .unwrap(),
            Decimal::percent(50)
        );

        assert_eq!(
            list_divs(&approximated_sma, &deps.storage),
            vec![
                CompressedDivision {
                    start_time: first_block_time,
                    cumsum: Decimal::percent(30 + 40 + 50 + 60),
                    n: 4u64.into()
                },
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(10),
                    cumsum: Decimal::percent(70),
                    n: 1u64.into()
                }
            ]
        );

        // 10 mins + 1ns since first data arrived
        let block_time = first_block_time.plus_minutes(10).plus_nanos(1);
        approximated_sma
            .clean_up_expired_div(&mut deps.storage, block_time)
            .unwrap();
        approximated_sma
            .update(&mut deps.storage, block_time, Decimal::percent(80))
            .unwrap();

        assert_eq!(
            approximated_sma
                .average(&mut deps.storage, block_time)
                .unwrap(),
            Decimal::percent(55)
        );

        assert_eq!(
            list_divs(&approximated_sma, &deps.storage),
            vec![
                CompressedDivision {
                    start_time: first_block_time,
                    cumsum: Decimal::percent(30 + 40 + 50 + 60),
                    n: 4u64.into()
                },
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(10),
                    cumsum: Decimal::percent(70 + 80),
                    n: 2u64.into()
                }
            ]
        );

        // 20 mins since first data arrived
        let block_time = first_block_time.plus_minutes(20);
        approximated_sma
            .clean_up_expired_div(&mut deps.storage, block_time)
            .unwrap();
        approximated_sma
            .update(&mut deps.storage, block_time, Decimal::percent(90))
            .unwrap();

        assert_eq!(
            approximated_sma
                .average(&mut deps.storage, block_time)
                .unwrap(),
            Decimal::percent(60)
        );

        assert_eq!(
            list_divs(&approximated_sma, &deps.storage),
            vec![
                CompressedDivision {
                    start_time: first_block_time,
                    cumsum: Decimal::percent(30 + 40 + 50 + 60),
                    n: 4u64.into()
                },
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(10),
                    cumsum: Decimal::percent(70 + 80),
                    n: 2u64.into()
                },
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(20),
                    cumsum: Decimal::percent(90),
                    n: 1u64.into()
                }
            ]
        );

        // 30 mins - 1ns since first data arrived
        let block_time = first_block_time.plus_minutes(30).minus_nanos(1);
        approximated_sma
            .clean_up_expired_div(&mut deps.storage, block_time)
            .unwrap();
        approximated_sma
            .update(&mut deps.storage, block_time, Decimal::percent(100))
            .unwrap();

        assert_eq!(
            approximated_sma
                .average(&mut deps.storage, block_time)
                .unwrap(),
            Decimal::percent(65)
        );

        assert_eq!(
            list_divs(&approximated_sma, &deps.storage),
            vec![
                CompressedDivision {
                    start_time: first_block_time,
                    cumsum: Decimal::percent(30 + 40 + 50 + 60),
                    n: 4u64.into()
                },
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(10),
                    cumsum: Decimal::percent(70 + 80),
                    n: 2u64.into()
                },
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(20),
                    cumsum: Decimal::percent(90 + 100),
                    n: 2u64.into()
                }
            ]
        );

        // 30 mins since first data arrived
        let block_time = first_block_time.plus_minutes(30);
        approximated_sma
            .clean_up_expired_div(&mut deps.storage, block_time)
            .unwrap();
        approximated_sma
            .update(&mut deps.storage, block_time, Decimal::percent(20))
            .unwrap();

        assert_eq!(
            approximated_sma
                .average(&mut deps.storage, block_time)
                .unwrap(),
            Decimal::percent(60)
        );

        assert_eq!(
            list_divs(&approximated_sma, &deps.storage),
            vec![
                CompressedDivision {
                    start_time: first_block_time,
                    cumsum: Decimal::percent(30 + 40 + 50 + 60),
                    n: 4u64.into()
                },
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(10),
                    cumsum: Decimal::percent(70 + 80),
                    n: 2u64.into()
                },
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(20),
                    cumsum: Decimal::percent(90 + 100),
                    n: 2u64.into()
                },
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(30),
                    cumsum: Decimal::percent(20),
                    n: 1u64.into()
                }
            ]
        );

        // 30 mins + 1ns since first data arrived
        let block_time = first_block_time.plus_minutes(30).plus_nanos(1);
        approximated_sma
            .clean_up_expired_div(&mut deps.storage, block_time)
            .unwrap();
        approximated_sma
            .update(&mut deps.storage, block_time, Decimal::percent(35))
            .unwrap();

        assert_eq!(
            approximated_sma
                .average(&mut deps.storage, block_time)
                .unwrap(),
            Decimal::raw(575000000000083333u128)
        );

        assert_eq!(
            list_divs(&approximated_sma, &deps.storage),
            vec![
                CompressedDivision {
                    start_time: first_block_time,
                    cumsum: Decimal::percent(30 + 40 + 50 + 60),
                    n: 4u64.into()
                },
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(10),
                    cumsum: Decimal::percent(70 + 80),
                    n: 2u64.into()
                },
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(20),
                    cumsum: Decimal::percent(90 + 100),
                    n: 2u64.into()
                },
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(30),
                    cumsum: Decimal::percent(20 + 35),
                    n: 2u64.into()
                }
            ]
        );

        // 40 mins - 1ns since first data arrived
        let block_time = first_block_time.plus_minutes(40).minus_nanos(1);

        approximated_sma
            .clean_up_expired_div(&mut deps.storage, block_time)
            .unwrap();

        approximated_sma
            .update(&mut deps.storage, block_time, Decimal::percent(30))
            .unwrap();

        assert_eq!(
            approximated_sma
                .average(&mut deps.storage, block_time)
                .unwrap(),
            Decimal::raw(607142857142707482u128)
        );

        assert_eq!(
            list_divs(&approximated_sma, &deps.storage),
            vec![
                CompressedDivision {
                    start_time: first_block_time,
                    cumsum: Decimal::percent(30 + 40 + 50 + 60),
                    n: 4u64.into()
                },
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(10),
                    cumsum: Decimal::percent(70 + 80),
                    n: 2u64.into()
                },
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(20),
                    cumsum: Decimal::percent(90 + 100),
                    n: 2u64.into()
                },
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(30),
                    cumsum: Decimal::percent(20 + 35 + 30),
                    n: 3u64.into()
                }
            ]
        );

        // 40 mins since first data arrived
        let block_time = first_block_time.plus_minutes(40);

        approximated_sma
            .clean_up_expired_div(&mut deps.storage, block_time)
            .unwrap();
        approximated_sma
            .update(&mut deps.storage, block_time, Decimal::percent(25))
            .unwrap();

        assert_eq!(
            approximated_sma
                .average(&mut deps.storage, block_time)
                .unwrap(),
            Decimal::raw(562500000000000000u128)
        );

        assert_eq!(
            list_divs(&approximated_sma, &deps.storage),
            vec![
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(10),
                    cumsum: Decimal::percent(70 + 80),
                    n: 2u64.into()
                },
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(20),
                    cumsum: Decimal::percent(90 + 100),
                    n: 2u64.into()
                },
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(30),
                    cumsum: Decimal::percent(20 + 35 + 30),
                    n: 3u64.into()
                },
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(40),
                    cumsum: Decimal::percent(25),
                    n: 1u64.into()
                }
            ]
        );

        // 40 mins + 1ns since first data arrived
        let block_time = first_block_time.plus_minutes(40).plus_nanos(1);
        approximated_sma
            .clean_up_expired_div(&mut deps.storage, block_time)
            .unwrap();
        approximated_sma
            .update(&mut deps.storage, block_time, Decimal::percent(20))
            .unwrap();

        assert_eq!(
            approximated_sma
                .average(&mut deps.storage, block_time)
                .unwrap(),
            Decimal::raw(522222222222137860u128)
        );

        assert_eq!(
            list_divs(&approximated_sma, &deps.storage),
            vec![
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(10),
                    cumsum: Decimal::percent(70 + 80),
                    n: 2u64.into()
                },
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(20),
                    cumsum: Decimal::percent(90 + 100),
                    n: 2u64.into()
                },
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(30),
                    cumsum: Decimal::percent(20 + 35 + 30),
                    n: 3u64.into()
                },
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(40),
                    cumsum: Decimal::percent(25 + 20),
                    n: 2u64.into()
                }
            ]
        );

        // 60 mins since first data arrived
        let block_time = first_block_time.plus_minutes(60);
        approximated_sma
            .clean_up_expired_div(&mut deps.storage, block_time)
            .unwrap();
        approximated_sma
            .update(&mut deps.storage, block_time, Decimal::percent(10))
            .unwrap();

        assert_eq!(
            approximated_sma
                .average(&mut deps.storage, block_time)
                .unwrap(),
            Decimal::raw(233333333333333333u128)
        );

        assert_eq!(
            list_divs(&approximated_sma, &deps.storage),
            vec![
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(30),
                    cumsum: Decimal::percent(20 + 35 + 30),
                    n: 3u64.into()
                },
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(40),
                    cumsum: Decimal::percent(25 + 20),
                    n: 2u64.into()
                },
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(60),
                    cumsum: Decimal::percent(10),
                    n: 1u64.into()
                }
            ]
        );

        // 60 mins + 1ns since first data arrived
        let block_time = first_block_time.plus_minutes(60).plus_nanos(1);
        approximated_sma
            .clean_up_expired_div(&mut deps.storage, block_time)
            .unwrap();
        approximated_sma
            .update(&mut deps.storage, block_time, Decimal::percent(35))
            .unwrap();

        assert_eq!(
            approximated_sma
                .average(&mut deps.storage, block_time)
                .unwrap(),
            Decimal::raw(249999999999976190u128)
        );

        assert_eq!(
            list_divs(&approximated_sma, &deps.storage),
            vec![
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(30),
                    cumsum: Decimal::percent(20 + 35 + 30),
                    n: 3u64.into()
                },
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(40),
                    cumsum: Decimal::percent(25 + 20),
                    n: 2u64.into()
                },
                CompressedDivision {
                    start_time: first_block_time.plus_minutes(60),
                    cumsum: Decimal::percent(10 + 35),
                    n: 2u64.into()
                }
            ]
        );

        // 60 mins + 10 mins (1 div) + 30 mins (1 window) since first data arrived
        // this should wipe all the previous blocks
        let block_time = first_block_time.plus_minutes(100);
        approximated_sma
            .clean_up_expired_div(&mut deps.storage, block_time)
            .unwrap();

        assert_eq!(
            approximated_sma
                .average(&mut deps.storage, block_time)
                .unwrap(),
            Decimal::percent(35)
        );

        assert_eq!(list_divs(&approximated_sma, &deps.storage), vec![]);

        // TODO: if update new value then that latest value becomes representative of the missing divs

        // approximated_sma
        //     .update(&mut deps.storage, block_time, Decimal::percent(35))
        //     .unwrap();

        // // 10 mins and 10 seconds since first data arrived
        // let block_time = first_block_time.plus_minutes(10).plus_seconds(10);
        // approximated_sma
        //     .update(&mut deps.storage, block_time, Decimal::percent(20))
        //     .unwrap();

        // assert_eq!(
        //     approximated_sma
        //         .average(&mut deps.storage, block_time)
        //         .unwrap(),
        //     Decimal::raw(348876404494382022u128)
        // );

        // // 10 mins and 20 seconds since first data arrived
        // let block_time = first_block_time.plus_minutes(10).plus_seconds(20);
        // approximated_sma
        //     .update(&mut deps.storage, block_time, Decimal::percent(35))
        //     .unwrap();

        // assert_eq!(
        //     approximated_sma
        //         .average(&mut deps.storage, block_time)
        //         .unwrap(),
        //     Decimal::raw(348058252427184465u128)
        // );

        // // 20 mins since first data arrived
        // let block_time = first_block_time.plus_minutes(20);
        // approximated_sma
        //     .update(&mut deps.storage, block_time, Decimal::percent(10))
        //     .unwrap();

        // assert_eq!(
        //     approximated_sma
        //         .average(&mut deps.storage, block_time)
        //         .unwrap(),
        //     Decimal::raw(348058252427184465u128)
        // );

        // After first 1hrs, the `average` function will take weighted average by
        // separating the oldest division and the rest of divisions.
        // For the oldest division, the weight is calculated by:
        // window_start_time = block_time - window_size
        // (1 - ((oldest_division_start_time - window_start_time) / division_size)) * oldest_division_n

        // For the rest, the weight is basically n of each division.

        // |-----div1----|-----div2-----|-----div3-----|------div4-----|
        // |   |█████████|              |              |   |
        // ------------------------------------------------>
        //     |                  window                   |
        // -------------------------------------------------

        // |-----[x]-----|-----div2-----|-----div3-----|------div4-----|
        // |             |              |              |               |
        // ------------------------------------------------------------->
        //               |                   window                    |
        // --------------------------------------------------------------

        // weighted avg
        // n_1' = n_1 * (1 - (new_div_elasped_time / window_size))
        // ((avg_1 * n_1') + sum(div_2) / (n_1' + n_2)

        // ----

        // let sum_1 = Decimal::percent(20 + 35 + 30);
        // let n_1 = Decimal::from_atomics(3u64, 0).unwrap();

        // let sum_2 = Decimal::percent(25 + 20 + 10 + 35);
        // let n_2 = Decimal::from_atomics(4u64, 0).unwrap();

        // dbg!(sum_2 / n_2);

        // let div_size = 1_800_000_000_000u64 / 3u64;
        // let n_1_prime = n_1 * (Decimal::one() - dbg!((Decimal::from_ratio(1u128, div_size))));

        // dbg!(n_1_prime);

        // dbg!(sum_1 / n_1);
        // dbg!((sum_1 / n_1) * n_1_prime);
        // dbg!(((sum_1 / n_1) * n_1_prime) + (sum_2));
        // dbg!(n_1_prime + n_2);

        // dbg!(
        //     "=== res ===",
        //     (((sum_1 / n_1) * n_1_prime) + (sum_2)) / (n_1_prime + n_2)
        // );
    }

    fn list_divs(
        approximated_sma: &ApproximatedSMA,
        storage: &dyn Storage,
    ) -> Vec<CompressedDivision> {
        approximated_sma
            .divisions
            .iter(storage)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }
}
