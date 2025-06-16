use crate::rebalancing::{
    range::{Bound, Range},
    zone::Zone,
};
use cosmwasm_std::Decimal;
use thiserror::Error;

#[derive(Debug)]
pub struct AdjustmentParams {
    ideal_upper: Decimal,
    ideal_lower: Decimal,
    critical_upper: Decimal,
    critical_lower: Decimal,
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
    #[error("ideal range must be ordered (upper <= lower)")]
    InvalidIdealRange,
    #[error("critical range must be ordered (upper <= lower)")]
    InvalidCriticalRange,
}

impl AdjustmentParams {
    pub fn new(
        ideal_upper: Decimal,
        ideal_lower: Decimal,
        critical_upper: Decimal,
        critical_lower: Decimal,
        limit: Decimal,
        adjustment_rate_strained: Decimal,
        adjustment_rate_critical: Decimal,
    ) -> Result<Self, AdjustmentParamsError> {
        // Validate ranges are properly ordered
        if ideal_upper > ideal_lower {
            return Err(AdjustmentParamsError::InvalidIdealRange);
        }
        if critical_upper > critical_lower {
            return Err(AdjustmentParamsError::InvalidCriticalRange);
        }

        // Validate critical range is within [0, limit]
        if critical_upper < Decimal::zero() || critical_lower > limit {
            return Err(AdjustmentParamsError::CriticalRangeOutOfBounds { limit });
        }

        // Validate ideal range is within critical range
        if ideal_upper < critical_upper || ideal_lower > critical_lower {
            return Err(AdjustmentParamsError::IdealRangeOutOfBounds {
                critical_start: critical_upper,
                critical_end: critical_lower,
            });
        }

        Ok(Self {
            ideal_upper,
            ideal_lower,
            critical_upper,
            critical_lower,
            limit,
            adjustment_rate_strained,
            adjustment_rate_critical,
        })
    }

    pub fn ideal(&self) -> Range {
        Range::new(
            Bound::Inclusive(self.ideal_upper),
            Bound::Inclusive(self.ideal_lower),
        )
        .unwrap()
    }

    pub fn zones(&self) -> [Zone; 5] {
        // critical low: [0, critical.start) - highest incentive to move out
        let critical_low = Zone::new(
            Bound::Inclusive(Decimal::zero()),
            Bound::Exclusive(self.critical_upper),
            self.adjustment_rate_critical,
        );

        // strained low: [critical.start, ideal.start) - moderate incentive to move up
        let strained_low = Zone::new(
            Bound::Inclusive(self.critical_upper),
            Bound::Exclusive(self.ideal_upper),
            self.adjustment_rate_strained,
        );

        // ideal zone: [ideal.start, ideal.end] - neutral, no fees or incentives
        let ideal = Zone::new(
            Bound::Inclusive(self.ideal_upper),
            Bound::Inclusive(self.ideal_lower),
            Decimal::zero(),
        );

        // strained high: (ideal.end, critical.end] - moderate incentive to move down
        let strained_high = Zone::new(
            Bound::Exclusive(self.ideal_lower),
            Bound::Inclusive(self.critical_lower),
            self.adjustment_rate_strained,
        );

        // critical high: (critical.end, limit] - highest incentive to move out
        let critical_high = Zone::new(
            Bound::Exclusive(self.critical_lower),
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
    use rstest::rstest;

    #[rstest]
    #[case::valid_parameters(
        Decimal::percent(40),
        Decimal::percent(60),
        Decimal::percent(20),
        Decimal::percent(80),
        Decimal::percent(90),
        Decimal::percent(1),
        Decimal::percent(2),
        true
    )]
    #[case::critical_upper_exceeds_limit(
        Decimal::percent(40),
        Decimal::percent(60),
        Decimal::percent(20),
        Decimal::percent(100),
        Decimal::percent(90),
        Decimal::percent(1),
        Decimal::percent(2),
        false
    )]
    #[case::ideal_lower_below_critical_lower(
        Decimal::percent(10),
        Decimal::percent(60),
        Decimal::percent(20),
        Decimal::percent(80),
        Decimal::percent(90),
        Decimal::percent(1),
        Decimal::percent(2),
        false
    )]
    #[case::ideal_upper_above_critical_upper(
        Decimal::percent(40),
        Decimal::percent(90),
        Decimal::percent(20),
        Decimal::percent(80),
        Decimal::percent(90),
        Decimal::percent(1),
        Decimal::percent(2),
        false
    )]
    #[case::ideal_range_reversed(
        Decimal::percent(60),
        Decimal::percent(40),
        Decimal::percent(20),
        Decimal::percent(80),
        Decimal::percent(90),
        Decimal::percent(1),
        Decimal::percent(2),
        false
    )]
    #[case::critical_range_reversed(
        Decimal::percent(40),
        Decimal::percent(60),
        Decimal::percent(80),
        Decimal::percent(20),
        Decimal::percent(90),
        Decimal::percent(1),
        Decimal::percent(2),
        false
    )]
    #[case::zero_adjustment_rates(
        Decimal::percent(40),
        Decimal::percent(60),
        Decimal::percent(20),
        Decimal::percent(80),
        Decimal::percent(90),
        Decimal::zero(),
        Decimal::zero(),
        true
    )]
    #[case::zero_limit(
        Decimal::percent(40),
        Decimal::percent(60),
        Decimal::percent(20),
        Decimal::percent(80),
        Decimal::zero(),
        Decimal::percent(1),
        Decimal::percent(2),
        false
    )]
    #[case::max_limit(
        Decimal::percent(40),
        Decimal::percent(60),
        Decimal::percent(20),
        Decimal::percent(80),
        Decimal::percent(100),
        Decimal::percent(1),
        Decimal::percent(2),
        true
    )]
    fn test_adjustment_params_validation(
        #[case] ideal_upper: Decimal,
        #[case] ideal_lower: Decimal,
        #[case] critical_upper: Decimal,
        #[case] critical_lower: Decimal,
        #[case] limit: Decimal,
        #[case] adjustment_rate_strained: Decimal,
        #[case] adjustment_rate_critical: Decimal,
        #[case] should_succeed: bool,
    ) {
        let result = AdjustmentParams::new(
            ideal_upper,
            ideal_lower,
            critical_upper,
            critical_lower,
            limit,
            adjustment_rate_strained,
            adjustment_rate_critical,
        );

        assert_eq!(result.is_ok(), should_succeed);
    }

    #[rstest]
    #[case::normal_zones(
        Decimal::percent(40),
        Decimal::percent(60),
        Decimal::percent(20),
        Decimal::percent(80),
        Decimal::percent(90),
        Decimal::percent(1),
        Decimal::percent(2),
        [
            // critical low: [0, critical.upper)
            Zone::new(
                Bound::Inclusive(Decimal::zero()),
                Bound::Exclusive(Decimal::percent(20)),
                Decimal::percent(2),
            ),
            // strained low: [critical.upper, ideal.upper)
            Zone::new(
                Bound::Inclusive(Decimal::percent(20)),
                Bound::Exclusive(Decimal::percent(40)),
                Decimal::percent(1),
            ),
            // ideal zone: [ideal.upper, ideal.lower]
            Zone::new(
                Bound::Inclusive(Decimal::percent(40)),
                Bound::Inclusive(Decimal::percent(60)),
                Decimal::zero(),
            ),
            // strained high: (ideal.lower, critical.lower]
            Zone::new(
                Bound::Exclusive(Decimal::percent(60)),
                Bound::Inclusive(Decimal::percent(80)),
                Decimal::percent(1),
            ),
            // critical high: (critical.lower, limit]
            Zone::new(
                Bound::Exclusive(Decimal::percent(80)),
                Bound::Inclusive(Decimal::percent(90)),
                Decimal::percent(2),
            ),
        ]
    )]
    #[case::tight_ranges(
        Decimal::percent(49),
        Decimal::percent(51),
        Decimal::percent(45),
        Decimal::percent(55),
        Decimal::percent(100),
        Decimal::percent(1),
        Decimal::percent(2),
        [
            Zone::new(
                Bound::Inclusive(Decimal::zero()),
                Bound::Exclusive(Decimal::percent(45)),
                Decimal::percent(2),
            ),
            Zone::new(
                Bound::Inclusive(Decimal::percent(45)),
                Bound::Exclusive(Decimal::percent(49)),
                Decimal::percent(1),
            ),
            Zone::new(
                Bound::Inclusive(Decimal::percent(49)),
                Bound::Inclusive(Decimal::percent(51)),
                Decimal::zero(),
            ),
            Zone::new(
                Bound::Exclusive(Decimal::percent(51)),
                Bound::Inclusive(Decimal::percent(55)),
                Decimal::percent(1),
            ),
            Zone::new(
                Bound::Exclusive(Decimal::percent(55)),
                Bound::Inclusive(Decimal::percent(100)),
                Decimal::percent(2),
            ),
        ]
    )]
    #[case::minimal_ranges(
        Decimal::percent(49),
        Decimal::percent(51),
        Decimal::percent(48),
        Decimal::percent(52),
        Decimal::percent(100),
        Decimal::percent(1),
        Decimal::percent(2),
        [
            Zone::new(
                Bound::Inclusive(Decimal::zero()),
                Bound::Exclusive(Decimal::percent(48)),
                Decimal::percent(2),
            ),
            Zone::new(
                Bound::Inclusive(Decimal::percent(48)),
                Bound::Exclusive(Decimal::percent(49)),
                Decimal::percent(1),
            ),
            Zone::new(
                Bound::Inclusive(Decimal::percent(49)),
                Bound::Inclusive(Decimal::percent(51)),
                Decimal::zero(),
            ),
            Zone::new(
                Bound::Exclusive(Decimal::percent(51)),
                Bound::Inclusive(Decimal::percent(52)),
                Decimal::percent(1),
            ),
            Zone::new(
                Bound::Exclusive(Decimal::percent(52)),
                Bound::Inclusive(Decimal::percent(100)),
                Decimal::percent(2),
            ),
        ]
    )]
    fn test_zones(
        #[case] ideal_upper: Decimal,
        #[case] ideal_lower: Decimal,
        #[case] critical_upper: Decimal,
        #[case] critical_lower: Decimal,
        #[case] limit: Decimal,
        #[case] adjustment_rate_strained: Decimal,
        #[case] adjustment_rate_critical: Decimal,
        #[case] expected_zones: [Zone; 5],
    ) {
        let params = AdjustmentParams::new(
            ideal_upper,
            ideal_lower,
            critical_upper,
            critical_lower,
            limit,
            adjustment_rate_strained,
            adjustment_rate_critical,
        )
        .unwrap();

        let actual_zones = params.zones();
        assert_eq!(actual_zones, expected_zones);
    }
}
