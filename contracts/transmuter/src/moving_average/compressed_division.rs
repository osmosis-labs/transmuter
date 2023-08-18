use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Decimal, Timestamp, Uint64};

use crate::ContractError;

/// CompressedDivision is a compressed representation of a compressed sliding window
/// for calculating approximated simple moving average.
#[cw_serde]
pub struct CompressedDivision {
    pub start_time: Timestamp,
    pub cumsum: Decimal,
    pub n: Uint64,
}

impl CompressedDivision {
    pub fn start(start_time: Timestamp, value: Decimal) -> Self {
        Self {
            start_time,
            cumsum: value,
            n: Uint64::one(),
        }
    }

    pub fn accum(&self, value: Decimal) -> Result<Self, ContractError> {
        Ok(CompressedDivision {
            start_time: self.start_time,
            cumsum: self
                .cumsum
                .checked_add(value)
                .map_err(ContractError::calculation_error)?,
            n: self
                .n
                .checked_add(Uint64::one())
                .map_err(ContractError::calculation_error)?,
        })
    }

    pub fn elasped_time(&self, block_time: Timestamp) -> Result<Uint64, ContractError> {
        Uint64::from(block_time.nanos())
            .checked_sub(self.start_time.nanos().into())
            .map_err(ContractError::calculation_error)
    }

    pub fn average(&self) -> Result<Decimal, ContractError> {
        let n = Decimal::from_atomics(self.n, 0).map_err(ContractError::calculation_error)?;

        self.cumsum
            .checked_div(n)
            .map_err(ContractError::calculation_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compressed_division() {
        // Create a new CompressedDivision
        let start_time = Timestamp::from_nanos(0);
        let value0 = Decimal::percent(10);
        let compressed_division = CompressedDivision::start(start_time, value0);

        // Accumulate values
        let value1 = Decimal::percent(30);
        let value2 = Decimal::percent(40);
        let value3 = Decimal::percent(50);
        let updated_division = compressed_division
            .accum(value1)
            .unwrap()
            .accum(value2)
            .unwrap()
            .accum(value3)
            .unwrap();

        // Verify the accumulated values
        assert_eq!(updated_division.start_time, start_time);
        assert_eq!(updated_division.cumsum, value0 + value1 + value2 + value3);
        assert_eq!(updated_division.n, Uint64::from(4u64));
    }
}
