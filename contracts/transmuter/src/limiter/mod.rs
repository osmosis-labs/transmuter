mod compressed_sma_division;
mod compressed_sma_limiter;
mod helpers;

pub use compressed_sma_limiter::CompressedSMALimiterManager;
use cosmwasm_std::{Decimal, Storage, Timestamp};

use crate::ContractError;

pub trait Limiter {
    fn check_limit_and_update(
        &self,
        storage: &mut dyn Storage,
        block_time: Timestamp,
        value: Decimal,
    ) -> Result<(), ContractError>;
}
