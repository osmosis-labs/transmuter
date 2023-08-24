use cosmwasm_schema::cw_serde;
use cosmwasm_std::{ensure, Decimal, StdError, Timestamp, Uint64};

use crate::ContractError;

/// CompressedDivision is a compressed representation of a compressed sliding window
/// for calculating approximated moving average.
#[cw_serde]
pub struct CompressedSMADivision {
    /// Time where the division is mark as started
    started_at: Timestamp,

    /// Time where it is last updated
    updated_at: Timestamp,

    /// The latest value that gets updated
    latest_value: Decimal,

    /// cumulative sum of each updated value * elasped time since last update
    cumsum: Decimal,
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

        let elapsed_time =
            Uint64::from(updated_at.nanos()).checked_sub(started_at.nanos().into())?;
        Ok(Self {
            started_at,
            updated_at,
            latest_value: value,
            cumsum: prev_value.checked_mul(Decimal::checked_from_ratio(elapsed_time, 1u128)?)?,
        })
    }

    pub fn update(&self, updated_at: Timestamp, value: Decimal) -> Result<Self, ContractError> {
        let elapsed_time =
            Uint64::from(updated_at.nanos()).checked_sub(self.updated_at.nanos().into())?;
        Ok(Self {
            started_at: self.started_at,
            updated_at,
            latest_value: value,
            cumsum: self.cumsum.checked_add(
                self.latest_value
                    .checked_mul(Decimal::checked_from_ratio(elapsed_time, 1u128)?)?,
            )?,
        })
    }

    pub fn is_outdated(
        &self,
        block_time: Timestamp,
        window_size: Uint64,
        division_size: Uint64,
    ) -> Result<bool, ContractError> {
        let window_started_at = Uint64::from(block_time.nanos()).checked_sub(window_size)?;
        let division_ended_at = Uint64::from(self.started_at.nanos()).checked_add(division_size)?;

        Ok(window_started_at >= division_ended_at)
    }

    pub fn elapsed_time(&self, block_time: Timestamp) -> Result<Uint64, ContractError> {
        let block_time = Uint64::from(block_time.nanos());

        block_time
            .checked_sub(Uint64::from(self.started_at.nanos()))
            .map_err(Into::into)
    }

    // TODO: test this
    pub fn next_started_at(
        &self,
        division_size: Uint64,
        block_time: Timestamp,
    ) -> Result<Timestamp, ContractError> {
        let division_size = Uint64::from(division_size);
        let started_at = Uint64::from(self.started_at.nanos());
        let block_time = Uint64::from(block_time.nanos());
        let ended_at = started_at.checked_add(division_size)?;
        let elapsed_time_after_end = block_time.checked_sub(ended_at)?;
        let elapsed_time_within_next_div = elapsed_time_after_end.checked_rem(division_size)?;

        Ok(Timestamp::from_nanos(
            block_time.checked_sub(elapsed_time_within_next_div)?.u64(),
        ))
    }

    /// This function calculates the average of the divisions in a specified window
    /// The window is defined by the `window_size` and `block_time`
    ///
    /// The calculation is done by accumulating the sum of (value * elapsed_time)
    /// then divide by the total elapsed time
    ///
    /// As this is `CompressionDivision`, not all the data points in the window is stored for gas optimization
    /// When the window covers portion of the first division, it needs to readjust the cumsum
    /// based on how far the window start time eats in to the first division proportionally.
    ///
    /// ## Assumptions
    /// - Divisions are sorted by started_at
    /// - Last division's updated_at is less than block_time
    /// - All divisions are within the window or at least overlap with the window
    /// - All divisions are of the same size
    pub fn compressed_moving_average(
        divisions: impl IntoIterator<Item = Self>,
        division_size: Uint64,
        window_size: Uint64,
        block_time: Timestamp,
    ) -> Result<Decimal, ContractError> {
        let mut divisions = divisions.into_iter();
        let window_started_at = Uint64::from(block_time.nanos()).checked_sub(window_size)?;

        let first_division = divisions.next();

        // keep track of the lastest division
        let mut lastest_division = first_division.clone();

        // process first division
        // this needs to be handled differently because the first division's cumsum needs to be recalibrated
        // based on how far window start time eats in to the first division
        let (first_div_stared_at, mut cumsum) = match first_division {
            Some(division) => {
                let division_started_at = Uint64::from(division.started_at.nanos());
                let remaining_division_size = division_started_at
                    .checked_add(division_size)?
                    .checked_sub(window_started_at)?
                    .min(division_size);

                let latest_value_elapsed_time =
                    division.latest_value_elapsed_time(division_size, block_time)?;

                let cumsum = if remaining_division_size > latest_value_elapsed_time {
                    // readjust cumsum if window start after first division
                    // if the window start before the first division, then the first division's cumsum can be used as is
                    let window_started_after_first_division =
                        window_started_at > division_started_at;

                    let cumsum = if window_started_after_first_division {
                        division
                            .adjusted_cumsum(remaining_division_size, latest_value_elapsed_time)?
                    } else {
                        division.cumsum
                    };

                    cumsum
                        .checked_add(division.weighted_latest_value(division_size, block_time)?)?
                } else {
                    division
                        .latest_value
                        .checked_mul(Decimal::checked_from_ratio(remaining_division_size, 1u128)?)?
                };

                (division.started_at, cumsum)
            }
            None => return Err(StdError::not_found("division").into()),
        };

        // accumulate cumsum from the rest of divisions
        for division in divisions {
            cumsum = cumsum
                .checked_add(division.cumsum)?
                .checked_add(division.weighted_latest_value(division_size, block_time)?)?;

            lastest_division = Some(division);
        }

        // if latest division end before block time,
        // add latest value * elasped time after latest division to cumsum
        if let Some(latest_division) = lastest_division {
            let latest_division_ended_at =
                Uint64::from(latest_division.started_at.nanos()).checked_add(division_size)?;

            if Uint64::from(block_time.nanos()) > latest_division_ended_at {
                let elasped_time_after_latest_division =
                    Uint64::from(block_time.nanos()).checked_sub(latest_division_ended_at)?;

                cumsum = cumsum.checked_add(latest_division.latest_value.checked_mul(
                    Decimal::checked_from_ratio(elasped_time_after_latest_division, 1u128)?,
                )?)?;
            }
        }

        let started_at = window_started_at.max(first_div_stared_at.nanos().into());
        let total_elapsed_time = Uint64::from(block_time.nanos()).checked_sub(started_at)?;

        cumsum
            .checked_div(Decimal::checked_from_ratio(total_elapsed_time, 1u128)?)
            .map_err(Into::into)
    }

    fn adjusted_cumsum(
        &self,
        remaining_division_size: Uint64,
        latest_value_elapsed_time: Uint64,
    ) -> Result<Decimal, ContractError> {
        let current_cumsum_weight =
            Uint64::from(self.updated_at.nanos()).checked_sub(self.started_at.nanos().into())?;

        let new_cumsum_weight = remaining_division_size.checked_sub(latest_value_elapsed_time)?;

        let division_average_before_latest_update = self
            .cumsum
            .checked_div(Decimal::checked_from_ratio(current_cumsum_weight, 1u128)?)?;

        division_average_before_latest_update
            .checked_mul(Decimal::checked_from_ratio(new_cumsum_weight, 1u128)?)
            .map_err(Into::into)
    }

    fn latest_value_elapsed_time(
        &self,
        division_size: Uint64,
        block_time: Timestamp,
    ) -> Result<Uint64, ContractError> {
        let ended_at = Uint64::from(self.started_at.nanos()).checked_add(division_size)?;
        let block_time = Uint64::from(block_time.nanos());
        if block_time > ended_at {
            ended_at.checked_sub(self.updated_at.nanos().into())
        } else {
            block_time.checked_sub(self.updated_at.nanos().into())
        }
        .map_err(Into::into)
    }

    fn weighted_latest_value(
        &self,
        division_size: Uint64,
        block_time: Timestamp,
    ) -> Result<Decimal, ContractError> {
        let elapsed_time = self.latest_value_elapsed_time(division_size, block_time)?;
        self.latest_value
            .checked_mul(Decimal::checked_from_ratio(elapsed_time, 1u128)?)
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
                cumsum: Decimal::percent(10) * Decimal::from_ratio(10u128, 1u128)
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
                cumsum: Decimal::zero()
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
                cumsum: (Decimal::percent(10) * Decimal::from_ratio(10u128, 1u128))
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
            divisions.into_iter(),
            division_size,
            window_size,
            block_time,
        );

        assert_eq!(
            average.unwrap_err(),
            ContractError::Std(StdError::not_found("division"))
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
    fn test_outdated() {
        let division = CompressedSMADivision {
            started_at: Timestamp::from_nanos(1000000000),
            updated_at: Timestamp::from_nanos(1000000022),
            latest_value: Decimal::percent(10),
            cumsum: Decimal::percent(22),
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
}
