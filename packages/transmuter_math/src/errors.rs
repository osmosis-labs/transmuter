use cosmwasm_std::{CheckedFromRatioError, DivideByZeroError, OverflowError, StdError};

use crate::rebalancing::{adjustment_params::AdjustmentParamsError, range::Bound};

#[derive(thiserror::Error, Debug, PartialEq)]
pub enum TransmuterMathError {
    /// Time invariant error, this should never happen
    #[error("Time must be monotonically increasing")]
    NonMonotonicTime,

    #[error("Moving average is undefined due to zero elapsed time since limiter started tracking")]
    UndefinedMovingAverage,

    #[error("Missing data points to calculate moving average")]
    MissingDataPoints,

    #[error("{0}")]
    OverflowError(#[from] OverflowError),

    #[error("{0}")]
    DivideByZeroError(#[from] DivideByZeroError),

    #[error("{0}")]
    CheckedFromRatioError(#[from] CheckedFromRatioError),

    #[error("Invalid range: start={0}, end={1}")]
    InvalidRange(Bound, Bound),

    #[error("{0}")]
    AdjustmentParamsError(#[from] AdjustmentParamsError),

    #[error("{0}")]
    StdError(#[from] StdError),
}
