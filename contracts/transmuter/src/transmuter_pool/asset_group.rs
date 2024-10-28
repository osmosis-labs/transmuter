use std::collections::{BTreeMap, HashSet};

use cosmwasm_schema::cw_serde;
use cosmwasm_std::{ensure, Decimal, Decimal256, Uint128, Uint256, Uint64};

use crate::{
    corruptable::Corruptable, math::lcm_from_iter, transmuter_pool::MAX_ASSET_GROUPS, ContractError,
};

use super::TransmuterPool;

#[cw_serde]
pub struct AssetGroup {
    denoms: Vec<String>,
    is_corrupted: bool,
}

impl AssetGroup {
    pub fn new(denoms: Vec<String>) -> Self {
        Self {
            denoms,
            is_corrupted: false,
        }
    }

    pub fn denoms(&self) -> &[String] {
        &self.denoms
    }

    pub fn into_denoms(self) -> Vec<String> {
        self.denoms
    }

    pub fn add_denoms(&mut self, denoms: Vec<String>) -> &mut Self {
        self.denoms.extend(denoms);
        self
    }

    pub fn remove_denoms(&mut self, denoms: Vec<String>) -> &mut Self {
        self.denoms.retain(|d| !denoms.contains(d));
        self
    }
}

impl Corruptable for AssetGroup {
    fn is_corrupted(&self) -> bool {
        self.is_corrupted
    }

    fn mark_as_corrupted(&mut self) -> &mut Self {
        self.is_corrupted = true;
        self
    }

    fn unmark_as_corrupted(&mut self) -> &mut Self {
        self.is_corrupted = false;
        self
    }
}

impl TransmuterPool {
    pub fn has_asset_group(&self, label: &str) -> bool {
        self.asset_groups.contains_key(label)
    }

    pub fn mark_corrupted_asset_group(&mut self, label: &str) -> Result<&mut Self, ContractError> {
        self.asset_groups
            .get_mut(label)
            .ok_or_else(|| ContractError::AssetGroupNotFound {
                label: label.to_string(),
            })?
            .mark_as_corrupted();

        Ok(self)
    }

    pub fn unmark_corrupted_asset_group(
        &mut self,
        label: &str,
    ) -> Result<&mut Self, ContractError> {
        self.asset_groups
            .get_mut(label)
            .ok_or_else(|| ContractError::AssetGroupNotFound {
                label: label.to_string(),
            })?
            .unmark_as_corrupted();

        Ok(self)
    }

    pub fn create_asset_group(
        &mut self,
        label: String,
        denoms: Vec<String>,
    ) -> Result<&mut Self, ContractError> {
        // ensure that asset group does not already exist
        ensure!(
            !self.asset_groups.contains_key(&label),
            ContractError::AssetGroupAlreadyExists {
                label: label.clone()
            }
        );

        // ensure that asset group label is not empty string
        ensure!(!label.is_empty(), ContractError::EmptyAssetGroupLabel {});

        // ensure that all denoms are valid pool assets and has no duplicated denoms
        // ensuring no duplicated denoms also ensures that it's within MAX_POOL_ASSET_DENOMS limit
        let mut denoms_set = HashSet::new();
        for denom in &denoms {
            ensure!(
                self.has_denom(denom),
                ContractError::InvalidPoolAssetDenom {
                    denom: denom.clone()
                }
            );
            ensure!(
                denoms_set.insert(denom.clone()),
                ContractError::DuplicatedPoolAssetDenom {
                    denom: denom.clone()
                }
            );
        }

        self.asset_groups.insert(label, AssetGroup::new(denoms));

        ensure!(
            Uint64::from(self.asset_groups.len() as u64) <= MAX_ASSET_GROUPS,
            ContractError::AssetGroupCountOutOfRange {
                max: MAX_ASSET_GROUPS,
                actual: Uint64::new(self.asset_groups.len() as u64)
            }
        );

        Ok(self)
    }

    pub fn remove_asset_group(&mut self, label: &str) -> Result<&mut Self, ContractError> {
        ensure!(
            self.asset_groups.remove(label).is_some(),
            ContractError::AssetGroupNotFound {
                label: label.to_string()
            }
        );

        Ok(self)
    }

    pub fn asset_group_weights(&self) -> Result<Option<BTreeMap<String, Decimal>>, ContractError> {
        let std_norm_factor = lcm_from_iter(
            self.pool_assets
                .iter()
                .map(|pool_asset| pool_asset.normalization_factor()),
        )?;

        let normalized_asset_values: BTreeMap<String, Uint128> = self
            .normalized_asset_values(std_norm_factor)?
            .into_iter()
            .collect();

        let total_normalized_pool_value = normalized_asset_values
            .values()
            .copied()
            .map(Uint256::from)
            .try_fold(Uint256::zero(), Uint256::checked_add)?;

        if total_normalized_pool_value.is_zero() {
            return Ok(None);
        }

        let mut weights = BTreeMap::new();
        for (label, asset_group) in &self.asset_groups {
            let mut group_normalized_value = Uint256::zero();
            for denom in &asset_group.denoms {
                let denom_normalized_value = normalized_asset_values
                    .get(denom)
                    .copied()
                    .map(Uint256::from)
                    .unwrap_or_else(Uint256::zero);

                group_normalized_value =
                    group_normalized_value.checked_add(denom_normalized_value)?;
            }

            weights.insert(
                label.to_string(),
                Decimal256::checked_from_ratio(
                    group_normalized_value,
                    total_normalized_pool_value,
                )?
                // This is safe since weights are always less than 1, downcasting from Decimal256 to Decimal should never fail
                .try_into()?,
            );
        }

        Ok(Some(weights))
    }
}

#[cfg(test)]
mod tests {
    use cosmwasm_std::Uint128;

    use crate::asset::Asset;

    use super::*;

    #[test]
    fn test_add_remove_denoms() {
        let mut group = AssetGroup::new(vec!["denom1".to_string(), "denom2".to_string()]);

        // Test initial state
        assert_eq!(group.denoms(), &["denom1", "denom2"]);

        // Test adding denoms
        group.add_denoms(vec!["denom3".to_string(), "denom4".to_string()]);
        assert_eq!(group.denoms(), &["denom1", "denom2", "denom3", "denom4"]);

        // Test adding duplicate denom
        group.add_denoms(vec!["denom2".to_string(), "denom5".to_string()]);
        assert_eq!(
            group.denoms(),
            &["denom1", "denom2", "denom3", "denom4", "denom2", "denom5"]
        );

        // Test removing denoms
        group.remove_denoms(vec!["denom2".to_string(), "denom4".to_string()]);
        assert_eq!(group.denoms(), &["denom1", "denom3", "denom5"]);

        // Test removing non-existent denom
        group.remove_denoms(vec!["denom6".to_string()]);
        assert_eq!(group.denoms(), &["denom1", "denom3", "denom5"]);
    }

    #[test]
    fn test_mark_unmark_corrupted() {
        let mut group = AssetGroup::new(vec!["denom1".to_string(), "denom2".to_string()]);

        // Test initial state
        assert!(!group.is_corrupted());

        // Test marking as corrupted
        group.mark_as_corrupted();
        assert!(group.is_corrupted());

        // Test unmarking as corrupted
        group.unmark_as_corrupted();
        assert!(!group.is_corrupted());

        // Test marking and unmarking multiple times
        group.mark_as_corrupted().mark_as_corrupted();
        assert!(group.is_corrupted());
        group.unmark_as_corrupted().unmark_as_corrupted();
        assert!(!group.is_corrupted());
    }

    #[test]
    fn test_asset_group_weights() {
        let mut pool = TransmuterPool::new(vec![
            Asset::new(Uint128::new(200), "denom1", Uint128::new(2)).unwrap(),
            Asset::new(Uint128::new(300), "denom2", Uint128::new(3)).unwrap(),
            Asset::new(Uint128::new(500), "denom3", Uint128::new(5)).unwrap(),
        ])
        .unwrap();

        // Test with empty pool
        let weights = pool.asset_group_weights().unwrap().unwrap_or_default();
        assert!(weights.is_empty());

        pool.create_asset_group(
            "group1".to_string(),
            vec!["denom1".to_string(), "denom2".to_string()],
        )
        .unwrap();

        pool.create_asset_group("group2".to_string(), vec!["denom3".to_string()])
            .unwrap();

        let weights = pool.asset_group_weights().unwrap().unwrap();
        assert_eq!(weights.len(), 2);
        assert_eq!(
            weights.get("group1").unwrap(),
            &Decimal::raw(666666666666666666)
        );
        assert_eq!(
            weights.get("group2").unwrap(),
            &Decimal::raw(333333333333333333)
        );
    }

    #[test]
    fn test_asset_group_weights_with_potential_decimal_precision_loss() {
        let mut pool = TransmuterPool::new(vec![
            Asset::new(Uint128::new(100), "denom1", Uint128::new(1)).unwrap(),
            Asset::new(Uint128::new(200), "denom2", Uint128::new(1)).unwrap(),
            Asset::new(Uint128::new(0), "denom3", Uint128::new(1)).unwrap(),
        ])
        .unwrap();

        pool.create_asset_group(
            "group1".to_string(),
            vec!["denom1".to_string(), "denom2".to_string()],
        )
        .unwrap();

        let weights = pool.asset_group_weights().unwrap().unwrap_or_default();

        assert_eq!(weights.get("group1").unwrap(), &Decimal::percent(100));
    }

    #[test]
    fn test_create_asset_group_with_empty_string() {
        let mut pool = TransmuterPool::new(vec![
            Asset::new(Uint128::new(100), "denom1", Uint128::new(1)).unwrap(),
            Asset::new(Uint128::new(200), "denom2", Uint128::new(1)).unwrap(),
        ])
        .unwrap();

        let err = pool
            .create_asset_group("".to_string(), vec!["denom1".to_string()])
            .unwrap_err();

        assert_eq!(err, ContractError::EmptyAssetGroupLabel {});
    }

    #[test]
    fn test_create_asset_group_with_duplicated_denom() {
        let mut pool = TransmuterPool::new(vec![
            Asset::new(Uint128::new(100), "denom1", Uint128::new(1)).unwrap(),
            Asset::new(Uint128::new(200), "denom2", Uint128::new(1)).unwrap(),
        ])
        .unwrap();

        let err = pool
            .create_asset_group(
                "group1".to_string(),
                vec!["denom1".to_string(), "denom1".to_string()],
            )
            .unwrap_err();

        assert_eq!(
            err,
            ContractError::DuplicatedPoolAssetDenom {
                denom: "denom1".to_string()
            }
        );
    }

    #[test]
    fn test_create_asset_group_within_range() {
        let mut pool = TransmuterPool::new(vec![
            Asset::new(Uint128::new(100), "denom1", Uint128::new(1)).unwrap(),
            Asset::new(Uint128::new(200), "denom2", Uint128::new(1)).unwrap(),
            Asset::new(Uint128::new(300), "denom3", Uint128::new(1)).unwrap(),
        ])
        .unwrap();

        // Test creating groups up to the maximum allowed
        for i in 1..=MAX_ASSET_GROUPS.u64() {
            let group_name = format!("group{}", i);
            let result = pool.create_asset_group(group_name.clone(), vec!["denom1".to_string()]);
            assert!(result.is_ok(), "Failed to create group {}", i);
        }

        // Attempt to create one more group, which should fail
        let result = pool.create_asset_group("extra_group".to_string(), vec!["denom1".to_string()]);
        assert!(
            result.is_err(),
            "Should not be able to create group beyond the maximum"
        );
        assert!(
            matches!(
                result.unwrap_err(),
                ContractError::AssetGroupCountOutOfRange { max, actual }
                if max == MAX_ASSET_GROUPS && actual == MAX_ASSET_GROUPS + Uint64::one()
            ),
            "Unexpected error when exceeding max asset groups"
        );
    }
}
