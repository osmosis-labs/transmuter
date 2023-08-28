use super::helpers::*;
use crate::ContractError;
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{ensure, Decimal, StdError, Timestamp, Uint64};

/// CompressedDivision is a compressed representation of a data points in sliding window.
/// It is used to reduce the gas cost of storing, retriving & cleaning up data points in sliding window.
///
/// The structure of the compression is as follows:
///
/// ======= CompressedSMADivision =======
/// |                                   |
/// |                . `latest_value`   |
/// ^ `started_at`   ^ `updated_at`     ^ `ended_at`[1]
/// |████████████████                   |
/// |     ^ integral = sum(value * elapsed_time) til latest update
/// =====================================
/// -------------------------------------> time
///
/// It is a lossy compression of the data points in the window, it keeps the integral
/// of the data points until latest update of the division, which is enough to calculate
/// the average of the data points in the window. But the average will become approximate once
/// the window edge is within the integral region (before latest update of the division).
///
/// [1] `ended_at` is not defined in the struct because it can be calculated by `started_at` + `division_size`
/// where `division_size` is defined at the `CompressedSMALimiter`
#[cw_serde]
pub struct CompressedSMADivision {
    /// Time where the division is mark as started
    started_at: Timestamp,

    /// Time where it is last updated
    updated_at: Timestamp,

    /// The latest value that gets updated
    latest_value: Decimal,

    /// sum of each updated value * elasped time since last update
    integral: Decimal,
}

impl CompressedSMADivision {
    pub fn new(
        started_at: Timestamp,
        updated_at: Timestamp,
        value: Decimal,
        prev_value: Decimal,
    ) -> Result<Self, ContractError> {
        ensure!(
            updated_at >= started_at,
            ContractError::change_limit_error(
                "`updated_at` must be greater than or equal to `started_at`"
            )
        );

        let elapsed_time = elapsed_time(started_at.nanos(), updated_at.nanos())?;

        Ok(Self {
            started_at,
            updated_at,
            latest_value: value,
            integral: prev_value.checked_mul(from_uint(elapsed_time))?,
        })
    }

    pub fn update(&self, updated_at: Timestamp, value: Decimal) -> Result<Self, ContractError> {
        let prev_updated_at = self.updated_at.nanos();
        let elapsed_time = elapsed_time(prev_updated_at, updated_at.nanos())?;
        Ok(Self {
            started_at: self.started_at,
            updated_at,
            latest_value: value,
            integral: self
                .integral
                .checked_add(self.latest_value.checked_mul(from_uint(elapsed_time))?)?,
        })
    }

    pub fn is_outdated(
        &self,
        block_time: Timestamp,
        window_size: Uint64,
        division_size: Uint64,
    ) -> Result<bool, ContractError> {
        let window_started_at = backward(block_time.nanos(), window_size)?;
        let division_ended_at = forward(self.started_at.nanos(), division_size)?;

        Ok(window_started_at >= division_ended_at)
    }

    pub fn elapsed_time(&self, block_time: Timestamp) -> Result<Uint64, ContractError> {
        elapsed_time(self.started_at.nanos(), block_time.nanos())
    }

    pub fn ended_at(&self, division_size: Uint64) -> Result<Uint64, ContractError> {
        forward(self.started_at.nanos(), division_size)
    }

    /// Find the next started_at time based on the division size.
    ///
    /// In the following graphic, each division is `division_size` long.
    ///
    /// |   self   |         |         |          |
    ///                                   ^ block_time
    ///                                ^ next_started_at
    pub fn next_started_at(
        &self,
        division_size: Uint64,
        block_time: Timestamp,
    ) -> Result<Timestamp, ContractError> {
        let started_at = Uint64::from(self.started_at.nanos());
        let block_time = Uint64::from(block_time.nanos());

        let elapsed_time_within_next_div =
            elapsed_time(started_at, block_time)?.checked_rem(division_size)?;

        Ok(Timestamp::from_nanos(
            backward(block_time, elapsed_time_within_next_div)?.u64(),
        ))
    }

    /// This function calculates the arithmatic mean of the divisions in a specified window
    /// The window is defined by the `window_size` and `division_size`
    ///
    /// The calculation is done by accumulating the sum of value * elapsed_time (integral)
    /// then divide by the total elapsed time (integral range)
    ///
    /// As this is `CompressionDivision`, not all the data points in the window is stored for gas optimization
    /// When the window covers portion of the first division, it needs to readjust the integral
    /// based on how far the window start time eats in to the first division proportionally.
    ///
    /// ## Assumptions
    /// - Divisions are sorted by started_at
    /// - Last division's updated_at is less than block_time
    /// - All divisions are within the window or at least overlap with the window
    /// - All divisions are of the same size
    ///
    /// The above assumptions are guaranteed by the `CompressedSMALimiter`
    pub fn compressed_moving_average(
        latest_removed_division: Option<Self>,
        divisions: impl IntoIterator<Item = Self>,
        division_size: Uint64,
        window_size: Uint64,
        block_time: Timestamp,
    ) -> Result<Decimal, ContractError> {
        let mut divisions = divisions.into_iter();
        let window_started_at = backward(block_time.nanos(), window_size)?;

        let first_division = divisions.next();

        // process first division
        // this needs to be handled differently because the first division's integral needs to be recalibrated
        // based on how far window start time eats in to the first division
        let (mut processed_division, mut integral) = match first_division {
            Some(division) => {
                let division_started_at = Uint64::from(division.started_at.nanos());

                // |  div 1   |  div 2  |  div 3  |
                //      ██████ <- remiaining division size
                //     |           window            |
                let remaining_division_size =
                    elapsed_time(window_started_at, division.ended_at(division_size)?)?
                        .min(division_size);

                let latest_value_elapsed_time =
                    division.latest_value_elapsed_time(division_size, block_time)?;

                let window_started_before_last_first_div_update =
                    window_started_at < division.updated_at.nanos().into();

                let integral =
                // |  div 1   |
                //      ^ last first div updated
                //   |          window           |
                //   ^ window started at
                //
                // This case, existing integral needs to be taken into account
                if window_started_before_last_first_div_update {
                    let window_started_after_first_division =
                        window_started_at > division_started_at;

                    let integral =
                    // if the window start after the first division, then the first division's integral needs
                    // readjustment based on how far the window start time eats in to the first division
                    if window_started_after_first_division {
                        division
                            .adjusted_integral(remaining_division_size, latest_value_elapsed_time)?
                    }
                    // if the window start before the first division, then the first division's integral can be used as is
                    else {
                        division.integral
                    };

                    integral
                        .checked_add(division.latest_value_integral(division_size, block_time)?)?
                }
                // else disregard the existing integral, and calculate integral from the remaning division size
                else {
                    division
                        .latest_value
                        .checked_mul(from_uint(remaining_division_size))?
                };

                (division, integral)
            }
            None => {
                // if there is no divisions, check the latest removed one
                // if it's there then take its latest value as average
                return match latest_removed_division {
                    Some(CompressedSMADivision { latest_value, .. }) => Ok(latest_value),
                    None => {
                        Err(StdError::generic_err("there is no data point since last reset").into())
                    }
                };
            }
        };

        let first_div_started_at = processed_division.started_at;

        // integrate the rest of divisions
        for division in divisions {
            // check if there is any gap between divisions
            // if so, take the latest value * gap the missing integral

            // |  div 1   |    x    |    x    |  div 2  |
            //            ████████████████████ <- gap
            //     |           window            |
            let gap = elapsed_time(
                processed_division.ended_at(division_size)?,
                division.started_at.nanos(),
            )?;

            // if there is no gap between divisions, then the gap_integral is 0
            let gap_integral = processed_division
                .latest_value
                .checked_mul(from_uint(gap))?;

            integral = integral
                .checked_add(gap_integral)?
                .checked_add(division.integral)?
                .checked_add(division.latest_value_integral(division_size, block_time)?)?;

            processed_division = division;
        }

        let latest_division = processed_division;

        // if latest division end before block time,
        // add latest value * elasped time after latest division to integral
        let latest_division_ended_at = latest_division.ended_at(division_size)?;

        // |   div 1   |   div 2   |   div 3   |
        //                                     ^ latest division ended at
        //                                         ^ block time
        //                                     ████ <- latest value elapsed time after latest division
        if Uint64::from(block_time.nanos()) > latest_division_ended_at {
            let elasped_time_after_latest_division =
                elapsed_time(latest_division_ended_at, block_time.nanos())?;

            // integrate with the latest value by elasped time after latest division
            integral = integral.checked_add(
                latest_division
                    .latest_value
                    .checked_mul(from_uint(elasped_time_after_latest_division))?,
            )?;
        }

        match latest_removed_division {
            Some(latest_removed_division) => {
                // if a division gets removed, it must be older than window_started_at
                // its latest value should be static throughout window start until first division within window
                // so we take integral of it
                // if window started at first division, then there is no missing integral and this will be 0
                //
                // ==============================================
                //
                //     v period with latest removed window value
                //    ██|   div 1   |   div 2   |   div 3   |
                //   |              window               |
                //
                // ==============================================
                let missing_period = elapsed_time(window_started_at, first_div_started_at.nanos())?;
                let missing_integral = latest_removed_division
                    .latest_value
                    .checked_mul(from_uint(missing_period))?;

                integral
                    .checked_add(missing_integral)?
                    // for this case, we can be sure that total integral range is window size
                    // since it integrates from window stared at
                    .checked_div(from_uint(window_size))
                    .map_err(Into::into)
            }
            None => {
                // if there is no removed division, then the total integral range can be either case
                //
                // ========================================
                //
                //      |   div 1   |   div 2   |   div 3   |
                //   |              window               |
                //      █████████████████████████████████
                //      ^ start at first division
                //                                      ^ end at block time
                // ========================================
                //
                // |   div 1   |   div 2   |   div 3   |
                //   |              window               |
                //   ████████████████████████████████████
                //   ^ start at window start
                //                                      ^ end at block time
                let started_at = window_started_at.max(first_div_started_at.nanos().into());
                let total_elapsed_time = elapsed_time(started_at, block_time.nanos())?;
                integral
                    .checked_div(from_uint(total_elapsed_time))
                    .map_err(Into::into)
            }
        }
    }

    fn adjusted_integral(
        &self,
        remaining_division_size: Uint64,
        latest_value_elapsed_time: Uint64,
    ) -> Result<Decimal, ContractError> {
        let current_integral_range =
            elapsed_time(self.started_at.nanos(), self.updated_at.nanos())?;

        let new_integral_range = remaining_division_size.checked_sub(latest_value_elapsed_time)?;

        let division_average_before_latest_update = self
            .integral
            .checked_div(from_uint(current_integral_range))?;

        division_average_before_latest_update
            .checked_mul(from_uint(new_integral_range))
            .map_err(Into::into)
    }

    /// This function calculates the elapsed time since the latest value is updated
    /// The elapsed time is capped by the division size:
    /// If the end of the division is before the block time,
    /// then the elapsed time is counted until the end of the division
    ///
    /// === end of division is before block time ===
    ///
    /// |  div 1   |
    ///      ^ latest value updated
    ///          ^ block time
    ///      ████ <- latest value elapsed time
    ///
    /// === end of division is after block time ===
    ///
    /// |  div 1   |
    ///      ^ latest value updated
    ///               ^ block time
    ///      ██████ <- latest value elapsed time
    fn latest_value_elapsed_time(
        &self,
        division_size: Uint64,
        block_time: Timestamp,
    ) -> Result<Uint64, ContractError> {
        let ended_at = Uint64::from(self.started_at.nanos()).checked_add(division_size)?;
        let block_time = Uint64::from(block_time.nanos());

        let latest_value_persist_until = block_time.min(ended_at);
        elapsed_time(self.updated_at.nanos(), latest_value_persist_until).map_err(Into::into)
    }

    fn latest_value_integral(
        &self,
        division_size: Uint64,
        block_time: Timestamp,
    ) -> Result<Decimal, ContractError> {
        let elapsed_time = self.latest_value_elapsed_time(division_size, block_time)?;
        self.latest_value
            .checked_mul(from_uint(elapsed_time))
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use cosmwasm_std::StdError;

    use super::*;

    #[test]
    fn test_new_compressed_division() {
        // started_at < updated_at
        let started_at = Timestamp::from_nanos(90);
        let updated_at = Timestamp::from_nanos(100);
        let value = Decimal::percent(10);
        let prev_value = Decimal::percent(10);
        let compressed_division =
            CompressedSMADivision::new(started_at, updated_at, value, prev_value).unwrap();

        assert_eq!(
            compressed_division,
            CompressedSMADivision {
                started_at,
                updated_at,
                latest_value: value,
                integral: Decimal::percent(10) * Decimal::from_ratio(10u128, 1u128)
            }
        );

        // started_at == updated_at
        let started_at = Timestamp::from_nanos(90);
        let updated_at = Timestamp::from_nanos(90);

        let compressed_division =
            CompressedSMADivision::new(started_at, updated_at, value, prev_value).unwrap();

        assert_eq!(
            compressed_division,
            CompressedSMADivision {
                started_at,
                updated_at,
                latest_value: value,
                integral: Decimal::zero()
            }
        );

        // started_at > updated_at
        let started_at = Timestamp::from_nanos(90);
        let updated_at = Timestamp::from_nanos(89);

        let err =
            CompressedSMADivision::new(started_at, updated_at, value, prev_value).unwrap_err();

        assert_eq!(
            err,
            ContractError::change_limit_error(
                "`updated_at` must be greater than or equal to `started_at`"
            )
        );
    }

    #[test]
    fn test_update_compressed_division() {
        let started_at = Timestamp::from_nanos(90);
        let updated_at = Timestamp::from_nanos(100);
        let value = Decimal::percent(20);
        let prev_value = Decimal::percent(10);
        let compressed_division =
            CompressedSMADivision::new(started_at, updated_at, value, prev_value).unwrap();

        let updated_at = Timestamp::from_nanos(120);
        let value = Decimal::percent(20);
        let updated_compressed_division = compressed_division.update(updated_at, value).unwrap();

        assert_eq!(
            updated_compressed_division,
            CompressedSMADivision {
                started_at,
                updated_at,
                latest_value: value,
                integral: (Decimal::percent(10) * Decimal::from_ratio(10u128, 1u128))
                    + (Decimal::percent(20) * Decimal::from_ratio(20u128, 1u128))
            }
        );
    }

    #[test]
    fn test_average_empty_iter() {
        let divisions = vec![];
        let division_size = Uint64::from(100u64);
        let window_size = Uint64::from(1000u64);
        let block_time = Timestamp::from_nanos(1100);
        let average = CompressedSMADivision::compressed_moving_average(
            None,
            divisions.into_iter(),
            division_size,
            window_size,
            block_time,
        );

        assert_eq!(
            average.unwrap_err(),
            ContractError::Std(StdError::generic_err(
                "there is no data point since last reset"
            ))
        );
    }

    #[test]
    fn test_average_single_div() {
        let started_at = Timestamp::from_nanos(1100);
        let updated_at = Timestamp::from_nanos(1110);
        let value = Decimal::percent(20);
        let prev_value = Decimal::percent(10);
        let compressed_division =
            CompressedSMADivision::new(started_at, updated_at, value, prev_value).unwrap();

        let divisions = vec![compressed_division];
        let division_size = Uint64::from(100u64);
        let window_size = Uint64::from(1000u64);
        let block_time = Timestamp::from_nanos(1110);
        let average = CompressedSMADivision::compressed_moving_average(
            None,
            divisions.clone().into_iter(),
            division_size,
            window_size,
            block_time,
        )
        .unwrap();

        // used to be x 10 / 10
        // but now it is x 100 / 10
        assert_eq!(average, prev_value);

        let block_time = Timestamp::from_nanos(1115);
        let average = CompressedSMADivision::compressed_moving_average(
            None,
            divisions.clone().into_iter(),
            division_size,
            window_size,
            block_time,
        )
        .unwrap();

        assert_eq!(
            average,
            ((prev_value * Decimal::from_ratio(10u128, 1u128))
                + (value * Decimal::from_ratio(5u128, 1u128)))
                / Decimal::from_ratio(15u128, 1u128)
        );

        // half way to the division size
        let block_time = Timestamp::from_nanos(1150);
        let average = CompressedSMADivision::compressed_moving_average(
            None,
            divisions.clone().into_iter(),
            division_size,
            window_size,
            block_time,
        )
        .unwrap();

        assert_eq!(
            average,
            ((prev_value * Decimal::from_ratio(10u128, 1u128))
                + (value * Decimal::from_ratio(40u128, 1u128)))
                / Decimal::from_ratio(50u128, 1u128)
        );

        // at the division edge
        let block_time = Timestamp::from_nanos(1200);
        let average = CompressedSMADivision::compressed_moving_average(
            None,
            divisions.clone().into_iter(),
            division_size,
            window_size,
            block_time,
        )
        .unwrap();

        assert_eq!(
            average,
            ((prev_value * Decimal::from_ratio(10u128, 1u128))
                + (value * Decimal::from_ratio(90u128, 1u128)))
                / Decimal::from_ratio(100u128, 1u128)
        );

        // at the division edge but there is some update before
        let update_time = Timestamp::from_nanos(1150);
        let updated_value = Decimal::percent(30);

        let updated_division = divisions
            .into_iter()
            .next()
            .unwrap()
            .update(update_time, updated_value)
            .unwrap();

        let divisions = vec![updated_division];

        let block_time = Timestamp::from_nanos(1200);
        let average = CompressedSMADivision::compressed_moving_average(
            None,
            divisions.into_iter(),
            division_size,
            window_size,
            block_time,
        )
        .unwrap();

        assert_eq!(
            average,
            ((prev_value * Decimal::from_ratio(10u128, 1u128))
                + (value * Decimal::from_ratio(40u128, 1u128))
                + (updated_value * Decimal::from_ratio(50u128, 1u128)))
                / Decimal::from_ratio(100u128, 1u128)
        );
    }

    #[test]
    fn test_average_double_divs() {
        let division_size = Uint64::from(100u64);
        let window_size = Uint64::from(1000u64);

        let divisions = vec![
            {
                let started_at = Timestamp::from_nanos(1100);
                let updated_at = Timestamp::from_nanos(1110);
                let value = Decimal::percent(20);
                let prev_value = Decimal::percent(10);
                CompressedSMADivision::new(started_at, updated_at, value, prev_value).unwrap()
            },
            {
                let started_at = Timestamp::from_nanos(1200);
                let updated_at = Timestamp::from_nanos(1260);
                let value = Decimal::percent(30);
                let prev_value = Decimal::percent(20);
                CompressedSMADivision::new(started_at, updated_at, value, prev_value).unwrap()
            },
        ];

        let block_time = Timestamp::from_nanos(1270);
        let average = CompressedSMADivision::compressed_moving_average(
            None,
            divisions.into_iter(),
            division_size,
            window_size,
            block_time,
        )
        .unwrap();

        assert_eq!(
            average,
            ((Decimal::percent(10) * Decimal::from_ratio(10u128, 1u128))
                + (Decimal::percent(20) * Decimal::from_ratio(90u128, 1u128))
                + (Decimal::percent(20) * Decimal::from_ratio(60u128, 1u128))
                + (Decimal::percent(30) * Decimal::from_ratio(10u128, 1u128)))
                / Decimal::from_ratio(170u128, 1u128)
        );
    }

    #[test]
    fn test_average_tripple_divs() {
        let division_size = Uint64::from(100u64);
        let window_size = Uint64::from(1000u64);

        let divisions = vec![
            {
                let started_at = Timestamp::from_nanos(1100);
                let updated_at = Timestamp::from_nanos(1110);
                let value = Decimal::percent(20);
                let prev_value = Decimal::percent(10);
                CompressedSMADivision::new(started_at, updated_at, value, prev_value).unwrap()
            },
            {
                let started_at = Timestamp::from_nanos(1200);
                let updated_at = Timestamp::from_nanos(1260);
                let value = Decimal::percent(30);
                let prev_value = Decimal::percent(20);
                CompressedSMADivision::new(started_at, updated_at, value, prev_value).unwrap()
            },
            {
                let started_at = Timestamp::from_nanos(1300);
                let updated_at = Timestamp::from_nanos(1340);
                let value = Decimal::percent(40);
                let prev_value = Decimal::percent(30);
                CompressedSMADivision::new(started_at, updated_at, value, prev_value).unwrap()
            },
        ];

        let block_time = Timestamp::from_nanos(1370);

        let average = CompressedSMADivision::compressed_moving_average(
            None,
            divisions.into_iter(),
            division_size,
            window_size,
            block_time,
        )
        .unwrap();

        assert_eq!(
            average,
            ((Decimal::percent(10) * Decimal::from_ratio(10u128, 1u128))
                + (Decimal::percent(20) * Decimal::from_ratio(90u128, 1u128))
                + (Decimal::percent(20) * Decimal::from_ratio(60u128, 1u128))
                + (Decimal::percent(30) * Decimal::from_ratio(40u128, 1u128))
                + (Decimal::percent(30) * Decimal::from_ratio(40u128, 1u128))
                + (Decimal::percent(40) * Decimal::from_ratio(30u128, 1u128)))
                / Decimal::from_ratio(270u128, 1u128)
        );
    }

    #[test]
    fn test_average_when_div_is_in_overlapping_window() {
        let division_size = Uint64::from(200u64);
        let window_size = Uint64::from(600u64);

        let divisions = vec![
            {
                let started_at = Timestamp::from_nanos(1100);
                let updated_at = Timestamp::from_nanos(1110);
                let value = Decimal::percent(20);
                let prev_value = Decimal::percent(10);
                CompressedSMADivision::new(started_at, updated_at, value, prev_value).unwrap()
            },
            {
                let started_at = Timestamp::from_nanos(1300);
                let updated_at = Timestamp::from_nanos(1360);
                let value = Decimal::percent(30);
                let prev_value = Decimal::percent(20);
                CompressedSMADivision::new(started_at, updated_at, value, prev_value).unwrap()
            },
            {
                let started_at = Timestamp::from_nanos(1500);
                let updated_at = Timestamp::from_nanos(1640);
                let value = Decimal::percent(40);
                let prev_value = Decimal::percent(30);
                CompressedSMADivision::new(started_at, updated_at, value, prev_value).unwrap()
            },
        ];

        let block_time = Timestamp::from_nanos(1700);

        let average = CompressedSMADivision::compressed_moving_average(
            None,
            divisions.clone().into_iter(),
            division_size,
            window_size,
            block_time,
        )
        .unwrap();

        assert_eq!(
            average,
            ((Decimal::percent(10) * Decimal::from_ratio(10u128, 1u128))
                + (Decimal::percent(20) * Decimal::from_ratio(190u128, 1u128))
                + (Decimal::percent(20) * Decimal::from_ratio(60u128, 1u128))
                + (Decimal::percent(30) * Decimal::from_ratio(140u128, 1u128))
                + (Decimal::percent(30) * Decimal::from_ratio(140u128, 1u128))
                + (Decimal::percent(40) * Decimal::from_ratio(60u128, 1u128)))
                / Decimal::from_ratio(600u128, 1u128)
        );

        let base_divisions = divisions;

        let divisions = vec![
            base_divisions.clone(),
            vec![{
                let started_at = Timestamp::from_nanos(1700);
                let updated_at = Timestamp::from_nanos(1700);
                let value = Decimal::percent(50);
                let prev_value = Decimal::percent(40);
                CompressedSMADivision::new(started_at, updated_at, value, prev_value).unwrap()
            }],
        ]
        .concat();

        let block_time = Timestamp::from_nanos(1705);

        let average = CompressedSMADivision::compressed_moving_average(
            None,
            divisions.into_iter(),
            division_size,
            window_size,
            block_time,
        )
        .unwrap();

        assert_eq!(
            average,
            ((Decimal::percent(10) * Decimal::from_ratio(5u128, 1u128))
                + (Decimal::percent(20) * Decimal::from_ratio(190u128, 1u128))
                + (Decimal::percent(20) * Decimal::from_ratio(60u128, 1u128))
                + (Decimal::percent(30) * Decimal::from_ratio(140u128, 1u128))
                + (Decimal::percent(30) * Decimal::from_ratio(140u128, 1u128))
                + (Decimal::percent(40) * Decimal::from_ratio(60u128, 1u128))
                + (Decimal::percent(50) * Decimal::from_ratio(5u128, 1u128)))
                / Decimal::from_ratio(600u128, 1u128)
        );

        let divisions = vec![
            base_divisions.clone(),
            vec![{
                let started_at = Timestamp::from_nanos(1700);
                let updated_at = Timestamp::from_nanos(1701);
                let value = Decimal::percent(50);
                let prev_value = Decimal::percent(40);
                CompressedSMADivision::new(started_at, updated_at, value, prev_value).unwrap()
            }],
        ]
        .concat();

        let block_time = Timestamp::from_nanos(1705);

        let average = CompressedSMADivision::compressed_moving_average(
            None,
            divisions.into_iter(),
            division_size,
            window_size,
            block_time,
        )
        .unwrap();

        assert_eq!(
            average,
            ((Decimal::percent(10) * Decimal::from_ratio(5u128, 1u128))
                + (Decimal::percent(20) * Decimal::from_ratio(190u128, 1u128))
                + (Decimal::percent(20) * Decimal::from_ratio(60u128, 1u128))
                + (Decimal::percent(30) * Decimal::from_ratio(140u128, 1u128))
                + (Decimal::percent(30) * Decimal::from_ratio(140u128, 1u128))
                + (Decimal::percent(40) * Decimal::from_ratio(60u128, 1u128))
                + (Decimal::percent(40) * Decimal::from_ratio(1u128, 1u128))
                + (Decimal::percent(50) * Decimal::from_ratio(4u128, 1u128)))
                / Decimal::from_ratio(600u128, 1u128)
        );

        let divisions = vec![
            base_divisions,
            vec![{
                let started_at = Timestamp::from_nanos(1700);
                let updated_at = Timestamp::from_nanos(1740);
                let value = Decimal::percent(50);
                let prev_value = Decimal::percent(40);
                CompressedSMADivision::new(started_at, updated_at, value, prev_value).unwrap()
            }],
        ]
        .concat();

        let block_time = Timestamp::from_nanos(1740);

        let average = CompressedSMADivision::compressed_moving_average(
            None,
            divisions.clone().into_iter(),
            division_size,
            window_size,
            block_time,
        )
        .unwrap();

        assert_eq!(
            average,
            ((Decimal::percent(20) * Decimal::from_ratio(160u128, 1u128)) // 32
                    + (Decimal::percent(20) * Decimal::from_ratio(60u128, 1u128)) // 32 + 12 = 44
                    + (Decimal::percent(30) * Decimal::from_ratio(140u128, 1u128)) // 44 + 42 = 86
                    + (Decimal::percent(30) * Decimal::from_ratio(140u128, 1u128)) // 86 + 42 = 128
                    + (Decimal::percent(40) * Decimal::from_ratio(60u128, 1u128)) // 128 + 24 = 152
                    + (Decimal::percent(40) * Decimal::from_ratio(40u128, 1u128))) // 152 + 16 = 168
                    / Decimal::from_ratio(600u128, 1u128)
        );

        let block_time = Timestamp::from_nanos(1899);

        let average = CompressedSMADivision::compressed_moving_average(
            None,
            divisions.into_iter(),
            division_size,
            window_size,
            block_time,
        )
        .unwrap();

        assert_eq!(
            average,
            ((Decimal::percent(20) * Decimal::from_ratio(1u128, 1u128))
                + (Decimal::percent(20) * Decimal::from_ratio(60u128, 1u128))
                + (Decimal::percent(30) * Decimal::from_ratio(140u128, 1u128))
                + (Decimal::percent(30) * Decimal::from_ratio(140u128, 1u128))
                + (Decimal::percent(40) * Decimal::from_ratio(60u128, 1u128))
                + (Decimal::percent(40) * Decimal::from_ratio(40u128, 1u128))
                + (Decimal::percent(50) * Decimal::from_ratio(159u128, 1u128)))
                / Decimal::from_ratio(600u128, 1u128)
        );
    }

    #[test]
    fn test_average_when_div_is_skipping() {
        // skipping 1 division
        let division_size = Uint64::from(200u64);
        let window_size = Uint64::from(600u64);

        let divisions = vec![
            {
                let started_at = Timestamp::from_nanos(1100);
                let updated_at = Timestamp::from_nanos(1110);
                let value = Decimal::percent(20);
                let prev_value = Decimal::percent(10);
                CompressedSMADivision::new(started_at, updated_at, value, prev_value).unwrap()
            },
            // -- skip 1300 -> 1500 --
            // 20% * 200 - 1 div size
            {
                let started_at = Timestamp::from_nanos(1500);
                let updated_at = Timestamp::from_nanos(1540);
                let value = Decimal::percent(30);
                let prev_value = Decimal::percent(20);
                CompressedSMADivision::new(started_at, updated_at, value, prev_value).unwrap()
            },
        ];

        let block_time = Timestamp::from_nanos(1600);

        let average = CompressedSMADivision::compressed_moving_average(
            None,
            divisions.clone().into_iter(),
            division_size,
            window_size,
            block_time,
        )
        .unwrap();

        assert_eq!(
            average,
            ((Decimal::percent(10) * Decimal::from_ratio(10u128, 1u128))
                + (Decimal::percent(20) * Decimal::from_ratio(190u128, 1u128))
                + (Decimal::percent(20) * Decimal::from_ratio(200u128, 1u128)) // skipped div
                + (Decimal::percent(20) * Decimal::from_ratio(40u128, 1u128))
                + (Decimal::percent(30) * Decimal::from_ratio(60u128, 1u128)))
                / Decimal::from_ratio(500u128, 1u128)
        );

        let average = CompressedSMADivision::compressed_moving_average(
            Some(
                CompressedSMADivision::new(
                    Timestamp::from_nanos(700),
                    Timestamp::from_nanos(750),
                    Decimal::percent(10),
                    Decimal::percent(15),
                )
                .unwrap(),
            ),
            divisions.clone().into_iter(),
            division_size,
            window_size,
            block_time,
        )
        .unwrap();

        assert_eq!(
            average,
            ((Decimal::percent(10) * Decimal::from_ratio(100u128, 1u128)) // before first div
                + (Decimal::percent(10) * Decimal::from_ratio(10u128, 1u128))
                + (Decimal::percent(20) * Decimal::from_ratio(190u128, 1u128))
                + (Decimal::percent(20) * Decimal::from_ratio(200u128, 1u128)) // skipped div
                + (Decimal::percent(20) * Decimal::from_ratio(40u128, 1u128))
                + (Decimal::percent(30) * Decimal::from_ratio(60u128, 1u128)))
                / Decimal::from_ratio(600u128, 1u128)
        );

        let block_time = Timestamp::from_nanos(1700);
        let average = CompressedSMADivision::compressed_moving_average(
            Some(
                CompressedSMADivision::new(
                    Timestamp::from_nanos(700),
                    Timestamp::from_nanos(750),
                    Decimal::percent(10),
                    Decimal::percent(15),
                )
                .unwrap(),
            ),
            divisions.into_iter(),
            division_size,
            window_size,
            block_time,
        )
        .unwrap();

        assert_eq!(
            average,
            ((Decimal::percent(10) * Decimal::from_ratio(10u128, 1u128))
                + (Decimal::percent(20) * Decimal::from_ratio(190u128, 1u128))
                + (Decimal::percent(20) * Decimal::from_ratio(200u128, 1u128)) // skipped div
                + (Decimal::percent(20) * Decimal::from_ratio(40u128, 1u128))
                + (Decimal::percent(30) * Decimal::from_ratio(160u128, 1u128)))
                / Decimal::from_ratio(600u128, 1u128)
        );

        // skipping 2 divisions
        let division_size = Uint64::from(100u64);
        let window_size = Uint64::from(600u64);

        let divisions = vec![
            {
                let started_at = Timestamp::from_nanos(1100);
                let updated_at = Timestamp::from_nanos(1110);
                let value = Decimal::percent(20);
                let prev_value = Decimal::percent(10);
                CompressedSMADivision::new(started_at, updated_at, value, prev_value).unwrap()
            },
            // -- skip 1300 -> 1500 --
            // 20% * 200 - 2 div size
            {
                let started_at = Timestamp::from_nanos(1500);
                let updated_at = Timestamp::from_nanos(1540);
                let value = Decimal::percent(30);
                let prev_value = Decimal::percent(20);
                CompressedSMADivision::new(started_at, updated_at, value, prev_value).unwrap()
            },
        ];

        let block_time = Timestamp::from_nanos(1600);

        let average = CompressedSMADivision::compressed_moving_average(
            None,
            divisions.clone().into_iter(),
            division_size,
            window_size,
            block_time,
        )
        .unwrap();

        assert_eq!(
            average,
            ((Decimal::percent(10) * Decimal::from_ratio(10u128, 1u128))
                + (Decimal::percent(20) * Decimal::from_ratio(190u128, 1u128))
                + (Decimal::percent(20) * Decimal::from_ratio(100u128, 1u128)) // skipped div
                + (Decimal::percent(20) * Decimal::from_ratio(100u128, 1u128)) // skipped div
                + (Decimal::percent(20) * Decimal::from_ratio(40u128, 1u128))
                + (Decimal::percent(30) * Decimal::from_ratio(60u128, 1u128)))
                / Decimal::from_ratio(500u128, 1u128)
        );

        let block_time = Timestamp::from_nanos(1710);

        let average = CompressedSMADivision::compressed_moving_average(
            None,
            divisions.into_iter(),
            division_size,
            window_size,
            block_time,
        )
        .unwrap();

        assert_eq!(
            average,
            ((Decimal::percent(20) * Decimal::from_ratio(190u128, 1u128))
                + (Decimal::percent(20) * Decimal::from_ratio(100u128, 1u128)) // skipped div
                + (Decimal::percent(20) * Decimal::from_ratio(100u128, 1u128)) // skipped div
                + (Decimal::percent(20) * Decimal::from_ratio(40u128, 1u128))
                + (Decimal::percent(30) * Decimal::from_ratio(170u128, 1u128)))
                / Decimal::from_ratio(600u128, 1u128)
        );
    }

    #[test]
    fn test_outdated() {
        let division = CompressedSMADivision {
            started_at: Timestamp::from_nanos(1000000000),
            updated_at: Timestamp::from_nanos(1000000022),
            latest_value: Decimal::percent(10),
            integral: Decimal::percent(22),
        };
        let window_size = Uint64::from(1000u64);
        let division_size = Uint64::from(100u64);

        let block_time = Timestamp::from_nanos(1000000022);

        // with window
        assert!(!division
            .is_outdated(block_time, window_size, division_size)
            .unwrap());

        let block_time = Timestamp::from_nanos(1000000999);
        assert!(!division
            .is_outdated(block_time, window_size, division_size)
            .unwrap());

        let block_time = Timestamp::from_nanos(1000001000);
        assert!(!division
            .is_outdated(block_time, window_size, division_size)
            .unwrap());

        let block_time = Timestamp::from_nanos(1000001099);
        assert!(!division
            .is_outdated(block_time, window_size, division_size)
            .unwrap());

        // out of window
        let block_time = Timestamp::from_nanos(1000001100);
        assert!(division
            .is_outdated(block_time, window_size, division_size)
            .unwrap());

        let block_time = Timestamp::from_nanos(1000001101);
        assert!(division
            .is_outdated(block_time, window_size, division_size)
            .unwrap());

        let block_time = Timestamp::from_nanos(1000001200);
        assert!(division
            .is_outdated(block_time, window_size, division_size)
            .unwrap());
    }

    #[test]
    fn test_next_started_at() {
        let division_size = Uint64::from(10u64);

        let started_at = Timestamp::from_nanos(90);
        let compressed_sma_division = CompressedSMADivision {
            started_at,
            updated_at: Timestamp::from_nanos(91),
            latest_value: Decimal::zero(),
            integral: Decimal::zero(),
        };

        let block_time = Timestamp::from_nanos(100);
        let result = compressed_sma_division.next_started_at(division_size, block_time);
        assert_eq!(result.unwrap(), Timestamp::from_nanos(100));

        let block_time = Timestamp::from_nanos(105);

        let result = compressed_sma_division.next_started_at(division_size, block_time);
        assert_eq!(result.unwrap(), Timestamp::from_nanos(100));

        let block_time = Timestamp::from_nanos(115);

        let result = compressed_sma_division.next_started_at(division_size, block_time);
        assert_eq!(result.unwrap(), Timestamp::from_nanos(110));

        let block_time = Timestamp::from_nanos(202);

        let result = compressed_sma_division.next_started_at(division_size, block_time);
        assert_eq!(result.unwrap(), Timestamp::from_nanos(200));
    }
}
