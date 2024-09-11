use cosmwasm_std::{Decimal, Decimal256, SignedDecimal256, Uint128};

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
            .pow(2) // ^2
    } else if normalized_balance > ideal_balance_upper_bound {
        normalized_balance // b
            .checked_sub(ideal_balance_upper_bound)? // - phi_u
            .checked_div(upper_limit.checked_sub(ideal_balance_upper_bound)?)? // / delta - phi_u
            .pow(2) // ^2
    } else {
        // within ideal balance
        Decimal::zero()
    };

    Ok(cumulative)
}

pub struct ImpactFactorParamGroup {
    prev_normalized_balance: Decimal,
    update_normalized_balance: Decimal,
    ideal_balance_lower_bound: Decimal,
    ideal_balance_upper_bound: Decimal,
    upper_limit: Decimal,
}

impl ImpactFactorParamGroup {
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

pub enum ReblancingResponse {
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
) -> Result<(ReblancingResponse, Decimal256), TransmuterMathError> {
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

    let reaction = if impact_factor_component_sum.is_negative() {
        ReblancingResponse::Incentive
    } else {
        ReblancingResponse::Fee
    };

    let impact_factor = cumulative_impact_factor_sqaure.checked_div(n)?.sqrt();

    Ok((reaction, impact_factor))
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
    if lambda > Decimal::one() {
        return Err(TransmuterMathError::NotNormalized {
            var_name: "lambda".to_string(),
        });
    }

    if impact_factor > Decimal::one() {
        return Err(TransmuterMathError::NotNormalized {
            var_name: "impact_factor".to_string(),
        });
    }

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
) -> Result<Decimal, TransmuterMathError> {
    let incentive_pool_dec = Decimal::from_atomics(incentive_pool, 0)?;
    let extended_incentive_pool = incentive_pool_dec.checked_add(impact)?;
    let impact_over_extended_incentive_pool = impact.checked_div(extended_incentive_pool)?;

    impact_over_extended_incentive_pool
        .checked_mul(incentive_pool_dec)
        .map_err(TransmuterMathError::OverflowError)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

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
        Err(TransmuterMathError::NotNormalized { var_name: "lambda".to_string() })
    )]
    #[case(
        Decimal::percent(100),
        Decimal::percent(101),
        Uint128::MAX,
        Err(TransmuterMathError::NotNormalized { var_name: "impact_factor".to_string() })
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
}
