use cosmwasm_std::{Decimal, Uint128, Uint64};

use crate::errors::TransmuterMathError;

pub(super) fn elapsed_time(
    from: impl Into<Uint64>,
    to: impl Into<Uint64>,
) -> Result<Uint64, TransmuterMathError> {
    let from = from.into();
    let to = to.into();

    to.checked_sub(from).map_err(Into::into)
}

pub(super) fn forward(from: impl Into<Uint64>, by: impl Into<Uint64>) -> Result<Uint64, TransmuterMathError> {
    let from = from.into();
    let by = by.into();

    from.checked_add(by).map_err(Into::into)
}

pub(super) fn backward(from: impl Into<Uint64>, by: impl Into<Uint64>) -> Result<Uint64, TransmuterMathError> {
    let from = from.into();
    let by = by.into();

    from.checked_sub(by).map_err(Into::into)
}

pub(super) fn from_uint(uint: impl Into<Uint128>) -> Decimal {
    Decimal::from_ratio(uint.into(), 1u128)
}
