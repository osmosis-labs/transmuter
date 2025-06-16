use cosmwasm_std::Decimal;

use crate::TransmuterMathError as Error;

/// Represents a bound in a range, which can be either inclusive or exclusive
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Bound {
    /// Inclusive bound - the value is included in the range
    Inclusive(Decimal),
    /// Exclusive bound - the value is not included in the range
    Exclusive(Decimal),
}

impl Bound {
    /// Returns the underlying decimal value
    pub fn value(&self) -> Decimal {
        match self {
            Self::Inclusive(v) | Self::Exclusive(v) => *v,
        }
    }

    /// Returns true if the given value is within this bound
    pub fn contains(&self, value: Decimal) -> bool {
        match self {
            Self::Inclusive(bound) => value <= *bound,
            Self::Exclusive(bound) => value < *bound,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct Range {
    start: Bound,
    end: Bound,
}

impl Range {
    pub fn new(start: Bound, end: Bound) -> Result<Self, Error> {
        if start.value() > end.value() {
            return Err(Error::InvalidRange(start.value(), end.value()));
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
        let segment_start = if self.start.value() > other.start.value() {
            self.start
        } else if other.start.value() > self.start.value() {
            other.start
        } else {
            // If values are equal, prefer inclusive bound
            match (self.start, other.start) {
                (Bound::Inclusive(v), _) | (_, Bound::Inclusive(v)) => Bound::Inclusive(v),
                _ => Bound::Exclusive(self.start.value()),
            }
        };

        let segment_end = if self.end.value() < other.end.value() {
            self.end
        } else if other.end.value() < self.end.value() {
            other.end
        } else {
            // If values are equal, prefer inclusive bound
            match (self.end, other.end) {
                (Bound::Inclusive(v), _) | (_, Bound::Inclusive(v)) => Bound::Inclusive(v),
                _ => Bound::Exclusive(self.end.value()),
            }
        };

        Range {
            start: segment_start,
            end: segment_end,
        }
    }

    /// Returns true if the range contains the given value
    pub fn contains(&self, value: Decimal) -> bool {
        self.start.contains(value) && self.end.contains(value)
    }

    /// Returns the start bound of the range
    pub fn start(&self) -> Bound {
        self.start
    }

    /// Returns the end bound of the range
    pub fn end(&self) -> Bound {
        self.end
    }
}
