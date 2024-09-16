use cosmwasm_std::{ensure, Decimal, Decimal256, SignedDecimal256, Uint128};

use crate::TransmuterMathError;

/// Calculating impact factor component
///
/// Considering change of balance of asset $i$, fee/incentive impact factor component $\gamma_i$ is
///
/// $$
/// \gamma_i =\left[C(b)\right]_{b_i}^{b'_i}
/// $$
///
/// $$
/// \gamma_i = C(b'_i) - C(b_i)
/// $$
///
/// where cumulative component $C(b)$ is
///
/// $$
/// C(b) =
/// \begin{cases}
///       \left(\frac{b - \phi_l}{\phi_l}\right)^2 & \text{if } 0 \leq b \lt \phi_l \\
///       0 & \text{if } \phi_l \leq b \leq \phi_u \\
///       \left(\frac{b - \phi_u}{\delta - \phi_u}\right)^2 & \text{if } \phi_u \lt b \leq \delta
///    \end{cases}
/// $$
pub fn calculate_cumulative_impact_factor_component(
    normalized_balance: Decimal,
    ideal_balance_lower_bound: Decimal,
    ideal_balance_upper_bound: Decimal,
    upper_limit: Decimal,
) -> Result<Decimal, TransmuterMathError> {
    // There might a case where $\delta$ dynamically moved to lower than $\phi_u$ (due to change limiter) uses
    // $min(\delta, \phi_u)$ instead of $\phi_u$
    // and $min(\delta, \phi_u, \phi_l)$ instead of $\phi_l$.
    let ideal_balance_upper_bound = ideal_balance_upper_bound.min(upper_limit);
    let ideal_balance_lower_bound = ideal_balance_lower_bound.min(ideal_balance_upper_bound);

    // Calculate the cumulative impact factor component
    let cumulative = if normalized_balance < ideal_balance_lower_bound {
        ideal_balance_lower_bound // phi_l
            .checked_sub(normalized_balance)? // - b
            .checked_div(ideal_balance_lower_bound)? // / phi_l
            .checked_pow(2)? // ^2
    } else if normalized_balance > ideal_balance_upper_bound {
        normalized_balance // b
            .checked_sub(ideal_balance_upper_bound)? // - phi_u
            // delta - phi_u will never be 0 as this case requires b > phi_u,
            // delta - phi_u = 0 then delta = phi_u
            // since b > delta is restricted by limiter, and delta <= phi_u, this will never happen
            .checked_div(upper_limit.checked_sub(ideal_balance_upper_bound)?)? // / delta - phi_u
            .checked_pow(2)? // ^2
    } else {
        // within ideal balance
        Decimal::zero()
    };

    Ok(cumulative)
}

#[derive(Debug, PartialEq, Eq)]
pub struct ImpactFactorParamGroup {
    prev_normalized_balance: Decimal,
    update_normalized_balance: Decimal,
    ideal_balance_lower_bound: Decimal,
    ideal_balance_upper_bound: Decimal,
    upper_limit: Decimal,
}

impl ImpactFactorParamGroup {
    pub fn new(
        prev_normalized_balance: Decimal,
        update_normalized_balance: Decimal,
        ideal_balance_lower_bound: Decimal,
        ideal_balance_upper_bound: Decimal,
        upper_limit: Decimal,
    ) -> Result<Self, TransmuterMathError> {
        // Check if the input parameters are within the valid range [0, 1]
        ensure!(
            ideal_balance_lower_bound <= Decimal::one(),
            TransmuterMathError::OutOfNormalizedRange {
                var_name: "ideal_balance_lower_bound".to_string()
            }
        );
        ensure!(
            ideal_balance_upper_bound <= Decimal::one(),
            TransmuterMathError::OutOfNormalizedRange {
                var_name: "ideal_balance_upper_bound".to_string()
            }
        );
        ensure!(
            upper_limit <= Decimal::one(),
            TransmuterMathError::OutOfNormalizedRange {
                var_name: "upper_limit".to_string()
            }
        );

        // Check if the normalized balance exceeds the upper limit
        ensure!(
            prev_normalized_balance <= upper_limit,
            TransmuterMathError::NormalizedBalanceExceedsUpperLimit
        );

        ensure!(
            update_normalized_balance <= upper_limit,
            TransmuterMathError::NormalizedBalanceExceedsUpperLimit
        );

        Ok(Self {
            prev_normalized_balance,
            update_normalized_balance,
            ideal_balance_lower_bound,
            ideal_balance_upper_bound,
            upper_limit,
        })
    }

    fn has_no_change_in_balance(&self) -> bool {
        self.prev_normalized_balance == self.update_normalized_balance
    }

    fn calculate_impact_factor_component(&self) -> Result<SignedDecimal256, TransmuterMathError> {
        // C(b)
        let c_b = SignedDecimal256::from(calculate_cumulative_impact_factor_component(
            self.prev_normalized_balance,
            self.ideal_balance_lower_bound,
            self.ideal_balance_upper_bound,
            self.upper_limit,
        )?);

        // C(b')
        let c_b_prime = SignedDecimal256::from(calculate_cumulative_impact_factor_component(
            self.update_normalized_balance,
            self.ideal_balance_lower_bound,
            self.ideal_balance_upper_bound,
            self.upper_limit,
        )?);

        // \gamma_i = C(b') - C(b)
        c_b_prime
            .checked_sub(c_b)
            .map_err(TransmuterMathError::OverflowError)
    }
}

pub enum PayoffType {
    Incentive,
    Fee,
}

/// combine all the impact factor components
///
/// $$
/// f = \frac{\Vert\vec{\gamma}\Vert}{\sqrt{n}}
/// $$
///
/// That gives a normalized magnitude of the vector of $n$ dimension into $[0,1]$.
/// The reason why it needs to include all dimensions is because the case that swapping with alloyed asset, which will effect overall composition rather than just 2 assets.
pub fn calculate_impact_factor(
    impact_factor_param_groups: &[ImpactFactorParamGroup],
) -> Result<(PayoffType, Decimal256), TransmuterMathError> {
    let mut cumulative_impact_factor_sqaure = Decimal256::zero();
    let mut impact_factor_component_sum = SignedDecimal256::zero();

    let n = Decimal256::from_atomics(impact_factor_param_groups.len() as u64, 0)?;

    for impact_factor_params in impact_factor_param_groups {
        // optimiztion: if there is no change in balance, the result will be 0 anyway, accumulating 0 has no effect
        if impact_factor_params.has_no_change_in_balance() {
            continue;
        }

        let impact_factor_component = impact_factor_params.calculate_impact_factor_component()?;
        let impact_factor_component_square =
            Decimal256::try_from(impact_factor_component.checked_pow(2)?)?;

        impact_factor_component_sum =
            impact_factor_component_sum.checked_add(impact_factor_component)?;
        cumulative_impact_factor_sqaure =
            cumulative_impact_factor_sqaure.checked_add(impact_factor_component_square)?;
    }

    let payoff_type = if impact_factor_component_sum.is_negative() {
        PayoffType::Incentive
    } else {
        PayoffType::Fee
    };

    let impact_factor = cumulative_impact_factor_sqaure.checked_div(n)?.sqrt();

    Ok((payoff_type, impact_factor))
}

/// Calculate the rebalancing fee
///
/// The fee is calculated as λ * f * amount_in, where:
/// - λ is the fee scaler, λ ∈ (0,1]
/// - f is the impact factor, f ∈ [0,1]
/// - amount_in is the amount being swapped in, normalized by standard normalization factor
pub fn calculate_rebalancing_fee(
    lambda: Decimal,
    impact_factor: Decimal,
    amount_in: Uint128,
) -> Result<Decimal256, TransmuterMathError> {
    ensure!(
        lambda <= Decimal::one(),
        TransmuterMathError::OutOfNormalizedRange {
            var_name: "lambda".to_string(),
        }
    );

    ensure!(
        impact_factor <= Decimal::one(),
        TransmuterMathError::OutOfNormalizedRange {
            var_name: "impact_factor".to_string(),
        }
    );

    let lambda = Decimal256::from(lambda);
    let impact_factor = Decimal256::from(impact_factor);
    let amount_in_dec = Decimal256::from_atomics(amount_in, 0)?;

    lambda
        .checked_mul(impact_factor)?
        .checked_mul(amount_in_dec)
        .map_err(TransmuterMathError::OverflowError)
}

/// Alias for calculate_rebalancing_fee, as it's used to calculate the impact used in incentive calculation
pub fn calculate_rebalancing_impact(
    lambda: Decimal,
    impact_factor: Decimal,
    amount_in: Uint128,
) -> Result<Decimal256, TransmuterMathError> {
    calculate_rebalancing_fee(lambda, impact_factor, amount_in)
}

/// Calculates the rebalancing incentive for a given swap.
///
/// The incentive should be distributed considering the impact factor $f$, amount $a_{in}$ and incentive pool $p$, so the naive model could be just $min(\lambda f a_{in},p)$.
/// But $\lambda$ be updated and largely impact incentive comparing to when fee has been collected.
///
/// So this function does not try to match the fee collected with the same amount in and impact factor, but scales the incentive by only looking at the amount in and impact factor comparing to overall incentive pool.
///
/// $$
/// \text{Incentive} = p \cdot \frac{\lambda fa_{in}}{p + \lambda fa_{in}}
/// $$
///
/// ## Arguments
/// - `impact`: the impact of the swap, calculated as $\lambda f a_{in}$
/// - `incentive_pool`: the remaining incentive pool $p$
///
/// ## Returns
/// - The rebalancing incentive
pub fn calculate_rebalancing_incentive(
    impact: Decimal,
    incentive_pool: Uint128,
) -> Result<Decimal256, TransmuterMathError> {
    if impact > Decimal::one() {
        return Err(TransmuterMathError::OutOfNormalizedRange {
            var_name: "impact".to_string(),
        });
    }

    let impact = Decimal256::from(impact);
    let incentive_pool_dec = Decimal256::from_atomics(incentive_pool, 0)?;
    let impact_by_incentive_pool = impact.checked_mul(incentive_pool_dec)?;
    let extended_incentive_pool = incentive_pool_dec.checked_add(impact)?;

    impact_by_incentive_pool
        .checked_div(extended_incentive_pool)
        .map_err(TransmuterMathError::CheckedFromRatioError)
}

#[cfg(test)]
mod tests {
    use std::{cmp::min, str::FromStr};

    use super::*;
    use cosmwasm_std::Uint256;
    use proptest::prelude::*;
    use rstest::rstest;

    const ONE_DEC_RAW: u128 = 1_000_000_000_000_000_000;

    #[rstest]
    #[case(
        Decimal::percent(100),
        Decimal::percent(100),
        Uint128::MAX,
        Ok(Decimal256::from_atomics(u128::MAX, 0).unwrap())
    )]
    #[case(
        Decimal::percent(100),
        Decimal::percent(100),
        Uint128::zero(),
        Ok(Decimal256::zero())
    )]
    #[case(
        Decimal::percent(50),
        Decimal::percent(50),
        Uint128::from(100u128),
        Ok(Decimal256::from_atomics(25u128, 0).unwrap())
    )]
    #[case(
        Decimal::percent(101),
        Decimal::percent(100),
        Uint128::MAX,
        Err(TransmuterMathError::OutOfNormalizedRange { var_name: "lambda".to_string() })
    )]
    #[case(
        Decimal::percent(100),
        Decimal::percent(101),
        Uint128::MAX,
        Err(TransmuterMathError::OutOfNormalizedRange { var_name: "impact_factor".to_string() })
    )]
    fn test_calculate_rebalancing_fee(
        #[case] lambda: Decimal,
        #[case] impact_factor: Decimal,
        #[case] amount_in: Uint128,
        #[case] expected: Result<Decimal256, TransmuterMathError>,
    ) {
        let actual = calculate_rebalancing_fee(lambda, impact_factor, amount_in);
        assert_eq!(expected, actual);
    }

    proptest! {
        #[test]
        fn test_rebalancing_fee_must_never_exceed_amount_in(
            lambda in 0u128..=ONE_DEC_RAW,
            impact_factor in 0u128..=ONE_DEC_RAW,
            amount_in in 0..=u128::MAX,
        ) {
            let lambda = Decimal::raw(lambda);
            let impact_factor = Decimal::raw(impact_factor);
            let amount_in = Uint128::new(amount_in);

            let actual = calculate_rebalancing_fee(lambda, impact_factor, amount_in).unwrap();
            assert!(actual <= Decimal256::from_atomics(amount_in, 0).unwrap());
        }

        #[test]
        fn test_rebalancing_fee_must_equal_rebalancing_impact(
            lambda in 0u128..=u128::MAX,
            impact_factor in 0u128..=u128::MAX,
            amount_in in 0..=u128::MAX,
        ) {
            let lambda = Decimal::raw(lambda);
            let impact_factor = Decimal::raw(impact_factor);
            let amount_in = Uint128::new(amount_in);

            let fee = calculate_rebalancing_fee(lambda, impact_factor, amount_in);
            let impact = calculate_rebalancing_impact(lambda, impact_factor, amount_in);

            assert_eq!(fee, impact);
        }
    }

    #[rstest]
    #[case(Decimal::one(), Uint128::MAX, Ok(Decimal256::from_ratio(
        Uint256::from(u128::MAX),
        Uint256::from(u128::MAX) + Uint256::from(1u128),
    )))]
    #[case(Decimal::zero(), Uint128::new(1000), Ok(Decimal256::zero()))]
    #[case(Decimal::one(), Uint128::zero(), Ok(Decimal256::zero()))]
    #[case(Decimal::percent(50), Uint128::new(1000), Ok(Decimal256::from_str("0.499750124937531234").unwrap()))]
    #[case(Decimal::percent(100), Uint128::new(1000), Ok(Decimal256::from_str("0.999000999000999").unwrap()))]
    #[case(Decimal::percent(101), Uint128::new(1000), Err(TransmuterMathError::OutOfNormalizedRange { var_name: "impact".to_string() }))]
    fn test_calculate_rebalancing_incentive(
        #[case] impact: Decimal,
        #[case] incentive_pool: Uint128,
        #[case] expected: Result<Decimal256, TransmuterMathError>,
    ) {
        let actual = calculate_rebalancing_incentive(impact, incentive_pool);
        assert_eq!(expected, actual);
    }

    proptest! {
        #[test]
        fn test_rebalancing_incentive_must_less_than_or_equal_to_incentive_pool(
            impact in 0u128..=ONE_DEC_RAW,
            incentive_pool in 0u128..=u128::MAX,
        ) {
            let impact = Decimal::raw(impact);
            let incentive_pool = Uint128::new(incentive_pool);

            let actual = calculate_rebalancing_incentive(impact, incentive_pool).unwrap();
            assert!(actual <= Decimal256::from_atomics(incentive_pool, 0).unwrap());
        }
    }

    #[rstest]
    #[case(
        Decimal::zero(),
        Decimal::percent(40),
        Decimal::percent(50),
        Decimal::percent(60),
        Ok(Decimal::one())
    )]
    #[case(
        Decimal::percent(39),
        Decimal::percent(40),
        Decimal::percent(50),
        Decimal::percent(60),
        Ok(Decimal::from_str("0.000625").unwrap())
    )]
    #[case(
        Decimal::percent(40),
        Decimal::percent(40),
        Decimal::percent(50),
        Decimal::percent(60),
        Ok(Decimal::zero())
    )]
    #[case(
        Decimal::percent(50),
        Decimal::percent(40),
        Decimal::percent(50),
        Decimal::percent(60),
        Ok(Decimal::zero())
    )]
    #[case(
        Decimal::percent(51),
        Decimal::percent(40),
        Decimal::percent(50),
        Decimal::percent(60),
        Ok(Decimal::percent(1))
    )]
    #[case(
        Decimal::percent(60),
        Decimal::percent(40),
        Decimal::percent(50),
        Decimal::percent(60),
        Ok(Decimal::one())
    )]
    #[case(
        Decimal::percent(50),
        Decimal::percent(40),
        Decimal::percent(60),
        Decimal::percent(50),
        Ok(Decimal::zero())
    )]
    #[case(
        Decimal::percent(30),
        Decimal::percent(40),
        Decimal::percent(60),
        Decimal::percent(30),
        Ok(Decimal::zero())
    )]
    #[case(
        Decimal::percent(30),
        Decimal::percent(40),
        Decimal::percent(30),
        Decimal::percent(50),
        Ok(Decimal::zero())
    )]
    fn test_calculate_cumulative_impact_factor_component(
        #[case] normalized_balance: Decimal,
        #[case] ideal_balance_lower_bound: Decimal,
        #[case] ideal_balance_upper_bound: Decimal,
        #[case] upper_limit: Decimal,
        #[case] expected: Result<Decimal, TransmuterMathError>,
    ) {
        let actual = calculate_cumulative_impact_factor_component(
            normalized_balance,
            ideal_balance_lower_bound,
            ideal_balance_upper_bound,
            upper_limit,
        );
        assert_eq!(expected, actual);
    }

    proptest! {
        #[test]
        fn test_calculate_impact_factor_component_must_be_within_0_and_1(
            normalized_balance in 0u128..=ONE_DEC_RAW,
            ideal_balance_lower_bound in 0u128..=ONE_DEC_RAW,
            ideal_balance_upper_bound in 0u128..=ONE_DEC_RAW,
            upper_limit in 0u128..=ONE_DEC_RAW,
        ) {
            prop_assume!(normalized_balance <= upper_limit);

            let normalized_balance = Decimal::raw(normalized_balance);
            let ideal_balance_lower_bound = Decimal::raw(ideal_balance_lower_bound);
            let ideal_balance_upper_bound = Decimal::raw(ideal_balance_upper_bound);
            let upper_limit = Decimal::raw(upper_limit);

            match calculate_cumulative_impact_factor_component(
                normalized_balance,
                ideal_balance_lower_bound,
                ideal_balance_upper_bound,
                upper_limit,
            ) {
              Ok(actual) => assert!(actual <= Decimal::one()),
              Err(e) => panic!("Failed to calculate impact factor component: {:?}", e),
            }
        }

        #[test]
        fn test_impact_factor_zero_within_ideal_bounds(
            normalized_balance in 0u128..=ONE_DEC_RAW,
            ideal_balance_lower_bound_from_normalized_balance in 0u128..=ONE_DEC_RAW,
            ideal_balance_upper_bound_from_normalized_balance in 0u128..=ONE_DEC_RAW,
        ) {
            let ideal_balance_upper_bound = Decimal::raw(min(ONE_DEC_RAW, normalized_balance + ideal_balance_upper_bound_from_normalized_balance));
            let ideal_balance_lower_bound = Decimal::raw(normalized_balance.saturating_sub(ideal_balance_lower_bound_from_normalized_balance));
            let normalized_balance = Decimal::raw(normalized_balance);
            let upper_limit = ideal_balance_upper_bound;
            let actual = calculate_cumulative_impact_factor_component(
                normalized_balance,
                ideal_balance_lower_bound,
                ideal_balance_upper_bound,
                upper_limit,
            ).unwrap();

            assert_eq!(Decimal::zero(), actual);
        }

        #[test]
        fn test_impact_factor_increases_as_balance_decreases_below_lower_bound(
            normalized_balance in 1u128..=ONE_DEC_RAW,
            ideal_balance_lower_bound in 0u128..=ONE_DEC_RAW,
        ) {
            prop_assume!(normalized_balance < ideal_balance_lower_bound);

            let ideal_balance_upper_bound = ideal_balance_lower_bound + 1;
            let upper_limit = ideal_balance_upper_bound + 1;

            let normalized_balance = Decimal::raw(normalized_balance);
            let ideal_balance_lower_bound = Decimal::raw(ideal_balance_lower_bound);
            let ideal_balance_upper_bound = Decimal::raw(ideal_balance_upper_bound);
            let upper_limit = Decimal::raw(upper_limit);
            let epsilon = Decimal::raw(1000u128);

            let c1 = calculate_cumulative_impact_factor_component(
                normalized_balance - epsilon,
                ideal_balance_lower_bound,
                ideal_balance_upper_bound,
                upper_limit,
            ).unwrap();

            let c2 = calculate_cumulative_impact_factor_component(
                normalized_balance,
                ideal_balance_lower_bound,
                ideal_balance_upper_bound,
                upper_limit,
            ).unwrap();

            assert!(c1 > c2, "c1: {c1}, c2: {c2}");
        }

        #[test]
        fn test_impact_factor_increases_above_upper_bound(
            normalized_balance in 0u128..=ONE_DEC_RAW-1,
            ideal_balance_upper_bound in 0u128..=ONE_DEC_RAW,
            upper_limit in 0u128..=ONE_DEC_RAW,
        ) {
            prop_assume!(normalized_balance > ideal_balance_upper_bound);
            prop_assume!(upper_limit > ideal_balance_upper_bound);

            let ideal_balance_lower_bound = ideal_balance_upper_bound - 1;

            let normalized_balance = Decimal::raw(normalized_balance);
            let ideal_balance_lower_bound = Decimal::raw(ideal_balance_lower_bound);
            let ideal_balance_upper_bound = Decimal::raw(ideal_balance_upper_bound);
            let upper_limit = Decimal::raw(upper_limit);
            let epsilon = Decimal::raw(1000u128);

            let c1 = calculate_cumulative_impact_factor_component(
                normalized_balance - epsilon,
                ideal_balance_lower_bound,
                ideal_balance_upper_bound,
                upper_limit,
            ).unwrap();

            let c2 = calculate_cumulative_impact_factor_component(
                normalized_balance,
                ideal_balance_lower_bound,
                ideal_balance_upper_bound,
                upper_limit,
            ).unwrap();

            assert!(c1 < c2, "c1: {c1}, c2: {c2}");
        }
    }
    // - ImpactFactorParamGroup
    #[rstest]
    #[case::valid(Decimal::percent(50), Decimal::percent(60), Decimal::percent(40), Decimal::percent(50), Decimal::percent(60), Ok(ImpactFactorParamGroup{
        prev_normalized_balance,
        update_normalized_balance,
        ideal_balance_lower_bound,
        ideal_balance_upper_bound,
        upper_limit,
    }))]
    #[case::valid(Decimal::zero(), Decimal::percent(10), Decimal::percent(40), Decimal::percent(50), Decimal::percent(60), Ok(ImpactFactorParamGroup{
        prev_normalized_balance,
        update_normalized_balance,
        ideal_balance_lower_bound,
        ideal_balance_upper_bound,
        upper_limit,
    }))]
    #[case::valid(Decimal::percent(99), Decimal::percent(100), Decimal::percent(40), Decimal::percent(50), Decimal::one(), Ok(ImpactFactorParamGroup{
        prev_normalized_balance,
        update_normalized_balance,
        ideal_balance_lower_bound,
        ideal_balance_upper_bound,
        upper_limit,
    }))]
    #[case::invalid(Decimal::percent(50), Decimal::percent(60), Decimal::percent(40), Decimal::percent(50), Decimal::percent(110), Err(TransmuterMathError::OutOfNormalizedRange { var_name: "upper_limit".to_string() }))]
    #[case::invalid(Decimal::percent(50), Decimal::percent(60), Decimal::percent(40), Decimal::percent(110), Decimal::percent(60), Err(TransmuterMathError::OutOfNormalizedRange { var_name: "ideal_balance_upper_bound".to_string() }))]
    #[case::invalid(Decimal::percent(50), Decimal::percent(60), Decimal::percent(110), Decimal::percent(50), Decimal::percent(60), Err(TransmuterMathError::OutOfNormalizedRange { var_name: "ideal_balance_lower_bound".to_string() }))]
    #[case::invalid(
        Decimal::percent(110),
        Decimal::percent(60),
        Decimal::percent(40),
        Decimal::percent(50),
        Decimal::percent(60),
        Err(TransmuterMathError::NormalizedBalanceExceedsUpperLimit)
    )]
    #[case::invalid(
        Decimal::percent(50),
        Decimal::percent(110),
        Decimal::percent(40),
        Decimal::percent(50),
        Decimal::percent(60),
        Err(TransmuterMathError::NormalizedBalanceExceedsUpperLimit)
    )]
    fn test_impact_factor_param_group_new(
        #[case] prev_normalized_balance: Decimal,
        #[case] update_normalized_balance: Decimal,
        #[case] ideal_balance_lower_bound: Decimal,
        #[case] ideal_balance_upper_bound: Decimal,
        #[case] upper_limit: Decimal,
        #[case] expected_result: Result<ImpactFactorParamGroup, TransmuterMathError>,
    ) {
        let result = ImpactFactorParamGroup::new(
            prev_normalized_balance,
            update_normalized_balance,
            ideal_balance_lower_bound,
            ideal_balance_upper_bound,
            upper_limit,
        );
        assert_eq!(result, expected_result);
    }

    proptest! {
        #[test]
        fn test_has_no_change_in_balance(
            normalized_balance in 0..=ONE_DEC_RAW,
            update_normalized_balance in 0..=ONE_DEC_RAW,
        ) {
            prop_assume!(normalized_balance != update_normalized_balance);
            let normalized_balance = Decimal::raw(normalized_balance);
            let update_normalized_balance = Decimal::raw(update_normalized_balance);

            // all bounds are set to 100% as it's unrelevant to the test
            let ideal_balance_lower_bound = Decimal::percent(100);
            let ideal_balance_upper_bound = Decimal::percent(100);
            let upper_limit = Decimal::percent(100);


            if let Ok(group) = ImpactFactorParamGroup::new(
                normalized_balance,
                normalized_balance,
                ideal_balance_lower_bound,
                ideal_balance_upper_bound,
                upper_limit,
            ) {
                assert!(group.has_no_change_in_balance());
            }

            if let Ok(group) = ImpactFactorParamGroup::new(
                normalized_balance,
                update_normalized_balance,
                ideal_balance_lower_bound,
                ideal_balance_upper_bound,
                upper_limit,
            ) {
                assert!(!group.has_no_change_in_balance());
            }
        }
    }

    #[rstest]
    #[case::no_change_in_balance(
        Decimal::percent(50),
        Decimal::percent(50),
        Decimal::percent(40),
        Decimal::percent(60),
        Decimal::one(),
        Ok(SignedDecimal256::zero())
    )]
    #[case::move_within_ideal_range(
        Decimal::percent(45),
        Decimal::percent(55),
        Decimal::percent(40),
        Decimal::percent(60),
        Decimal::one(),
        Ok(SignedDecimal256::zero())
    )]
    #[case::move_away_from_ideal_range(
        Decimal::percent(55),
        Decimal::percent(65),
        Decimal::percent(40),
        Decimal::percent(60),
        Decimal::one(),
        Ok(SignedDecimal256::from_str("0.015625").unwrap())
    )]
    #[case::move_towards_ideal_range(
        Decimal::percent(55),
        Decimal::percent(45),
        Decimal::percent(40),
        Decimal::percent(50),
        Decimal::one(),
        Ok(SignedDecimal256::from_str("-0.01").unwrap())
    )]
    #[case::cross_ideal_range_negative(
        Decimal::percent(30),
        Decimal::percent(65),
        Decimal::percent(40),
        Decimal::percent(60),
        Decimal::one(),
        Ok(SignedDecimal256::from_str("-0.046875").unwrap())
    )]
    #[case::cross_ideal_range_positive(
        Decimal::percent(70), 
        Decimal::percent(25),   
        Decimal::percent(40),   
        Decimal::percent(60),
        Decimal::one(),
        Ok(SignedDecimal256::from_str("0.078125").unwrap())
    )]
    // #[case::precision_issue(
    //     Decimal::from_str("0.600000000000000001").unwrap(), 
    //     Decimal::from_str("0.600000000000000002").unwrap(),  
    //     Decimal::percent(40),                               
    //     Decimal::percent(60),
    //     Decimal::one(),
    //     Ok(SignedDecimal256::from_str("-0.000000000000000001").unwrap())
    // )]
    fn test_calculate_impact_factor_component(
        #[case] prev_normalized_balance: Decimal,
        #[case] update_normalized_balance: Decimal,
        #[case] ideal_balance_lower_bound: Decimal,
        #[case] ideal_balance_upper_bound: Decimal,
        #[case] upper_limit: Decimal,
        #[case] expected: Result<SignedDecimal256, TransmuterMathError>,
    ) {
        let group = ImpactFactorParamGroup::new(
            prev_normalized_balance,
            update_normalized_balance,
            ideal_balance_lower_bound,
            ideal_balance_upper_bound,
            upper_limit,
        )
        .unwrap();

        let result = group.calculate_impact_factor_component();
        assert_eq!(result, expected);
    }
}
