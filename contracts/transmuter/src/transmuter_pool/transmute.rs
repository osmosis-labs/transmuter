use cosmwasm_std::{ensure, Coin, StdError, Uint128};

use crate::{asset::Rounding, ContractError};

use super::TransmuterPool;

#[derive(Clone, Debug, PartialEq)]
pub enum AmountConstraint {
    ExactIn(Uint128),
    ExactOut(Uint128),
}

impl AmountConstraint {
    pub fn exact_in(amount: impl Into<Uint128>) -> Self {
        Self::ExactIn(amount.into())
    }

    pub fn exact_out(amount: impl Into<Uint128>) -> Self {
        Self::ExactOut(amount.into())
    }
}

impl TransmuterPool {
    // TODO: take normalization factor into account to how much the resulted token_out will be
    pub fn transmute(
        &mut self,
        amount_constraint: AmountConstraint,
        token_in_denom: &str,
        token_out_denom: &str,
    ) -> Result<Coin, ContractError> {
        // ensure transmuting denom is one of the pool assets
        let pool_asset_by_denom = |denom: &str| {
            self.pool_assets
                .iter()
                .find(|pool_asset| pool_asset.denom() == denom)
        };

        // get all pool asset denoms
        let pool_asset_denoms: Vec<String> = self
            .pool_assets
            .iter()
            .map(|pool_asset| pool_asset.denom().to_string())
            .collect();

        // check if token_in is in pool_assets
        let token_in_pool_asset = pool_asset_by_denom(&token_in_denom).ok_or_else(|| {
            ContractError::InvalidTransmuteDenom {
                denom: token_in_denom.to_string(),
                expected_denom: pool_asset_denoms.clone(),
            }
        })?;

        // check if token_out_denom is in pool_assets
        let token_out_pool_asset = pool_asset_by_denom(token_out_denom).ok_or_else(|| {
            ContractError::InvalidTransmuteDenom {
                denom: token_out_denom.to_string(),
                expected_denom: pool_asset_denoms,
            }
        })?;

        let token_out_amount = match amount_constraint {
            AmountConstraint::ExactIn(in_amount) => token_in_pool_asset.convert_amount(
                in_amount,
                token_out_pool_asset.normalization_factor(),
                // rounding down token out amount for swap exact amount in
                // this will ensure no loss in liquidity value-wise
                // since it keeps in out value <= in value
                Rounding::DOWN,
            )?,
            AmountConstraint::ExactOut(out_amount) => out_amount,
        };

        let token_in_amount = match amount_constraint {
            AmountConstraint::ExactIn(in_amount) => in_amount,
            AmountConstraint::ExactOut(out_amount) => token_out_pool_asset.convert_amount(
                out_amount,
                token_in_pool_asset.normalization_factor(),
                // rounding up token in amount for swap exact amount out
                // this will ensure no loss in liquidity value-wise
                // since it keeps in value >= out value
                Rounding::UP,
            )?,
        };

        // Calculate the amount of token_out based on the normalization factor of token_in and token_out
        let token_out = Coin::new(token_out_amount.u128(), token_out_denom);

        // ensure there is enough token_out_denom in the pool
        ensure!(
            token_out_pool_asset.amount() >= token_out_amount,
            ContractError::InsufficientPoolAsset {
                required: token_out,
                available: token_out_pool_asset.to_coin()
            }
        );

        for pool_asset in &mut self.pool_assets {
            // increase token in from pool assets
            if token_in_denom == pool_asset.denom() {
                pool_asset.update_amount(|amount| {
                    amount
                        .checked_add(token_in_amount)
                        .map_err(StdError::overflow)
                        .map_err(ContractError::Std)
                })?;
            }

            // decrease token out from pool assets
            if token_out.denom == pool_asset.denom() {
                pool_asset.update_amount(|amount| {
                    amount
                        .checked_sub(token_out.amount)
                        .map_err(StdError::overflow)
                        .map_err(ContractError::Std)
                })?;
            }
        }

        Ok(token_out)
    }
}

#[cfg(test)]
mod tests {
    use cosmwasm_std::{testing::mock_dependencies, Uint128};

    use crate::asset::{Asset, AssetConfig};

    use super::*;
    const ETH_USDC: &str = "ibc/AXLETHUSDC";
    const COSMOS_USDC: &str = "ibc/COSMOSUSDC";

    const NBTC_SAT: &str = "usat";
    const WBTC_SAT: &str = "wbtc-satoshi";

    #[test]
    fn test_transmute_succeed() {
        let mut pool =
            TransmuterPool::new(Asset::unchecked_equal_assets(&[ETH_USDC, COSMOS_USDC])).unwrap();

        pool.join_pool(&[Coin::new(70_000, COSMOS_USDC)]).unwrap();
        assert_eq!(
            pool.transmute(
                AmountConstraint::exact_in(70_000u128),
                &ETH_USDC,
                &COSMOS_USDC
            )
            .unwrap(),
            Coin::new(70_000, COSMOS_USDC)
        );

        pool.join_pool(&[Coin::new(100_000, COSMOS_USDC)]).unwrap();
        assert_eq!(
            pool.transmute(
                AmountConstraint::exact_in(60_000u128),
                &ETH_USDC,
                &COSMOS_USDC
            )
            .unwrap(),
            Coin::new(60_000, COSMOS_USDC)
        );
        assert_eq!(
            pool.transmute(
                AmountConstraint::exact_in(20_000u128),
                &ETH_USDC,
                &COSMOS_USDC
            )
            .unwrap(),
            Coin::new(20_000, COSMOS_USDC)
        );
        assert_eq!(
            pool.transmute(
                AmountConstraint::exact_in(20_000u128),
                &ETH_USDC,
                &COSMOS_USDC
            )
            .unwrap(),
            Coin::new(20_000, COSMOS_USDC)
        );
        assert_eq!(
            pool.transmute(AmountConstraint::exact_in(0u128), &ETH_USDC, &COSMOS_USDC)
                .unwrap(),
            Coin::new(0, COSMOS_USDC)
        );

        assert_eq!(
            pool.pool_assets,
            Asset::unchecked_equal_assets_from_coins(&[
                Coin::new(170_000, ETH_USDC),
                Coin::new(0, COSMOS_USDC),
            ])
        );

        assert_eq!(
            pool.transmute(
                AmountConstraint::exact_in(100_000u128),
                &COSMOS_USDC,
                &ETH_USDC
            )
            .unwrap(),
            Coin::new(100_000, ETH_USDC)
        );

        assert_eq!(
            pool.pool_assets,
            Asset::unchecked_equal_assets_from_coins(&[
                Coin::new(70_000, ETH_USDC),
                Coin::new(100_000, COSMOS_USDC)
            ])
        );
    }

    #[test]
    fn test_transmute_token_out_denom_eq_token_in_denom() {
        let mut pool =
            TransmuterPool::new(Asset::unchecked_equal_assets(&[ETH_USDC, COSMOS_USDC])).unwrap();

        pool.join_pool(&[Coin::new(70_000, COSMOS_USDC)]).unwrap();

        assert_eq!(
            pool.transmute(
                AmountConstraint::exact_in(70_000u128),
                COSMOS_USDC,
                COSMOS_USDC
            )
            .unwrap(),
            Coin::new(70_000, COSMOS_USDC)
        );
    }

    #[test]
    fn test_transmute_fail_token_out_not_enough() {
        let mut pool =
            TransmuterPool::new(Asset::unchecked_equal_assets(&[ETH_USDC, COSMOS_USDC])).unwrap();

        pool.join_pool(&[Coin::new(70_000, COSMOS_USDC)]).unwrap();
        assert_eq!(
            pool.transmute(
                AmountConstraint::exact_in(70_001u128),
                &ETH_USDC,
                &COSMOS_USDC
            )
            .unwrap_err(),
            ContractError::InsufficientPoolAsset {
                required: Coin::new(70_001, COSMOS_USDC),
                available: Coin::new(70_000, COSMOS_USDC)
            }
        );
    }

    #[test]
    fn test_transmute_fail_token_in_not_allowed() {
        let mut pool =
            TransmuterPool::new(Asset::unchecked_equal_assets(&[ETH_USDC, COSMOS_USDC])).unwrap();

        pool.join_pool(&[Coin::new(70_000, COSMOS_USDC)]).unwrap();
        assert_eq!(
            pool.transmute(
                AmountConstraint::exact_in(70_000u128),
                "urandom",
                COSMOS_USDC
            )
            .unwrap_err(),
            ContractError::InvalidTransmuteDenom {
                denom: "urandom".to_string(),
                expected_denom: vec![ETH_USDC.to_string(), COSMOS_USDC.to_string()]
            }
        );
    }

    #[test]
    fn test_transmute_fail_token_out_denom_not_allowed() {
        let mut pool =
            TransmuterPool::new(Asset::unchecked_equal_assets(&[ETH_USDC, COSMOS_USDC])).unwrap();

        pool.join_pool(&[Coin::new(70_000, COSMOS_USDC)]).unwrap();
        assert_eq!(
            pool.transmute(
                AmountConstraint::exact_in(70_000u128),
                &COSMOS_USDC,
                "urandom2"
            )
            .unwrap_err(),
            ContractError::InvalidTransmuteDenom {
                denom: "urandom2".to_string(),
                expected_denom: vec![ETH_USDC.to_string(), COSMOS_USDC.to_string()]
            }
        );
    }

    #[test]
    fn test_transmute_with_normalization_factor_10_power_n() {
        let mut deps = mock_dependencies();
        deps.querier.update_balance(
            "creator",
            vec![
                Coin::new(70_000 * 10u128.pow(14), NBTC_SAT),
                Coin::new(70_000 * 10u128.pow(8), WBTC_SAT),
            ],
        );

        let mut pool = TransmuterPool::new(vec![
            AssetConfig {
                denom: NBTC_SAT.to_string(),                        // exponent = 14
                normalization_factor: Uint128::from(10u128.pow(6)), // 10^14 / 10^6 = 10^8
            }
            .checked_init_asset(deps.as_ref())
            .unwrap(),
            AssetConfig {
                denom: WBTC_SAT.to_string(),          // exponent = 8
                normalization_factor: Uint128::one(), // 10^8 / 10^0 = 10^8
            }
            .checked_init_asset(deps.as_ref())
            .unwrap(),
        ])
        .unwrap();

        pool.join_pool(&[Coin::new(70_000 * 10u128.pow(14), NBTC_SAT)])
            .unwrap();

        assert_eq!(
            pool.transmute(
                AmountConstraint::exact_in(70_000 * 10u128.pow(8)),
                &WBTC_SAT,
                &NBTC_SAT
            )
            .unwrap(),
            Coin::new(70_000 * 10u128.pow(14), NBTC_SAT)
        );

        assert_eq!(
            pool.pool_assets
                .iter()
                .map(|asset: &'_ Asset| -> (u128, &'_ str) {
                    (asset.amount().u128(), asset.denom())
                })
                .collect::<Vec<_>>(),
            vec![(0, NBTC_SAT), (70_000 * 10u128.pow(8), WBTC_SAT),]
        );
    }
}
