use cosmwasm_std::{
    ensure, CheckedFromRatioError, CheckedMultiplyRatioError, Decimal, DivideByZeroError, Uint128,
};
use thiserror::Error;

#[derive(Error, Debug, PartialEq)]
pub enum MathError {
    #[error("{0}")]
    CheckedMultiplyRatioError(#[from] CheckedMultiplyRatioError),

    #[error("{0}")]
    CheckedFromRatioError(#[from] CheckedFromRatioError),

    #[error("{0}")]
    DivideByZeroError(#[from] DivideByZeroError),

    #[error("Rescaling parameter is not divisible: rescale {n} by {numerator}/{denominator}")]
    NonDivisibleRescaleError {
        n: Uint128,
        numerator: Uint128,
        denominator: Uint128,
    },

    #[error("Can't rescale to zero")]
    RescaleToZeroError {},

    #[error("Input can't be zero")]
    ZeroInput {},

    #[error("Empty iterator")]
    EmptyIterator {},
}

type MathResult<T> = Result<T, MathError>;

pub fn lcm_from_iter(iter: impl IntoIterator<Item = Uint128>) -> MathResult<Uint128> {
    let mut iter = iter.into_iter().peekable();
    ensure!(iter.peek().is_some(), MathError::EmptyIterator {});

    iter.try_fold(Uint128::one(), lcm)
}

fn lcm(n: Uint128, m: Uint128) -> MathResult<Uint128> {
    n.checked_multiply_ratio(m, gcd(n, m)?).map_err(Into::into)
}

fn gcd(mut n: Uint128, mut m: Uint128) -> MathResult<Uint128> {
    ensure!(!n.is_zero(), MathError::ZeroInput {});
    ensure!(!m.is_zero(), MathError::ZeroInput {});

    while !m.is_zero() {
        if m < n {
            std::mem::swap(&mut m, &mut n);
        }
        m %= n;
    }
    Ok(n)
}

pub fn rescale(n: Uint128, numerator: Uint128, denominator: Uint128) -> MathResult<Uint128> {
    // ensure that numerator is not zero
    ensure!(!numerator.is_zero(), MathError::RescaleToZeroError {});

    // ensure that n * numerator is divisible by denominator
    ensure!(
        n.full_mul(numerator)
            .checked_rem(denominator.into())?
            .is_zero(),
        MathError::NonDivisibleRescaleError {
            n,
            numerator,
            denominator,
        }
    );

    n.checked_multiply_ratio(numerator, denominator)
        .map_err(Into::into)
}

/// Calculate the price of the base asset in terms of the quote asset based on the normalized factors
///
/// ```
/// quote_amt / quote_norm_factor = base_amt / base_norm_factor
/// quote_amt = base_amt * quote_norm_factor / base_norm_factor
/// ```
///
/// spot price is how much of the quote asset is needed to buy one unit of the base asset
/// therefore:
///
/// ```
/// spot_price = 1 * quote_norm_factor / base_norm_factor
/// ```
pub fn price(base_norm_factor: Uint128, quote_norm_factor: Uint128) -> MathResult<Decimal> {
    Decimal::checked_from_ratio(quote_norm_factor, base_norm_factor).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    // typical cases
    #[case(1u128, 1u128, Ok(1u128))]
    #[case(2u128, 1u128, Ok(1u128))]
    #[case(2u128, 2u128, Ok(2u128))]
    #[case(4u128, 2u128, Ok(2u128))]
    #[case(2u128, 4u128, Ok(2u128))]
    #[case(6u128, 2u128, Ok(2u128))]
    #[case(6u128, 3u128, Ok(3u128))]
    // prime number
    #[case(7u128, 3u128, Ok(1u128))]
    #[case(7u128, 5u128, Ok(1u128))]
    #[case(7u128, 13u128, Ok(1u128))]
    // big number
    #[case(u128::MAX, 5u128, Ok(5u128))]
    #[case(u128::MAX, u128::MAX, Ok(u128::MAX))]
    #[case(u128::MAX, 5057672949897463733145855, Ok(5057672949897463733145855))]
    // error cases
    #[case(0u128, 1u128, Err(MathError::ZeroInput { }))]
    #[case(1u128, 0u128, Err(MathError::ZeroInput { }))]
    #[case(0u128, 0u128, Err(MathError::ZeroInput { }))]
    fn test_gcd(#[case] n: u128, #[case] m: u128, #[case] expected: MathResult<u128>) {
        assert_eq!(
            gcd(Uint128::from(n), Uint128::from(m)),
            expected.map(Uint128::from)
        );
    }

    #[rstest]
    // typical cases
    #[case(1u128, 1u128, Ok(1u128))]
    #[case(2u128, 2u128, Ok(2u128))]
    #[case(2u128, 3u128, Ok(6u128))]
    #[case(4u128, 2u128, Ok(4u128))]
    #[case(2u128, 4u128, Ok(4u128))]
    #[case(6u128, 2u128, Ok(6u128))]
    #[case(6u128, 3u128, Ok(6u128))]
    // prime number
    #[case(7u128, 3u128, Ok(21u128))]
    #[case(7u128, 5u128, Ok(35u128))]
    #[case(7u128, 13u128, Ok(91u128))]
    // big number
    #[case(u128::MAX, 5u128, Ok(u128::MAX))]
    #[case(u128::MAX, u128::MAX, Ok(u128::MAX))]
    #[case(u128::MAX, 5057672949897463733145855, Ok(u128::MAX))]
    // error cases
    #[case(0u128, 1u128, Err(MathError::ZeroInput { }))]
    #[case(1u128, 0u128, Err(MathError::ZeroInput { }))]
    #[case(0u128, 0u128, Err(MathError::ZeroInput { }))]
    #[case(u128::MAX, u128::MAX - 1, Err(MathError::CheckedMultiplyRatioError(CheckedMultiplyRatioError::Overflow { })))]
    fn test_lcm(#[case] n: u128, #[case] m: u128, #[case] expected: MathResult<u128>) {
        assert_eq!(
            lcm(Uint128::from(n), Uint128::from(m)),
            expected.map(Uint128::from)
        );
    }

    #[rstest]
    // typical cases
    #[case(vec![1u128, 1u128], Ok(1u128))]
    #[case(vec![2u128, 1u128], Ok(2u128))]
    #[case(vec![13u128, 26u128, 12u128], Ok(156u128))]
    // error cases
    #[case(vec![], Err(MathError::EmptyIterator { }))]
    fn test_lcm_from_iter(#[case] iter: Vec<u128>, #[case] expected: MathResult<u128>) {
        assert_eq!(
            lcm_from_iter(iter.into_iter().map(Uint128::from)),
            expected.map(Uint128::from)
        );
    }

    #[rstest]
    #[case(1u128, 1u128, 1u128, Ok(1u128))]
    #[case(5u128, 20u128, 1u128, Ok(100u128))]
    #[case(5u128, 20u128, 2u128, Ok(50u128))]
    #[case(1000u128, 1u128, 10u128, Ok(100u128))]
    #[case(5u128, 20u128, 3u128, Err(MathError::NonDivisibleRescaleError { n: 5u128.into(), numerator: 20u128.into(), denominator: 3u128.into() }))]
    #[case(
        5u128,
        20u128,
        0u128,
        Err(MathError::DivideByZeroError(DivideByZeroError { operand: String::from("100")}))
    )]
    #[case(
        1000u128,
        0u128,
        10u128,
        Err(MathError::RescaleToZeroError { })
    )]

    fn test_rescale(
        #[case] n: u128,
        #[case] numerator: u128,
        #[case] denominator: u128,
        #[case] expected: MathResult<u128>,
    ) {
        assert_eq!(
            rescale(
                Uint128::from(n),
                Uint128::from(numerator),
                Uint128::from(denominator)
            ),
            expected.map(Uint128::from)
        );
    }

    #[rstest]
    #[case(1u128, 1u128, Ok(Decimal::one()))]
    #[case(10u128, 20u128, Ok(Decimal::from_ratio(2u128, 1u128)))]
    #[case(100u128, 200u128, Ok(Decimal::from_ratio(2u128, 1u128)))]
    #[case(
        10_000_000_000_000_000u128,
        1_000_000_000_000_000_000u128,
        Ok(Decimal::from_ratio(100u128, 1u128))
    )]
    #[case(
        1_000_000_000_000_000_000u128,
        10_000_000_000_000_000u128,
        Ok(Decimal::from_ratio(1u128, 100u128))
    )]
    #[case(100u128, 0u128, Ok(Decimal::zero()))]
    #[case(
        0u128,
        100u128,
        Err(MathError::CheckedFromRatioError(CheckedFromRatioError::DivideByZero))
    )]
    fn test_price(
        #[case] base_norm_factor: u128,
        #[case] quote_norm_factor: u128,
        #[case] expected: MathResult<Decimal>,
    ) {
        assert_eq!(
            price(
                Uint128::from(base_norm_factor),
                Uint128::from(quote_norm_factor)
            ),
            expected
        );
    }
}
