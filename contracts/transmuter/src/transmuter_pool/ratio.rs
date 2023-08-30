use cosmwasm_std::{Decimal, Uint128};

use crate::ContractError;

use super::TransmuterPool;

impl TransmuterPool {
    /// Ratio of the pool asset amount to the total pool asset amount.
    pub fn ratio(&self, denom: &str) -> Result<Option<Decimal>, ContractError> {
        let pool_asset = self
            .pool_assets
            .iter()
            .find(|pool_asset| pool_asset.denom == denom)
            .ok_or_else(|| ContractError::InvalidPoolAssetDenom {
                denom: denom.to_string(),
            })?;

        let total_pool_asset_amount: Uint128 = self
            .pool_assets
            .iter()
            .map(|pool_asset| pool_asset.amount)
            .fold(Ok(Uint128::zero()), |acc, amount| {
                acc.and_then(|acc| acc.checked_add(amount))
            })?;

        let ratio = if total_pool_asset_amount.is_zero() {
            None
        } else {
            Some(Decimal::from_ratio(
                pool_asset.amount,
                total_pool_asset_amount,
            ))
        };

        Ok(ratio)
    }

    // TODO: ratio for all pool assets
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use cosmwasm_std::Coin;

    use super::*;

    #[test]
    fn test_ratio_when_there_are_target_denom() {
        let pool = TransmuterPool {
            pool_assets: vec![Coin::new(6000, "axlusdc"), Coin::new(4000, "whusdc")],
        };

        let ratio = pool.ratio("axlusdc").unwrap();
        assert_eq!(ratio, Some(Decimal::percent(60)));

        let ratio = pool.ratio("whusdc").unwrap();
        assert_eq!(ratio, Some(Decimal::percent(40)));

        let pool = TransmuterPool {
            pool_assets: vec![Coin::new(0, "axlusdc"), Coin::new(9999, "whusdc")],
        };

        let ratio = pool.ratio("axlusdc").unwrap();
        assert_eq!(ratio, Some(Decimal::percent(0)));

        let ratio = pool.ratio("whusdc").unwrap();
        assert_eq!(ratio, Some(Decimal::percent(100)));

        let pool = TransmuterPool {
            pool_assets: vec![
                Coin::new(2, "axlusdc"),
                Coin::new(9999, "whusdc"),
                Coin::new(9999, "xusdc"),
            ],
        };

        let ratio = pool.ratio("axlusdc").unwrap();
        assert_eq!(ratio, Some(Decimal::from_str("0.0001").unwrap()));

        let ratio = pool.ratio("whusdc").unwrap();
        assert_eq!(ratio, Some(Decimal::from_str("0.49995").unwrap()));

        let ratio = pool.ratio("xusdc").unwrap();
        assert_eq!(ratio, Some(Decimal::from_str("0.49995").unwrap()));
    }

    #[test]
    fn test_ratio_when_there_are_no_target_denom() {
        let pool = TransmuterPool {
            pool_assets: vec![
                Coin::new(2, "axlusdc"),
                Coin::new(9999, "whusdc"),
                Coin::new(9999, "xusdc"),
            ],
        };

        let err = pool.ratio("random").unwrap_err();

        assert_eq!(
            err,
            ContractError::InvalidPoolAssetDenom {
                denom: "random".to_string()
            }
        );
    }

    #[test]
    fn test_ratio_when_total_pool_asset_amount_is_zero() {
        let pool = TransmuterPool {
            pool_assets: vec![Coin::new(0, "axlusdc"), Coin::new(0, "whusdc")],
        };

        let ratio = pool.ratio("axlusdc").unwrap();
        assert_eq!(ratio, None);

        let ratio = pool.ratio("whusdc").unwrap();
        assert_eq!(ratio, None);
    }
}
