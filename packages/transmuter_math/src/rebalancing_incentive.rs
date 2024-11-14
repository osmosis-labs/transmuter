use cosmwasm_std::{ensure, Decimal, Decimal256, SignedDecimal256, Uint128, Uint256};

use crate::TransmuterMathError;

#[derive(Debug, PartialEq, Eq)]
pub enum ImpactFactor {
    Incentive(Decimal),
    Fee(Decimal),
    None,
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
) -> Result<ImpactFactor, TransmuterMathError> {
    if impact_factor_param_groups.is_empty() {
        return Ok(ImpactFactor::None);
    }

    let mut cumulative_impact_factor_sqaure = Decimal256::zero();
    let mut impact_factor_component_sum = SignedDecimal256::zero();

    // accumulated impact_factor_component_square rounded to smallest possible Decimal256
    // when impact_factor_component is smaller then 10^-9 to prevent fee exploitation
    let mut lost_rounded_impact_factor_component_square_sum = Decimal256::zero();

    let n = Decimal256::from_atomics(impact_factor_param_groups.len() as u64, 0)?;

    for impact_factor_params in impact_factor_param_groups {
        // optimiztion: if there is no change in balance, the result will be 0 anyway, accumulating 0 has no effect
        if impact_factor_params.has_no_change_in_balance() {
            continue;
        }
        
        let impact_factor_component = impact_factor_params.calculate_impact_factor_component()?;        

        // when impact_factor_component<= 10^-9, the result after squaring will be 0, then
        // - if total is counted as incentive, there will be no incentive and it's fine 
        //   since it's neglectible and will not overincentivize and drain incentive pool
        // - if total is counted as fee, it could be exploited by 
        //   making swap with small impact_factor_component over and over again to avoid being counted as fee
        let impact_factor_component_dec = impact_factor_component.abs_diff(SignedDecimal256::zero());
        if impact_factor_component_dec <= Decimal256::raw(1_000_000_000u128) {
            lost_rounded_impact_factor_component_square_sum = lost_rounded_impact_factor_component_square_sum.checked_add(Decimal256::raw(1u128))?;
        }
        
        let impact_factor_component_square = impact_factor_component_dec.checked_pow(2)?;

        impact_factor_component_sum =
            impact_factor_component_sum.checked_add(impact_factor_component)?;
        cumulative_impact_factor_sqaure =
            cumulative_impact_factor_sqaure.checked_add(impact_factor_component_square)?;
    }

    if impact_factor_component_sum.is_zero() {
        Ok(ImpactFactor::None)
    } else if impact_factor_component_sum.is_negative() {
        Ok(ImpactFactor::Incentive(
            cumulative_impact_factor_sqaure
                .checked_div(n)?
                .sqrt()
                .try_into()? // safe to convert to Decimal as it's always less than 1
        ))
    } else {
        // add back lost impact_factor_component_square_sum before normalizing
        Ok(ImpactFactor::Fee(
            cumulative_impact_factor_sqaure
                .checked_add(lost_rounded_impact_factor_component_square_sum)?
                .checked_div(n)?
                .sqrt()
                .try_into()? // safe to convert to Decimal as it's always less than 1   
        ))
    }
}

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
///       \left(\frac{\phi_l - b}{\phi_l}\right)^2 & \text{if } 0 \leq b \lt \phi_l \\
///       0 & \text{if } \phi_l \leq b \leq \phi_u \\
///       \left(\frac{b - \phi_u}{\delta - \phi_u}\right)^2 & \text{if } \phi_u \lt b \leq \delta
///    \end{cases}
/// $$
///
/// This function returns √C(b) to delay precision loss handling
pub fn calculate_sqrt_cumulative_impact_factor_component(
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
    } else if normalized_balance > ideal_balance_upper_bound {
        normalized_balance // b
            .checked_sub(ideal_balance_upper_bound)? // - phi_u
            // delta - phi_u will never be 0 as this case requires b > phi_u,
            // delta - phi_u = 0 then delta = phi_u
            // since b > delta is restricted by limiter, and delta <= phi_u, this will never happen
            .checked_div(upper_limit.checked_sub(ideal_balance_upper_bound)?)? // / delta - phi_u
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
        // √C(b)
        let sqrt_c_b = SignedDecimal256::from(calculate_sqrt_cumulative_impact_factor_component(
            self.prev_normalized_balance,
            self.ideal_balance_lower_bound,
            self.ideal_balance_upper_bound,
            self.upper_limit,
        )?);

        // √C(b')
        let sqrt_c_b_prime = SignedDecimal256::from(calculate_sqrt_cumulative_impact_factor_component(
            self.update_normalized_balance,
            self.ideal_balance_lower_bound,
            self.ideal_balance_upper_bound,
            self.upper_limit,
        )?);


        // \gamma_i = C(b') - C(b)
        let c_b_prime = sqrt_c_b_prime.checked_pow(2)?;
        let c_b = sqrt_c_b.checked_pow(2)?;
        let gamma_i = c_b_prime.checked_sub(c_b)?;

        // gamma_i = 0 might be due to precision loss after squaring
        // if C(b') - C(b) > 0  is counted as fee factor
        // round to most precise positive number representable in SignedDecimal256
        //
        // C(b') - C(b) < 0 case is not handled here, as it will be counted as incentive factor
        // keep it as 0 to prevent overincentive
        if gamma_i.is_zero() && sqrt_c_b_prime > sqrt_c_b {
            return Ok(SignedDecimal256::raw(1));
        }

        Ok(gamma_i)
    }
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


/// The incentive should be distributed considering the impact factor `f`, amount `a_in` and remaining pool incentive `p`, so the naive model could be just:
///
/// ```text
/// Incentive = min(λ * f * a_in, p)
/// ```
///
/// But `λ` can be updated and largely impact incentive comparing to when fee has been collected. To account for `λ` transition, we track `λ_hist` and `p_hist` which is an incentive pool collected with `λ_hist`.
///
/// If there is more `λ` update when the `p_hist` hasn't run out, update the following:
///
/// ```text
/// λ_hist := (λ_hist * p_hist + λ * p) / (p_hist + p)
/// p_hist := p_hist + p
/// ```
///
/// So that we keep remembering past `λ` without storing all of the history.
///
/// To calculate the incentive now we need incentive portion that derived from `λ_hist`:
///
/// ```text
/// Incentive_hist = min(λ_hist * f * a_in, p_hist)
/// ```
///
/// Then use the remaining portion, adjusted to current `λ`, then capped with current `p`:
///
/// ```text
/// Incentive_curr = min((λ_hist * f * a_in - Incentive_hist) * (λ / λ_hist), p)
/// ```
///
/// This has the property that:
///
/// If `λ_hist * f * a_in < p_hist` then `Incentive_curr = 0` (meaning `p_hist` hasn't run out).
///
/// If `p_hist = 0` then `Incentive_curr = min(λ * f * a_in, p)`
///
/// The final incentive function is:
///
/// ```text
/// Incentive = Incentive_hist + Incentive_curr
/// ```
pub fn calculate_rebalancing_incentive(
    impact_factor: Decimal,
    amount_in: Uint128,
    lambda_hist: Decimal,
    lambda_curr: Decimal,
    incentive_pool_hist: Uint256,
    incentive_pool_curr: Uint256,
) -> Result<Uint128, TransmuterMathError> {
    // when `lambda_hist` is 0, incentive_pool_hist should be ignored since it should also be 0 as no fee was collected
    let total_incentive  = if lambda_hist.is_zero() {
        let p_curr_incentive = calculate_rebalancing_impact(lambda_curr, impact_factor, amount_in)?.to_uint_floor();
        let p_curr_incentive_capped = p_curr_incentive.min(incentive_pool_curr);
        
        p_curr_incentive_capped
    } else {
        let p_hist_incentive = calculate_rebalancing_impact(lambda_hist, impact_factor, amount_in)?.to_uint_floor();
        let p_hist_incentive_capped = p_hist_incentive.min(incentive_pool_hist);
        let rem = p_hist_incentive.checked_sub(p_hist_incentive_capped)?;
        
        // when `incentive_pool_hist` is 0, this becomes `lambda_curr * f * a`
        let p_curr_incentive = Decimal256::from_atomics(rem, 0)?.checked_mul(lambda_curr.into())?.checked_div(lambda_hist.into())?.to_uint_floor();
        let p_curr_incentive_capped = p_curr_incentive.min(incentive_pool_curr);

        p_hist_incentive_capped.checked_add(p_curr_incentive_capped)?
    };


    // capped at Uint128::MAX since bank send only supports Uint128
    let total_capped = total_incentive.min(Uint128::MAX.into());
    Ok(total_capped.try_into()?)
}

#[cfg(test)]
mod tests {
    use std::{cmp::min, str::FromStr};

    use super::*;
    use cosmwasm_std::{Uint256, Uint512};
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
    #[case(
        Decimal::percent(50),
        Uint128::from(100u128),
        Decimal::percent(100),
        Decimal::percent(50),
        Uint256::from(50u128),
        Uint256::from(50u128),
        Ok(Uint128::from(50u128))
    )]
    #[case(
        Decimal::percent(50),
        Uint128::from(200u128),
        Decimal::percent(100), 
        Decimal::percent(50),
        Uint256::from(50u128),
        Uint256::from(50u128),
        Ok(Uint128::from(75u128))
    )]
    #[case(
        Decimal::percent(50),
        Uint128::from(200u128),
        Decimal::percent(100),
        Decimal::percent(50),
        Uint256::from(50u128),
        Uint256::from(0u128),
        Ok(Uint128::from(50u128))
    )]
    #[case(
        Decimal::percent(50),
        Uint128::from(200u128),
        Decimal::percent(100),
        Decimal::percent(50),
        Uint256::from(40u128),
        Uint256::from(0u128),
        Ok(Uint128::from(40u128))
    )]
    #[case(
        Decimal::percent(50),
        Uint128::from(200u128),
        Decimal::percent(100),
        Decimal::percent(50),
        Uint256::from(0u128),
        Uint256::from(20u128),
        Ok(Uint128::from(20u128))
    )]
    #[case(
        Decimal::percent(50),
        Uint128::from(200u128),
        Decimal::percent(100),
        Decimal::percent(50),
        Uint256::from(0u128),
        Uint256::from(200u128),
        Ok(Uint128::from(50u128))
    )]
    #[case(
        Decimal::percent(50),
        Uint128::from(200u128),
        Decimal::percent(0),
        Decimal::percent(50),
        Uint256::from(100u128),
        Uint256::from(100u128),
        Ok(Uint128::from(50u128))
    )]
    #[case(
        Decimal::percent(100),
        Uint128::MAX,
        Decimal::percent(100),
        Decimal::percent(100),
        Uint256::MAX,
        Uint256::MAX,
        Ok(Uint128::MAX)
    )]
    fn test_calculate_rebalancing_incentive(
        #[case] impact_factor: Decimal,
        #[case] amount_in: Uint128,
        #[case] lambda_hist: Decimal,
        #[case] lambda_curr: Decimal,
        #[case] incentive_pool_hist: Uint256,
        #[case] incentive_pool_curr: Uint256,
        #[case] expected: Result<Uint128, TransmuterMathError>,
    ) {
        let actual = calculate_rebalancing_incentive(impact_factor, amount_in, lambda_hist, lambda_curr, incentive_pool_hist, incentive_pool_curr);
        assert_eq!(expected, actual);
    }

    proptest! {
        #[test]
        fn proptest_incentive_less_than_or_equal_sum_of_pools(
            impact_factor in 0u128..=ONE_DEC_RAW,
            amount_in in any::<u128>(),
            lambda_hist in 0u128..=ONE_DEC_RAW,
            lambda_curr in 0u128..=ONE_DEC_RAW,
            incentive_pool_hist_1 in 0u128..=u128::MAX,
            incentive_pool_hist_2 in 0u128..=u128::MAX,
            incentive_pool_curr_1 in 0u128..=u128::MAX,
            incentive_pool_curr_2 in 0u128..=u128::MAX,
        ) {
            let incentive_pool_hist = Uint256::from(incentive_pool_hist_1).checked_add(Uint256::from(incentive_pool_hist_2)).unwrap();
            let incentive_pool_curr = Uint256::from(incentive_pool_curr_1).checked_add(Uint256::from(incentive_pool_curr_2)).unwrap();
            let result = calculate_rebalancing_incentive(
                Decimal::raw(impact_factor),
                Uint128::new(amount_in),
                Decimal::raw(lambda_hist),
                Decimal::raw(lambda_curr),
                Uint256::from(incentive_pool_hist),
                Uint256::from(incentive_pool_curr),
            );

            if let Ok(incentive) = result {
                let total_incentive_pool = Uint512::from(incentive_pool_hist).checked_add(Uint512::from(incentive_pool_curr)).unwrap();
                assert!(Uint512::from(incentive) <= total_incentive_pool);
            }
        }


        #[test]
        fn proptest_incentive_must_always_succeed(
            impact_factor in 0u128..=ONE_DEC_RAW,
            amount_in in any::<u128>(),
            lambda_hist in 0u128..=ONE_DEC_RAW,
            lambda_curr in 0u128..=ONE_DEC_RAW,
            incentive_pool_hist_1 in 0u128..=u128::MAX,
            incentive_pool_hist_2 in 0u128..=u128::MAX,
            incentive_pool_curr_1 in 0u128..=u128::MAX,
            incentive_pool_curr_2 in 0u128..=u128::MAX,
        ) {
            let incentive_pool_hist = Uint256::from(incentive_pool_hist_1).checked_add(Uint256::from(incentive_pool_hist_2)).unwrap();
            let incentive_pool_curr = Uint256::from(incentive_pool_curr_1).checked_add(Uint256::from(incentive_pool_curr_2)).unwrap();
            calculate_rebalancing_incentive(
                Decimal::raw(impact_factor),
                Uint128::new(amount_in),
                Decimal::raw(lambda_hist),
                Decimal::raw(lambda_curr),
                Uint256::from(incentive_pool_hist),
                Uint256::from(incentive_pool_curr),
            ).unwrap();
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
        let actual = calculate_sqrt_cumulative_impact_factor_component(
            normalized_balance,
            ideal_balance_lower_bound,
            ideal_balance_upper_bound,
            upper_limit,
        ).map(|x| x.pow(2));
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

            match calculate_sqrt_cumulative_impact_factor_component(
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
            let actual = calculate_sqrt_cumulative_impact_factor_component(
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

            let c1 = calculate_sqrt_cumulative_impact_factor_component(
                normalized_balance - epsilon,
                ideal_balance_lower_bound,
                ideal_balance_upper_bound,
                upper_limit,
            ).unwrap();

            let c2 = calculate_sqrt_cumulative_impact_factor_component(
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

            let c1 = calculate_sqrt_cumulative_impact_factor_component(
                normalized_balance - epsilon,
                ideal_balance_lower_bound,
                ideal_balance_upper_bound,
                upper_limit,
            ).unwrap();

            let c2 = calculate_sqrt_cumulative_impact_factor_component(
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
    // precision loss for fee impact factor >> 0.000000000000000001
    #[case::precision_loss_positive_impact(
        Decimal::from_str("0.600000000000000001").unwrap(), 
        Decimal::from_str("0.600000000000000002").unwrap(),  
        Decimal::percent(40),                               
        Decimal::percent(60),
        Decimal::one(),
        Ok(SignedDecimal256::from_str("0.000000000000000001").unwrap())
    )]
    #[case::precision_loss_positive_impact(
        Decimal::from_str("0.499999999999999999").unwrap(), 
        Decimal::from_str("0.600000000000000001").unwrap(),  
        Decimal::percent(40),                               
        Decimal::percent(60),
        Decimal::one(),
        Ok(SignedDecimal256::from_str("0.000000000000000001").unwrap())
    )]
    // precision loss for incentive impact factor >> 0
    #[case::precision_loss_negative_impact(
        Decimal::from_str("0.600000000000000002").unwrap(), 
        Decimal::from_str("0.600000000000000001").unwrap(),  
        Decimal::percent(40),                               
        Decimal::percent(60),
        Decimal::one(),
        Ok(SignedDecimal256::zero())
    )]
    #[case::precision_loss_negative_impact(
        Decimal::from_str("0.600000000000000001").unwrap(),  
        Decimal::from_str("0.499999999999999999").unwrap(),
        Decimal::percent(40),                               
        Decimal::percent(60),
        Decimal::one(),
        Ok(SignedDecimal256::zero())
    )]
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

    #[rstest]
    #[case::empty_input(Vec::new(), Ok(ImpactFactor::None))]
    #[case::all_no_change(
        vec![
            ImpactFactorParamGroup {
                prev_normalized_balance: Decimal::percent(50),
                update_normalized_balance: Decimal::percent(50),
                ideal_balance_lower_bound: Decimal::percent(40),
                ideal_balance_upper_bound: Decimal::percent(60),
                upper_limit: Decimal::percent(100),
            },
            ImpactFactorParamGroup {
                prev_normalized_balance: Decimal::percent(70),
                update_normalized_balance: Decimal::percent(70),
                ideal_balance_lower_bound: Decimal::percent(40),
                ideal_balance_upper_bound: Decimal::percent(60),
                upper_limit: Decimal::percent(100),
            }            
        ],
        Ok(ImpactFactor::None)
    )]
    #[case::all_positive_resulted_in_fee(
        vec![
            ImpactFactorParamGroup {
                prev_normalized_balance: Decimal::percent(70),
                update_normalized_balance: Decimal::percent(80),
                ideal_balance_lower_bound: Decimal::percent(40),
                ideal_balance_upper_bound: Decimal::percent(60),
                upper_limit: Decimal::percent(100),
            },
            ImpactFactorParamGroup {
                prev_normalized_balance: Decimal::percent(65),
                update_normalized_balance: Decimal::percent(75),
                ideal_balance_lower_bound: Decimal::percent(40),
                ideal_balance_upper_bound: Decimal::percent(60),
                upper_limit: Decimal::percent(100),
            },
        ],
        Ok(ImpactFactor::Fee(Decimal::from_str("0.159344359799774525").unwrap()))
    )]
    #[case::all_negative_resulted_in_incentive(
        vec![
            ImpactFactorParamGroup {
                prev_normalized_balance: Decimal::percent(70),
                update_normalized_balance: Decimal::percent(60),
                ideal_balance_lower_bound: Decimal::percent(40),
                ideal_balance_upper_bound: Decimal::percent(60),
                upper_limit: Decimal::percent(100),
            },
            ImpactFactorParamGroup {
                prev_normalized_balance: Decimal::percent(35),
                update_normalized_balance: Decimal::percent(45),
                ideal_balance_lower_bound: Decimal::percent(40),
                ideal_balance_upper_bound: Decimal::percent(60),
                upper_limit: Decimal::percent(100),
            },
        ],
        Ok(ImpactFactor::Incentive(Decimal::from_str("0.045554311678478909").unwrap())))
    ]
    #[case::mixed_positive_and_negative_resulted_in_fee(
        vec![
            ImpactFactorParamGroup {
                prev_normalized_balance: Decimal::percent(70),
                update_normalized_balance: Decimal::percent(80),
                ideal_balance_lower_bound: Decimal::percent(40),
                ideal_balance_upper_bound: Decimal::percent(60),
                upper_limit: Decimal::percent(100),
            },
            ImpactFactorParamGroup {
                prev_normalized_balance: Decimal::percent(35),
                update_normalized_balance: Decimal::percent(45),
                ideal_balance_lower_bound: Decimal::percent(40),
                ideal_balance_upper_bound: Decimal::percent(60),
                upper_limit: Decimal::percent(100),
            },
        ],
        Ok(ImpactFactor::Fee(Decimal::from_str("0.133042080983800009").unwrap()))
    )]
    #[case::mixed_positive_and_negative_resulted_in_incentive(
        vec![
            ImpactFactorParamGroup {
                prev_normalized_balance: Decimal::percent(70),
                update_normalized_balance: Decimal::percent(60),
                ideal_balance_lower_bound: Decimal::percent(40),
                ideal_balance_upper_bound: Decimal::percent(60),
                upper_limit: Decimal::percent(100),
            },
            ImpactFactorParamGroup {
                prev_normalized_balance: Decimal::percent(35),
                update_normalized_balance: Decimal::percent(30),
                ideal_balance_lower_bound: Decimal::percent(40),
                ideal_balance_upper_bound: Decimal::percent(60),
                upper_limit: Decimal::percent(100),
            },
        ],
        Ok(ImpactFactor::Incentive(Decimal::from_str("0.055242717280199025").unwrap()))
    )]
    #[case::loss_rounding_fee(
        vec![
            ImpactFactorParamGroup {
                prev_normalized_balance: Decimal::percent(60),
                update_normalized_balance: Decimal::from_atomics(600_000_000_000_000_001u128, 18).unwrap(),
                ideal_balance_lower_bound: Decimal::percent(40),
                ideal_balance_upper_bound: Decimal::percent(60),
                upper_limit: Decimal::percent(100),
            },
            ImpactFactorParamGroup {
                prev_normalized_balance: Decimal::percent(60),
                update_normalized_balance: Decimal::from_atomics(600_000_000_000_000_001u128, 18).unwrap(),
                ideal_balance_lower_bound: Decimal::percent(40),
                ideal_balance_upper_bound: Decimal::percent(60),
                upper_limit: Decimal::percent(100),
            },
        ],
        Ok(ImpactFactor::Fee(Decimal::from_str("0.000000001").unwrap()))    
    )]
    #[case::no_loss_rounding_incentive(
        vec![
            ImpactFactorParamGroup {
                prev_normalized_balance: Decimal::from_atomics(600_000_000_000_000_001u128, 18).unwrap(),
                update_normalized_balance: Decimal::percent(60),
                ideal_balance_lower_bound: Decimal::percent(40),
                ideal_balance_upper_bound: Decimal::percent(60),
                upper_limit: Decimal::percent(100),
            },
            ImpactFactorParamGroup {
                prev_normalized_balance: Decimal::from_atomics(600_000_000_000_000_001u128, 18).unwrap(),
                update_normalized_balance: Decimal::percent(60),
                ideal_balance_lower_bound: Decimal::percent(40),
                ideal_balance_upper_bound: Decimal::percent(60),
                upper_limit: Decimal::percent(100),
            },
        ],
        Ok(ImpactFactor::None)
    )]
    fn test_calculate_impact_factor(
    #[case] input_param_groups: Vec<ImpactFactorParamGroup>,
    #[case] expected: Result<ImpactFactor, TransmuterMathError>,
    ) {
        let result = calculate_impact_factor(&input_param_groups);
        assert_eq!(result, expected);
    }
}
