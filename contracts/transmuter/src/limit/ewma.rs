use std::str::FromStr;

use bigdecimal::{BigDecimal, One, ToPrimitive};
use cosmwasm_std::{Decimal, Deps, DepsMut, StdError, StdResult, Uint128};
use cw_storage_plus::Item;

pub struct EWMA<'a> {
    /// The current EWMA value (timestamp, value)
    pub latest: Item<'a, (u64, Uint128)>,

    /// half life of the EWMA in nanoseconds
    pub half_life: Item<'a, u64>,
}

impl EWMA<'_> {
    pub fn new<'a>(latest_namespace: &'a str, half_life_namespace: &'a str) -> EWMA<'a> {
        EWMA {
            latest: Item::new(latest_namespace),
            half_life: Item::new(half_life_namespace),
        }
    }

    pub fn latest(&self, deps: Deps) -> StdResult<Option<(u64, Uint128)>> {
        self.latest.may_load(deps.storage)
    }

    pub fn set_half_life(&self, deps: DepsMut, half_life: u64) -> StdResult<()> {
        self.half_life.save(deps.storage, &half_life)
    }

    pub fn half_life(&self, deps: Deps) -> StdResult<u64> {
        self.half_life.load(deps.storage)
    }

    pub fn update_latest(&self, deps: DepsMut, now: u64, x: Uint128) -> StdResult<()> {
        let Some((latest_timestamp, latest_ewma)) = self.latest(deps.as_ref())? else {
            return self.latest.save(deps.storage, &(now, x));
        };

        let half_life = BigDecimal::from(self.half_life(deps.as_ref())?);
        let x = BigDecimal::from_str(&x.to_string()).map_err(bigdecimal_to_std_err)?;
        let latest_ewma =
            BigDecimal::from_str(&latest_ewma.to_string()).map_err(bigdecimal_to_std_err)?;
        let detla_t = BigDecimal::from(now - latest_timestamp);
        let ln_2 = BigDecimal::from_str(
            "0.6931471805599453094172321214581765680755001343602552541206800094",
        )
        .map_err(bigdecimal_to_std_err)?;

        // α = 1 - e^(-ln(2) · Δt / half_life)
        // α = 1 - 1 / (e^(ln(2) · Δt / half_life))
        let alpha = BigDecimal::one() - BigDecimal::one() / (ln_2 * detla_t / half_life).exp();

        // ewma(t) = α · x(t) + (1 - α) · ewma(t - 1)
        let ewma = alpha.clone() * x + (BigDecimal::one() - alpha) * latest_ewma;

        // TODO: this does to_u64() and then back to Uint128, which is not ideal
        let ewma = ewma.to_u128().unwrap().into();

        self.latest.save(deps.storage, &(now, ewma))
    }
}

fn bigdecimal_to_std_err(e: bigdecimal::ParseBigDecimalError) -> StdError {
    StdError::parse_err("bigdecimal::BigDecimal", e)
}
