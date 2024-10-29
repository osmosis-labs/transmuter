use std::collections::{HashMap, HashSet};

use cosmwasm_schema::cw_serde;
use cosmwasm_std::Decimal;

use crate::{scope::Scope, ContractError};

#[cw_serde]
pub struct IdealBalance {
    pub lower: Decimal,
    pub upper: Decimal,
}

/// Default ideal balance bounds 0 - 100%
/// This means that by default, the transmuter will not incentivize rebalancing
/// as it is interpreted as ideal for the whole range.
impl Default for IdealBalance {
    fn default() -> Self {
        Self {
            lower: Decimal::zero(),
            upper: Decimal::one(),
        }
    }
}

#[cw_serde]
#[derive(Default)]
pub struct RebalancingIncentiveConfig {
    /// The lambda parameter for scaling the fee \in λ ∈ (0,1], default is 0
    pub lambda: Decimal,

    /// Ideal balance bounds for each asset
    pub ideal_balances: HashMap<Scope, IdealBalance>,
}

impl RebalancingIncentiveConfig {
    pub fn set_lambda(self, new_lambda: Decimal) -> Result<Self, ContractError> {
        if new_lambda <= Decimal::zero() || new_lambda > Decimal::one() {
            todo!("Return Invalid lambda");
        }

        Ok(Self {
            lambda: new_lambda,
            ..self
        })
    }

    pub fn set_ideal_balances(
        self,
        available_denoms: impl Iterator<Item = String>,
        available_asset_groups: impl Iterator<Item = String>,
        new_ideal_balances: impl Iterator<Item = (Scope, IdealBalance)>,
    ) -> Result<Self, ContractError> {
        let available_scopes: HashSet<_> = available_denoms
            .map(Scope::Denom)
            .chain(available_asset_groups.map(Scope::AssetGroup))
            .collect();

        let mut ideal_balances = self.ideal_balances;
        for (scope, balance) in new_ideal_balances {
            if !available_scopes.contains(&scope) {
                todo!("Return Invalid scope");
            }
            ideal_balances.insert(scope, balance);
        }

        Ok(Self {
            ideal_balances,
            ..self
        })
    }
}
