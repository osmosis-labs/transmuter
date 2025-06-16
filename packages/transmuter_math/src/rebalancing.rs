use crate::TransmuterMathError as Error;

#[derive(Copy, Clone, Debug)]
pub struct Range {
    start: f64,
    end: f64,
}

impl Range {
    pub fn new(start: f64, end: f64) -> Result<Self, Error> {
        if start > end {
            return Err(Error::InvalidRange(start, end));
        }

        Ok(Self { start, end })
    }

    /// Returns the overlapping segment between two ranges.
    ///
    /// This function finds the intersection of two ranges, returning a new range
    /// that represents the common segment between them.
    pub fn get_overlap(&self, other: Range) -> Range {
        // Find the intersection of the two ranges
        // segment_start: The later of the two range starts
        // segment_end: The earlier of the two range ends
        let segment_start = other.start.max(self.start);
        let segment_end = other.end.min(self.end);

        Range {
            start: segment_start,
            end: segment_end,
        }
    }
}

/// Represents the direction of a balance change
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BalanceDirection {
    /// Balance is increasing
    Increasing,
    /// Balance is decreasing
    Decreasing,
    /// Balance remains unchanged
    Neutral,
}

/// Represents the impact of a balance shift on the balance
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BalanceShiftImpact {
    /// Shift away from ideal
    Debalance,
    /// Shift towards ideal
    Rebalance,
    /// No impact
    Neutral,
}

impl BalanceShiftImpact {
    /// Returns the numeric value of the impact (-1 for debalance, 1 for rebalance, 0 for neutral)
    pub fn as_f64(&self) -> f64 {
        match self {
            Self::Debalance => -1.0,
            Self::Rebalance => 1.0,
            Self::Neutral => 0.0,
        }
    }
}

/// Represents a balance change with its range and direction
#[derive(Debug, Clone)]
pub struct BalanceShift {
    range: Range,
    direction: BalanceDirection,
}

impl BalanceShift {
    /// Creates a new balance shift from an old balance to a new balance
    ///
    /// # Arguments
    /// * `balance` - The initial balance
    /// * `balance_new` - The new balance
    ///
    /// # Returns
    /// * `Result<Self, Error>` - The balance shift or an error if the range is invalid
    pub fn new(balance: f64, balance_new: f64) -> Result<Self, Error> {
        let range = Range::new(balance.min(balance_new), balance.max(balance_new))?;

        let direction = if balance < balance_new {
            BalanceDirection::Increasing
        } else if balance > balance_new {
            BalanceDirection::Decreasing
        } else {
            BalanceDirection::Neutral
        };
        Ok(Self { range, direction })
    }

    /// Returns the direction of the balance change
    pub fn direction(&self) -> BalanceDirection {
        self.direction
    }

    /// Returns the range of the balance change
    pub fn range(&self) -> &Range {
        &self.range
    }

    /// Returns the impact of this balance shift relative to the ideal range
    pub fn get_impact(&self, ideal: Range) -> BalanceShiftImpact {
        if self.direction == BalanceDirection::Neutral {
            return BalanceShiftImpact::Neutral;
        }

        let is_below_ideal = self.range.end <= ideal.start;
        let is_above_ideal = self.range.start >= ideal.end;
        let is_ideal_zone = self.range.start == ideal.start && self.range.end == ideal.end;

        if is_ideal_zone {
            return BalanceShiftImpact::Neutral;
        }

        match self.direction {
            BalanceDirection::Increasing => {
                if is_below_ideal {
                    BalanceShiftImpact::Rebalance
                } else if is_above_ideal {
                    BalanceShiftImpact::Debalance
                } else {
                    BalanceShiftImpact::Neutral
                }
            }
            BalanceDirection::Decreasing => {
                if is_above_ideal {
                    BalanceShiftImpact::Rebalance
                } else if is_below_ideal {
                    BalanceShiftImpact::Debalance
                } else {
                    BalanceShiftImpact::Neutral
                }
            }
            BalanceDirection::Neutral => BalanceShiftImpact::Neutral,
        }
    }
}

pub struct AdjustmentParams {
    pub ideal: Range,
    pub critical: Range,
    pub limit: f64,
    pub adjustment_rate_strained: f64,
    pub adjustment_rate_critical: f64,
}

impl AdjustmentParams {
    pub fn zones(&self) -> [Zone; 5] {
        // critical low: [0, critical.start) - highest incentive to move out
        let critical_low = Zone::new(0.0, self.critical.start, self.adjustment_rate_critical);

        // strained low: [critical.start, ideal.start) - moderate incentive to move up
        let strained_low = Zone::new(
            self.critical.start,
            self.ideal.start,
            self.adjustment_rate_strained,
        );

        // ideal zone: [ideal.start, ideal.end] - neutral, no fees or incentives
        let ideal = Zone::new(self.ideal.start, self.ideal.end, 0.0);

        // strained high: (ideal.end, critical.end] - moderate incentive to move down
        let strained_high = Zone::new(
            self.ideal.end,
            self.critical.end,
            self.adjustment_rate_strained,
        );

        // critical high: (critical.end, limit] - highest incentive to move out
        let critical_high = Zone::new(self.critical.end, self.limit, self.adjustment_rate_critical);

        [
            critical_low,
            strained_low,
            ideal,
            strained_high,
            critical_high,
        ]
    }
}

pub struct Zone {
    range: Range,
    adjustment_rate: f64,
}

impl Zone {
    pub fn new(start: f64, end: f64, adjustment_rate: f64) -> Self {
        Self {
            range: Range::new(start, end).unwrap(),
            adjustment_rate,
        }
    }

    pub fn compute_adjustment_rate(&self, balance_shift: &BalanceShift, ideal: Range) -> f64 {
        let overlap = self.range.get_overlap(balance_shift.range().clone());

        if overlap.end <= overlap.start {
            return 0.0;
        }

        let direction = balance_shift.get_impact(ideal);
        let segment_length = overlap.end - overlap.start;
        direction.as_f64() * self.adjustment_rate * segment_length
    }
}

/// Compute fee or incentive adjustment for a single asset's balance movement.
///
/// This function calculates the incentive/rebate (if positive) or fee (if negative)
/// for a swap that moves an asset's balance from balance to balance_new. The goal is to
/// encourage movements toward the ideal balance range [ideal.start, ideal.end] and
/// discourage movements away from it.
///
/// # Arguments
/// * `balance` - The initial balance
/// * `balance_new` - The new balance
/// * `balance_total` - The total balance of the asset
/// * `params` - The adjustment parameters
///
/// # Returns
/// * `Result<f64, Error>` - The adjustment value or an error if the range is invalid
pub fn compute_adjustment_value(
    balance: f64,
    balance_new: f64,
    balance_total: f64,
    params: AdjustmentParams,
) -> Result<f64, Error> {
    let balance_shift = BalanceShift::new(balance, balance_new)?;
    let adjustment = params.zones().iter().fold(0.0, |acc, zone| {
        acc + zone.compute_adjustment_rate(&balance_shift, params.ideal)
    });

    Ok(adjustment * balance_total)
}
