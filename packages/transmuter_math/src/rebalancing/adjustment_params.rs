use crate::rebalancing::{
    range::{Bound, Range},
    zone::Zone,
};
use cosmwasm_std::Decimal;

pub struct AdjustmentParams {
    pub ideal: Range,
    pub critical: Range,
    pub limit: Decimal,
    pub adjustment_rate_strained: Decimal,
    pub adjustment_rate_critical: Decimal,
}

impl AdjustmentParams {
    pub fn zones(&self) -> [Zone; 5] {
        // critical low: [0, critical.start) - highest incentive to move out
        let critical_low = Zone::new(
            Bound::Inclusive(Decimal::zero()),
            Bound::Exclusive(self.critical.start().value()),
            self.adjustment_rate_critical,
        );

        // strained low: [critical.start, ideal.start) - moderate incentive to move up
        let strained_low = Zone::new(
            Bound::Inclusive(self.critical.start().value()),
            Bound::Exclusive(self.ideal.start().value()),
            self.adjustment_rate_strained,
        );

        // ideal zone: [ideal.start, ideal.end] - neutral, no fees or incentives
        let ideal = Zone::new(
            Bound::Inclusive(self.ideal.start().value()),
            Bound::Inclusive(self.ideal.end().value()),
            Decimal::zero(),
        );

        // strained high: (ideal.end, critical.end] - moderate incentive to move down
        let strained_high = Zone::new(
            Bound::Exclusive(self.ideal.end().value()),
            Bound::Inclusive(self.critical.end().value()),
            self.adjustment_rate_strained,
        );

        // critical high: (critical.end, limit] - highest incentive to move out
        let critical_high = Zone::new(
            Bound::Exclusive(self.critical.end().value()),
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
