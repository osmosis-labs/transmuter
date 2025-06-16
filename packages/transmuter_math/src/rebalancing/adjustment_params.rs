use crate::rebalancing::{
    range::{Bound, Range},
    zone::Zone,
};
use cosmwasm_std::Decimal;
use thiserror::Error;

#[derive(Debug)]
pub struct AdjustmentParams {
    ideal_start: Decimal,
    ideal_end: Decimal,
    critical_start: Decimal,
    critical_end: Decimal,
    limit: Decimal,
    adjustment_rate_strained: Decimal,
    adjustment_rate_critical: Decimal,
}

#[derive(Debug, Error, PartialEq)]
pub enum AdjustmentParamsError {
    #[error("critical range must be within [0, {limit}]")]
    CriticalRangeOutOfBounds { limit: Decimal },
    #[error("ideal range must be within critical range [{critical_start}, {critical_end}]")]
    IdealRangeOutOfBounds {
        critical_start: Decimal,
        critical_end: Decimal,
    },
}

impl AdjustmentParams {
    pub fn new(
        ideal_start: Decimal,
        ideal_end: Decimal,
        critical_start: Decimal,
        critical_end: Decimal,
        limit: Decimal,
        adjustment_rate_strained: Decimal,
        adjustment_rate_critical: Decimal,
    ) -> Result<Self, AdjustmentParamsError> {
        // Validate critical range is within [0, limit]
        if critical_start < Decimal::zero() || critical_end > limit {
            return Err(AdjustmentParamsError::CriticalRangeOutOfBounds { limit });
        }

        // Validate ideal range is within critical range
        if ideal_start < critical_start || ideal_end > critical_end {
            return Err(AdjustmentParamsError::IdealRangeOutOfBounds {
                critical_start,
                critical_end,
            });
        }

        Ok(Self {
            ideal_start,
            ideal_end,
            critical_start,
            critical_end,
            limit,
            adjustment_rate_strained,
            adjustment_rate_critical,
        })
    }

    pub fn ideal(&self) -> Range {
        Range::new(
            Bound::Inclusive(self.ideal_start),
            Bound::Inclusive(self.ideal_end),
        )
        .unwrap()
    }

    pub fn zones(&self) -> [Zone; 5] {
        // critical low: [0, critical.start) - highest incentive to move out
        let critical_low = Zone::new(
            Bound::Inclusive(Decimal::zero()),
            Bound::Exclusive(self.critical_start),
            self.adjustment_rate_critical,
        );

        // strained low: [critical.start, ideal.start) - moderate incentive to move up
        let strained_low = Zone::new(
            Bound::Inclusive(self.critical_start),
            Bound::Exclusive(self.ideal_start),
            self.adjustment_rate_strained,
        );

        // ideal zone: [ideal.start, ideal.end] - neutral, no fees or incentives
        let ideal = Zone::new(
            Bound::Inclusive(self.ideal_start),
            Bound::Inclusive(self.ideal_end),
            Decimal::zero(),
        );

        // strained high: (ideal.end, critical.end] - moderate incentive to move down
        let strained_high = Zone::new(
            Bound::Exclusive(self.ideal_end),
            Bound::Inclusive(self.critical_end),
            self.adjustment_rate_strained,
        );

        // critical high: (critical.end, limit] - highest incentive to move out
        let critical_high = Zone::new(
            Bound::Exclusive(self.critical_end),
            Bound::Inclusive(self.limit),
            self.adjustment_rate_critical,
        );

        [
            critical_low,
            strained_low,
            ideal,
            strained_high,
            critical_high,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zones() {
        let params = AdjustmentParams::new(
            Decimal::percent(40),
            Decimal::percent(60),
            Decimal::percent(20),
            Decimal::percent(80),
            Decimal::percent(90),
            Decimal::percent(1),
            Decimal::percent(2),
        )
        .unwrap();

        let expected_zones = [
            // critical low: [0, critical.start)
            Zone::new(
                Bound::Inclusive(Decimal::zero()),
                Bound::Exclusive(Decimal::percent(20)),
                Decimal::percent(2),
            ),
            // strained low: [critical.start, ideal.start)
            Zone::new(
                Bound::Inclusive(Decimal::percent(20)),
                Bound::Exclusive(Decimal::percent(40)),
                Decimal::percent(1),
            ),
            // ideal zone: [ideal.start, ideal.end]
            Zone::new(
                Bound::Inclusive(Decimal::percent(40)),
                Bound::Inclusive(Decimal::percent(60)),
                Decimal::zero(),
            ),
            // strained high: (ideal.end, critical.end]
            Zone::new(
                Bound::Exclusive(Decimal::percent(60)),
                Bound::Inclusive(Decimal::percent(80)),
                Decimal::percent(1),
            ),
            // critical high: (critical.end, limit]
            Zone::new(
                Bound::Exclusive(Decimal::percent(80)),
                Bound::Inclusive(Decimal::percent(90)),
                Decimal::percent(2),
            ),
        ];

        let actual_zones = params.zones();
        assert_eq!(actual_zones, expected_zones);
    }

    #[test]
    fn test_validation() {
        // Test critical range out of bounds
        let err = AdjustmentParams::new(
            Decimal::percent(40),
            Decimal::percent(60),
            Decimal::percent(20),
            Decimal::percent(100), // Exceeds limit
            Decimal::percent(90),
            Decimal::percent(1),
            Decimal::percent(2),
        )
        .unwrap_err();
        assert_eq!(
            err,
            AdjustmentParamsError::CriticalRangeOutOfBounds {
                limit: Decimal::percent(90)
            }
        );

        // Test ideal range out of bounds
        let err = AdjustmentParams::new(
            Decimal::percent(10), // Below critical start
            Decimal::percent(60),
            Decimal::percent(20),
            Decimal::percent(80),
            Decimal::percent(90),
            Decimal::percent(1),
            Decimal::percent(2),
        )
        .unwrap_err();
        assert_eq!(
            err,
            AdjustmentParamsError::IdealRangeOutOfBounds {
                critical_start: Decimal::percent(20),
                critical_end: Decimal::percent(80),
            }
        );
    }
}
