use cosmwasm_std::{ensure, CheckedMultiplyRatioError, Uint128};
use thiserror::Error;

#[derive(Error, Debug, PartialEq)]
pub enum MathError {
    #[error("{0}")]
    CheckedMultiplyRatioError(#[from] CheckedMultiplyRatioError),

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
}
