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
    pub fn get_overlap(&self, other: Range) -> Option<Range> {
        let overlap_start = if self.start.value() > other.start.value() {
            self.start
        } else if other.start.value() > self.start.value() {
            other.start
        } else {
            // If values are equal, prefer exclusive bound
            match (self.start, other.start) {
                (Bound::Exclusive(v), _) | (_, Bound::Exclusive(v)) => Bound::Exclusive(v),
                _ => Bound::Inclusive(self.start.value()),
            }
        };

        let overlap_end = if self.end.value() < other.end.value() {
            self.end
        } else if other.end.value() < self.end.value() {
            other.end
        } else {
            // If values are equal, prefer exclusive bound
            match (self.end, other.end) {
                (Bound::Exclusive(v), _) | (_, Bound::Exclusive(v)) => Bound::Exclusive(v),
                _ => Bound::Inclusive(self.end.value()),
            }
        };

        // If the overlap starts after it ends, return None
        if overlap_start.value() > overlap_end.value() {
            return None;
        }

        if overlap_start.value() == overlap_end.value() {
            match (overlap_start, overlap_end) {
                (Bound::Exclusive(_), _) | (_, Bound::Exclusive(_)) => return None,
                (Bound::Inclusive(_), Bound::Inclusive(_)) => return Some(Range {
                    start: overlap_start,
                    end: overlap_end,
                }),
            }
        }

        Some(Range {
            start: overlap_start,
            end: overlap_end,
        })
    }

    /// Returns true if the range contains the given value
    pub fn contains(&self, value: Decimal) -> bool {
        match (self.start, self.end) {
            (Bound::Inclusive(start), Bound::Inclusive(end)) => value >= start && value <= end,
            (Bound::Inclusive(start), Bound::Exclusive(end)) => value >= start && value < end,
            (Bound::Exclusive(start), Bound::Inclusive(end)) => value > start && value <= end,
            (Bound::Exclusive(start), Bound::Exclusive(end)) => value > start && value < end,
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(Bound::Inclusive(Decimal::percent(50)), Decimal::percent(50))]
    #[case(Bound::Exclusive(Decimal::percent(50)), Decimal::percent(50))]
    fn test_bound_value(#[case] bound: Bound, #[case] expected: Decimal) {
        assert_eq!(bound.value(), expected);
    }

    #[rstest]
    #[case(
        Bound::Inclusive(Decimal::percent(10)),
        Bound::Inclusive(Decimal::percent(90)),
        true
    )]
    #[case(
        Bound::Inclusive(Decimal::percent(90)),
        Bound::Inclusive(Decimal::percent(10)),
        false
    )]
    #[case(
        Bound::Exclusive(Decimal::percent(10)),
        Bound::Exclusive(Decimal::percent(90)),
        true
    )]
    #[case(
        Bound::Exclusive(Decimal::percent(50)),
        Bound::Exclusive(Decimal::percent(50)),
        true
    )]
    #[case(
        Bound::Exclusive(Decimal::percent(90)),
        Bound::Exclusive(Decimal::percent(10)),
        false
    )]
    fn test_range_new(#[case] start: Bound, #[case] end: Bound, #[case] should_succeed: bool) {
        let result = Range::new(start, end);
        assert_eq!(result.is_ok(), should_succeed);
        if !should_succeed {
            assert!(matches!(result, Err(Error::InvalidRange(_, _))));
        }
    }

    #[rstest]
    #[case(
        Range::new(
            Bound::Inclusive(Decimal::percent(10)),
            Bound::Inclusive(Decimal::percent(90))
        ).unwrap(),
        Decimal::percent(50),
        true
    )]
    #[case(
        Range::new(
            Bound::Inclusive(Decimal::percent(10)),
            Bound::Inclusive(Decimal::percent(90))
        ).unwrap(),
        Decimal::percent(10),
        true
    )]
    #[case(
        Range::new(
            Bound::Inclusive(Decimal::percent(10)),
            Bound::Inclusive(Decimal::percent(90))
        ).unwrap(),
        Decimal::percent(90),
        true
    )]
    #[case(
        Range::new(
            Bound::Inclusive(Decimal::percent(10)),
            Bound::Inclusive(Decimal::percent(90))
        ).unwrap(),
        Decimal::percent(5),
        false
    )]
    #[case(
        Range::new(
            Bound::Inclusive(Decimal::percent(10)),
            Bound::Inclusive(Decimal::percent(90))
        ).unwrap(),
        Decimal::percent(95),
        false
    )]
    #[case(
        Range::new(
            Bound::Exclusive(Decimal::percent(10)),
            Bound::Exclusive(Decimal::percent(90))
        ).unwrap(),
        Decimal::percent(10),
        false
    )]
    #[case(
        Range::new(
            Bound::Exclusive(Decimal::percent(10)),
            Bound::Exclusive(Decimal::percent(90))
        ).unwrap(),
        Decimal::percent(90),
        false
    )]
    #[case(
        Range::new(
            Bound::Exclusive(Decimal::percent(10)),
            Bound::Inclusive(Decimal::percent(90))
        ).unwrap(),
        Decimal::percent(10),
        false
    )]
    #[case(
        Range::new(
            Bound::Exclusive(Decimal::percent(10)), 
            Bound::Inclusive(Decimal::percent(90))
        ).unwrap(),
        Decimal::percent(90),
        true
    )]
    #[case(
        Range::new(
            Bound::Inclusive(Decimal::percent(10)),
            Bound::Exclusive(Decimal::percent(90))
        ).unwrap(),
        Decimal::percent(10),
        true
    )]
    #[case(
        Range::new(
            Bound::Inclusive(Decimal::percent(10)),
            Bound::Exclusive(Decimal::percent(90))
        ).unwrap(),
        Decimal::percent(90),
        false
    )]
    fn test_range_contains(#[case] range: Range, #[case] value: Decimal, #[case] expected: bool) {
        assert_eq!(range.contains(value), expected);
    }

    #[rstest]
    #[case(
        Range::new(
            Bound::Inclusive(Decimal::percent(10)),
            Bound::Inclusive(Decimal::percent(90))
        ).unwrap(),
        Range::new(
            Bound::Inclusive(Decimal::percent(20)),
            Bound::Inclusive(Decimal::percent(80))
        ).unwrap(),
        Range::new(
            Bound::Inclusive(Decimal::percent(20)),
            Bound::Inclusive(Decimal::percent(80))
        ).ok()
    )]
    #[case(
        Range::new(
            Bound::Inclusive(Decimal::percent(20)),
            Bound::Inclusive(Decimal::percent(80))
        ).unwrap(),
        Range::new(
            Bound::Inclusive(Decimal::percent(10)),
            Bound::Inclusive(Decimal::percent(90))
        ).unwrap(),
        Range::new(
            Bound::Inclusive(Decimal::percent(20)),
            Bound::Inclusive(Decimal::percent(80))
        ).ok()
    )]
    #[case(
        Range::new(
            Bound::Inclusive(Decimal::percent(10)),
            Bound::Inclusive(Decimal::percent(30))
        ).unwrap(),
        Range::new(
            Bound::Inclusive(Decimal::percent(70)),
            Bound::Inclusive(Decimal::percent(90))
        ).unwrap(),
        None
    )]
    #[case(
        Range::new(
            Bound::Inclusive(Decimal::percent(10)),
            Bound::Inclusive(Decimal::percent(30))
        ).unwrap(),
        Range::new(
            Bound::Inclusive(Decimal::percent(30)),
            Bound::Inclusive(Decimal::percent(40))
        ).unwrap(),
        Range::new(
            Bound::Inclusive(Decimal::percent(30)),
            Bound::Inclusive(Decimal::percent(30))
        ).ok()
    )]
    #[case(
        Range::new(
            Bound::Inclusive(Decimal::percent(10)),
            Bound::Exclusive(Decimal::percent(30))
        ).unwrap(),
        Range::new(
            Bound::Inclusive(Decimal::percent(30)),
            Bound::Inclusive(Decimal::percent(40))
        ).unwrap(),
        None
    )]
    #[case(
        Range::new(
            Bound::Inclusive(Decimal::percent(10)),
            Bound::Inclusive(Decimal::percent(30)),
        ).unwrap(),
        Range::new(
            Bound::Exclusive(Decimal::percent(30)),
            Bound::Inclusive(Decimal::percent(40))
        ).unwrap(),
        None
    )]
    fn test_range_get_overlap(
        #[case] range1: Range,
        #[case] range2: Range,
        #[case] expected: Option<Range>,
    ) {
        let overlap = range1.get_overlap(range2);

        if let Some(overlap) = overlap {
            let expected = expected.expect("overlap is Some");
            assert_eq!(overlap.start(), expected.start(), "start");
            assert_eq!(overlap.end(), expected.end(), "end");
        } else {
            assert!(expected.is_none(), "expected None");
        }
    }
}
