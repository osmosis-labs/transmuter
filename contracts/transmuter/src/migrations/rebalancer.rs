use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Decimal, Order, Storage};
use cw_storage_plus::Map;
use transmuter_math::rebalancing::config::RebalancingConfig;

use crate::{contract::key, rebalancer::Rebalancer, scope::Scope, ContractError};

const LIMITERS_KEY: &str = "limiters";

/// Limiter that determines limit by upper bound of the value.
#[cw_serde]
pub struct StaticLimiter {
    /// Upper limit of the value
    upper_limit: Decimal,
}

/// NOTE: Change limiter is removed from this version so we don't need to deserialize it.
/// By keeping this empty, the deserialization will be able to match the structure defined in [Limiter] but the value inside will be ignored.
#[cw_serde]
pub struct ChangeLimiter {}

#[cw_serde]
pub enum Limiter {
    ChangeLimiter(ChangeLimiter),
    StaticLimiter(StaticLimiter),
}

pub fn migrate_limiters_to_rebalancer(storage: &mut dyn Storage) -> Result<(), ContractError> {
    // Map of (denom, label) -> Limiter
    let limiters = Map::<(&str, &str), Limiter>::new(LIMITERS_KEY);
    let rebalancer = Rebalancer::new(key::REBALANCER);

    let limiter_data: Vec<_> = limiters
        .range(storage, None, None, Order::Ascending)
        .collect::<Result<Vec<_>, _>>()?;

    for ((denom, _), limiter) in limiter_data {
        let scope = Scope::Denom(denom.to_string());
        let limit = match limiter {
            Limiter::StaticLimiter(limiter) => limiter.upper_limit,
            _ => Decimal::one(), // Default to 100%
        };
        let rebalancing_config = RebalancingConfig::limit_only(limit)?;

        match rebalancer.get_config_by_scope(storage, &scope)? {
            Some(existing_config) if existing_config.limit > rebalancing_config.limit => {
                rebalancer.update_config(storage, scope, &rebalancing_config)?;
            }
            Some(_) => {
                // existing config has lower or equal limit, no update needed
            }
            None => {
                rebalancer.add_config(storage, scope, rebalancing_config)?;
            }
        }
    }

    // remove the limiters map
    for (k, _) in limiters
        .range(storage, None, None, Order::Ascending)
        .collect::<Result<Vec<_>, _>>()?
    {
        limiters.remove(storage, (k.0.as_str(), k.1.as_str()));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ops::Deref;

    use cosmwasm_std::testing::mock_dependencies;
    use cw_storage_plus::Path;

    use super::*;

    #[test]
    fn test_migrate_limiters_to_rebalancer() {
        let mut deps = mock_dependencies();

        let kvs = vec![
            (
                [b"denom1".as_slice(), b"static1".as_slice()],
                "{\"static_limiter\":{\"upper_limit\":\"0.5\"}}",
            ),
            (
                [b"denom2".as_slice(), b"static1".as_slice()],
                "{\"static_limiter\":{\"upper_limit\":\"0.4\"}}",
            ),
            (
                [b"denom2".as_slice(), b"static2".as_slice()],
                "{\"static_limiter\":{\"upper_limit\":\"0.2\"}}",
            ),
            (
                [b"denom2".as_slice(), b"static3".as_slice()],
                "{\"static_limiter\":{\"upper_limit\":\"0.3\"}}",
            ),
            (
                [b"denom1".as_slice(), b"dynamic1".as_slice()],
                "{\"change_limiter\":{\"whatever\":\"doesnt matter\"}}",
            ),
        ];

        for (k, v) in kvs {
            deps.storage.set(
                Path::<Limiter>::new(LIMITERS_KEY.as_bytes(), &k).deref(),
                v.as_bytes(),
            );
        }

        migrate_limiters_to_rebalancer(&mut deps.storage).unwrap();

        let rebalancer = Rebalancer::new(key::REBALANCER);

        assert_eq!(
            rebalancer.list_configs(&deps.storage).unwrap(),
            vec![
                (
                    "denom::denom1".to_string(),
                    RebalancingConfig::limit_only(Decimal::percent(50)).unwrap()
                ),
                (
                    "denom::denom2".to_string(),
                    RebalancingConfig::limit_only(Decimal::percent(20)).unwrap()
                ),
            ]
        );

        // assert that the limiters map is empty
        let limiters = Map::<(&str, &str), Limiter>::new(LIMITERS_KEY);
        assert_eq!(
            limiters
                .range(&deps.storage, None, None, Order::Ascending)
                .next(),
            None
        );
    }
}
