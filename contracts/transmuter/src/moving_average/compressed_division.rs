use cosmwasm_schema::cw_serde;
use cosmwasm_std::{OverflowError, Timestamp, Uint128, Uint64};

/// CompressedDivision is a compressed representation of a compressed sliding window
/// for calculating approximated simple moving average.
#[cw_serde]
pub struct CompressedDivision {
    pub start_time: Timestamp,
    pub cumsum: Uint128,
    pub n: Uint64,
}

impl CompressedDivision {
    pub fn start(start_time: Timestamp) -> Self {
        Self {
            start_time,
            cumsum: Uint128::zero(),
            n: Uint64::zero(),
        }
    }

    pub fn accum(&self, value: Uint128) -> Result<Self, OverflowError> {
        Ok(CompressedDivision {
            start_time: self.start_time,
            cumsum: self.cumsum.checked_add(value)?,
            n: self.n.checked_add(Uint64::one())?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmwasm_std::Uint128;

    #[test]
    fn test_compressed_division() {
        // Create a new CompressedDivision
        let start_time = Timestamp::from_nanos(0);
        let compressed_division = CompressedDivision::start(start_time);

        // Accumulate values
        let value1 = Uint128::new(100);
        let value2 = Uint128::new(200);
        let value3 = Uint128::new(300);
        let updated_division = compressed_division
            .accum(value1)
            .unwrap()
            .accum(value2)
            .unwrap()
            .accum(value3)
            .unwrap();

        // Verify the accumulated values
        assert_eq!(updated_division.start_time, start_time);
        assert_eq!(updated_division.cumsum, value1 + value2 + value3);
        assert_eq!(updated_division.n, Uint64::from(3u64));
    }
}
