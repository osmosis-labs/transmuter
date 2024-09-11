mod division;
mod errors;
mod helpers;

pub mod rebalancing_incentive;

pub use cosmwasm_std::{Decimal, Timestamp, Uint64};
pub use division::Division;
pub use errors::TransmuterMathError;

pub fn compressed_moving_average(
    latest_removed_division: Option<Division>,
    divisions: &[Division],
    division_size: Uint64,
    window_size: Uint64,
    block_time: Timestamp,
) -> Result<Decimal, TransmuterMathError> {
    Division::compressed_moving_average(
        latest_removed_division,
        divisions,
        division_size,
        window_size,
        block_time,
    )
}
