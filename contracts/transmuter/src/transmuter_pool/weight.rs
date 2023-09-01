use cosmwasm_std::{Decimal, Uint128};

use crate::ContractError;

use super::TransmuterPool;

impl TransmuterPool {
    /// All weights of each pool assets. Returns pairs of (denom, weight)
    /// If total pool asset amount is zero, returns None to signify that
    /// it makes no sense to calculate ratios, but not an error.
    pub fn weights(&self) -> Result<Option<Vec<(String, Decimal)>>, ContractError> {
        let total_pool_asset_amount = self.total_pool_assets()?;

        if total_pool_asset_amount.is_zero() {
            return Ok(None);
        }

        let ratios = self
            .pool_assets
            .iter()
            .map(|pool_asset| {
                let ratio =
                    Decimal::checked_from_ratio(pool_asset.amount, total_pool_asset_amount)?;
                Ok((pool_asset.denom.to_string(), ratio))
            })
            .collect::<Result<_, ContractError>>()?;

        Ok(Some(ratios))
    }

    fn total_pool_assets(&self) -> Result<Uint128, ContractError> {
        self.pool_assets
            .iter()
            .map(|pool_asset| pool_asset.amount)
            .fold(Ok(Uint128::zero()), |acc, amount| {
                acc.and_then(|acc| acc.checked_add(amount).map_err(Into::into))
            })
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use cosmwasm_std::Coin;

    use super::*;

    #[test]
    fn test_all_ratios() {
        let pool = TransmuterPool {
            pool_assets: vec![Coin::new(6000, "axlusdc"), Coin::new(4000, "whusdc")],
        };

        let ratios = pool.weights().unwrap();
        assert_eq!(
            ratios,
            Some(vec![
                ("axlusdc".to_string(), Decimal::percent(60)),
                ("whusdc".to_string(), Decimal::percent(40))
            ])
        );

        let pool = TransmuterPool {
            pool_assets: vec![Coin::new(0, "axlusdc"), Coin::new(9999, "whusdc")],
        };

        let ratios = pool.weights().unwrap();
        assert_eq!(
            ratios,
            Some(vec![
                ("axlusdc".to_string(), Decimal::percent(0)),
                ("whusdc".to_string(), Decimal::percent(100))
            ])
        );

        let pool = TransmuterPool {
            pool_assets: vec![
                Coin::new(2, "axlusdc"),
                Coin::new(9999, "whusdc"),
                Coin::new(9999, "xusdc"),
            ],
        };

        let ratios = pool.weights().unwrap();
        assert_eq!(
            ratios,
            Some(vec![
                ("axlusdc".to_string(), Decimal::from_str("0.0001").unwrap()),
                ("whusdc".to_string(), Decimal::from_str("0.49995").unwrap()),
                ("xusdc".to_string(), Decimal::from_str("0.49995").unwrap())
            ])
        );
    }

    #[test]
    fn test_all_ratios_when_total_pool_assets_is_zero() {
        let pool = TransmuterPool {
            pool_assets: vec![Coin::new(0, "axlusdc"), Coin::new(0, "whusdc")],
        };

        let ratios = pool.weights().unwrap();
        assert_eq!(ratios, None);
    }
}
