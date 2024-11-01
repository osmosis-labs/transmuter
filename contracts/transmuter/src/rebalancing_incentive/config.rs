use std::collections::{HashMap, HashSet};

use cosmwasm_schema::cw_serde;
use cosmwasm_std::{ensure, Decimal};

use crate::{scope::Scope, ContractError};

#[cw_serde]
#[derive(Copy)]
pub struct IdealBalance {
    pub lower: Decimal,
    pub upper: Decimal,
}

impl IdealBalance {
    pub fn new(lower: Decimal, upper: Decimal) -> Self {
        Self { lower, upper }
    }
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
    pub fn set_lambda(&mut self, new_lambda: Decimal) -> Result<&mut Self, ContractError> {
        ensure!(
            new_lambda <= Decimal::one(),
            ContractError::InvalidLambda { lambda: new_lambda }
        );

        self.lambda = new_lambda;
        Ok(self)
    }

    pub fn set_ideal_balances(
        &mut self,
        available_denoms: impl IntoIterator<Item = String>,
        available_asset_groups: impl IntoIterator<Item = String>,
        new_ideal_balances: impl IntoIterator<Item = (Scope, IdealBalance)>,
    ) -> Result<&mut Self, ContractError> {
        let available_scopes: HashSet<_> = available_denoms
            .into_iter()
            .map(Scope::Denom)
            .chain(available_asset_groups.into_iter().map(Scope::AssetGroup))
            .collect();

        let mut seen_scopes = HashSet::new();

        for (scope, balance) in new_ideal_balances {
            let is_unseen = seen_scopes.insert(scope.clone());
            ensure!(is_unseen, ContractError::DuplicatedScope { scope });

            ensure!(
                available_scopes.contains(&scope),
                ContractError::ScopeNotFound { scope }
            );

            // set ideal balance with 0-1 bounds will delete value from storage
            // as it is interpreted as default value
            if balance == IdealBalance::default() {
                self.ideal_balances.remove(&scope);
            } else {
                self.ideal_balances.insert(scope, balance);
            }
        }

        Ok(self)
    }

    /// Explicitly remove ideal balance for scope
    pub fn remove_ideal_balance(&mut self, scope: Scope) -> Result<IdealBalance, ContractError> {
        self.ideal_balances
            .remove(&scope)
            .ok_or_else(|| ContractError::ScopeNotFound { scope })
    }

    pub fn lambda(&self) -> Decimal {
        self.lambda
    }

    pub fn ideal_balances(&self) -> &HashMap<Scope, IdealBalance> {
        &self.ideal_balances
    }

    pub fn ideal_balance(&self, scope: Scope) -> Option<&IdealBalance> {
        self.ideal_balances.get(&scope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmwasm_std::Decimal;
    use rstest::rstest;

    #[rstest]
    #[case(Decimal::percent(50), true)]
    #[case(Decimal::percent(100), true)]
    #[case(Decimal::percent(150), false)]
    fn test_set_lambda(#[case] new_lambda: Decimal, #[case] expected_result: bool) {
        let mut config = RebalancingIncentiveConfig {
            lambda: Decimal::percent(0),
            ideal_balances: HashMap::new(),
        };

        let result = config.set_lambda(new_lambda);

        if expected_result {
            assert!(result.is_ok());
            assert_eq!(config.lambda(), new_lambda);
        } else {
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_set_ideal_balances() {
        let existing_ideal_balances = vec![
            (
                Scope::Denom("existing_denom".to_string()),
                IdealBalance::new(Decimal::percent(10), Decimal::percent(25)),
            ),
            (
                Scope::AssetGroup("existing_asset_group".to_string()),
                IdealBalance::new(Decimal::percent(20), Decimal::percent(30)),
            ),
        ];

        let mut config = RebalancingIncentiveConfig {
            lambda: Decimal::percent(0),
            ideal_balances: existing_ideal_balances.clone().into_iter().collect(),
        };

        let available_denoms = vec!["valid_denom".to_string(), "existing_denom".to_string()];
        let available_asset_groups = vec![
            "valid_asset_group".to_string(),
            "existing_asset_group".to_string(),
        ];

        // get ideal balance for valid_denom when not set
        let ideal_balance = config.ideal_balance(Scope::denom("valid_denom"));
        assert_eq!(ideal_balance, None);

        // default ideal balance for valid_denom
        assert_eq!(
            ideal_balance.as_deref().cloned().unwrap_or_default(),
            IdealBalance::new(Decimal::zero(), Decimal::one())
        );

        // get ideal balance for existing_denom
        let ideal_balance = config.ideal_balance(Scope::denom("existing_denom"));
        assert_eq!(
            ideal_balance.as_deref().cloned().unwrap(),
            IdealBalance::new(Decimal::percent(10), Decimal::percent(25))
        );

        // get ideal balance for valid_asset_group
        let ideal_balance = config.ideal_balance(Scope::asset_group("valid_asset_group"));
        assert_eq!(
            ideal_balance.as_deref().cloned().unwrap_or_default(),
            IdealBalance::new(Decimal::zero(), Decimal::one())
        );

        // get ideal balance for existing_asset_group
        let ideal_balance = config.ideal_balance(Scope::asset_group("existing_asset_group"));
        assert_eq!(
            ideal_balance.as_deref().cloned().unwrap(),
            IdealBalance::new(Decimal::percent(20), Decimal::percent(30))
        );

        // set invalid denom
        let new_ideal_balances = vec![(
            Scope::denom("invalid_denom"),
            IdealBalance::new(Decimal::percent(30), Decimal::percent(40)),
        )];

        let err = config
            .set_ideal_balances(
                available_denoms.clone(),
                available_asset_groups.clone(),
                new_ideal_balances,
            )
            .unwrap_err();
        assert_eq!(
            err,
            ContractError::ScopeNotFound {
                scope: Scope::Denom("invalid_denom".to_string())
            }
        );

        // set invalid asset group
        let new_ideal_balances = vec![(
            Scope::asset_group("invalid_asset_group"),
            IdealBalance::new(Decimal::percent(30), Decimal::percent(40)),
        )];

        let err = config
            .set_ideal_balances(
                available_denoms.clone(),
                available_asset_groups.clone(),
                new_ideal_balances,
            )
            .unwrap_err();
        assert_eq!(
            err,
            ContractError::ScopeNotFound {
                scope: Scope::AssetGroup("invalid_asset_group".to_string())
            }
        );

        // set valid denom and asset group
        let new_ideal_balances = vec![
            (
                Scope::denom("valid_denom"),
                IdealBalance::new(Decimal::percent(23), Decimal::percent(37)),
            ),
            (
                Scope::asset_group("valid_asset_group"),
                IdealBalance::new(Decimal::percent(10), Decimal::percent(40)),
            ),
        ];

        let result = config
            .set_ideal_balances(
                available_denoms.clone(),
                available_asset_groups.clone(),
                new_ideal_balances.clone(),
            )
            .unwrap();
        assert_eq!(
            result.ideal_balances(),
            &vec![existing_ideal_balances, new_ideal_balances,]
                .concat()
                .into_iter()
                .collect::<HashMap<_, _>>()
        );

        // update existing denom
        let new_ideal_balances = vec![(
            Scope::denom("existing_denom"),
            IdealBalance::new(Decimal::percent(15), Decimal::percent(25)),
        )];

        let result = config
            .set_ideal_balances(
                available_denoms.clone(),
                available_asset_groups.clone(),
                new_ideal_balances,
            )
            .unwrap();
        assert_eq!(
            result.ideal_balances(),
            &vec![
                (
                    Scope::Denom("existing_denom".to_string()),
                    IdealBalance::new(Decimal::percent(15), Decimal::percent(25)),
                ),
                (
                    Scope::AssetGroup("existing_asset_group".to_string()),
                    IdealBalance::new(Decimal::percent(20), Decimal::percent(30)),
                ),
                (
                    Scope::denom("valid_denom"),
                    IdealBalance::new(Decimal::percent(23), Decimal::percent(37)),
                ),
                (
                    Scope::asset_group("valid_asset_group"),
                    IdealBalance::new(Decimal::percent(10), Decimal::percent(40)),
                ),
            ]
            .into_iter()
            .collect()
        );

        // set ideal balance with duplicated scopes will fail
        let new_ideal_balances = vec![
            (
                Scope::denom("existing_denom"),
                IdealBalance::new(Decimal::percent(15), Decimal::percent(25)),
            ),
            (
                Scope::denom("existing_denom"),
                IdealBalance::new(Decimal::percent(15), Decimal::percent(20)),
            ),
        ];

        let err = config
            .set_ideal_balances(
                available_denoms.clone(),
                available_asset_groups.clone(),
                new_ideal_balances,
            )
            .unwrap_err();
        assert_eq!(
            err,
            ContractError::DuplicatedScope {
                scope: Scope::Denom("existing_denom".to_string())
            }
        );

        // set ideal balance with 0-1 bounds will delete value from storage
        let new_ideal_balances = vec![
            (Scope::denom("existing_denom"), IdealBalance::default()),
            (
                Scope::asset_group("existing_asset_group"),
                IdealBalance::default(),
            ),
            (Scope::denom("valid_denom"), IdealBalance::default()),
            (
                Scope::asset_group("valid_asset_group"),
                IdealBalance::new(Decimal::zero(), Decimal::percent(1)),
            ),
        ];
        let result = config
            .set_ideal_balances(available_denoms, available_asset_groups, new_ideal_balances)
            .unwrap();
        assert_eq!(
            result.ideal_balances(),
            &vec![(
                Scope::asset_group("valid_asset_group"),
                IdealBalance::new(Decimal::zero(), Decimal::percent(1)),
            ),]
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn test_remove_ideal_balance() {
        let mut config = RebalancingIncentiveConfig {
            lambda: Decimal::percent(0),
            ideal_balances: vec![
                (
                    Scope::Denom("existing_denom".to_string()),
                    IdealBalance::new(Decimal::percent(10), Decimal::percent(25)),
                ),
                (
                    Scope::AssetGroup("existing_asset_group".to_string()),
                    IdealBalance::new(Decimal::percent(20), Decimal::percent(30)),
                ),
            ]
            .into_iter()
            .collect(),
        };

        // Test removing an existing ideal balance
        let removed_balance = config
            .remove_ideal_balance(Scope::Denom("existing_denom".to_string()))
            .unwrap();
        assert_eq!(
            removed_balance,
            IdealBalance::new(Decimal::percent(10), Decimal::percent(25))
        );
        assert!(!config
            .ideal_balances()
            .contains_key(&Scope::Denom("existing_denom".to_string())));

        // Test removing a non-existing ideal balance
        let err = config
            .remove_ideal_balance(Scope::Denom("non_existing_denom".to_string()))
            .unwrap_err();
        assert_eq!(
            err,
            ContractError::ScopeNotFound {
                scope: Scope::Denom("non_existing_denom".to_string())
            }
        );
    }
}
