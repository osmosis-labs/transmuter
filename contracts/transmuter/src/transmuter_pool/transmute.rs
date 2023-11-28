use cosmwasm_std::{ensure, Coin, Uint128};

use crate::{
    asset::{Asset, Rounding},
    ContractError,
};

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
    pub fn transmute(
        &mut self,
        amount_constraint: AmountConstraint,
        token_in_denom: &str,
        token_out_denom: &str,
    ) -> Result<Coin, ContractError> {
        let token_in_pool_asset = self.get_pool_asset_by_denom(&token_in_denom)?;
        let token_out_pool_asset = self.get_pool_asset_by_denom(token_out_denom)?;

        // calculate token in and token out amount based on normalized value
        let token_out_amount = self.calc_token_out_amount(
            token_in_pool_asset,
            token_out_pool_asset,
            &amount_constraint,
        )?;

        let token_in_amount = self.calc_token_in_amount(
            token_in_pool_asset,
            token_out_pool_asset,
            &amount_constraint,
        )?;

        let token_in = Coin::new(token_in_amount.u128(), token_in_denom);
        let token_out = Coin::new(token_out_amount.u128(), token_out_denom);

        // ensure there is enough token_out_denom in the pool
        ensure!(
            token_out_pool_asset.amount() >= token_out_amount,
            ContractError::InsufficientPoolAsset {
                required: token_out,
                available: token_out_pool_asset.to_coin()
            }
        );

        self.update_pool_assets(&token_in, &token_out)?;

        Ok(token_out)
    }

    /// update pool assets based on assets flow
    /// increase amount on token in
    /// decrease amount on token out
    fn update_pool_assets(
        &mut self,
        token_in: &Coin,
        token_out: &Coin,
    ) -> Result<(), ContractError> {
        for pool_asset in &mut self.pool_assets {
            // increase token in from pool assets
            if token_in.denom == pool_asset.denom() {
                pool_asset.increase_amount(token_in.amount)?;
            }

            // decrease token out from pool assets
            if token_out.denom == pool_asset.denom() {
                pool_asset.decrease_amount(token_out.amount)?;
            }
        }

        Ok(())
    }

    // This function calculates the amount of token_out for a transmutation.
    // The calculation depends on the amount constraint:
    // - If the constraint is ExactIn, the function converts the in amount to the equivalent out amount.
    //   This conversion takes into account the normalization factor of the tokens to ensure value consistency.
    //   The function rounds down the result to ensure that the out value <= in value.
    // - If the constraint is ExactOut, the function simply returns the out amount.
    fn calc_token_out_amount(
        &self,
        token_in_pool_asset: &Asset,
        token_out_pool_asset: &Asset,
        amount_constraint: &AmountConstraint,
    ) -> Result<Uint128, ContractError> {
        let token_out_amount = match amount_constraint {
            AmountConstraint::ExactIn(in_amount) => token_in_pool_asset.convert_amount(
                in_amount.to_owned(),
                token_out_pool_asset.normalization_factor(),
                Rounding::DOWN,
            )?,
            AmountConstraint::ExactOut(out_amount) => out_amount.to_owned(),
        };

        Ok(token_out_amount)
    }

    // This function calculates the amount of token_in required for a transmutation.
    // The calculation depends on the amount constraint:
    // - If the constraint is ExactIn, the function simply returns the in amount.
    // - If the constraint is ExactOut, the function converts the out amount to the equivalent in amount.
    //   This conversion takes into account the normalization factor of the tokens to ensure value consistency.
    //   The function rounds up the result to ensure that the in value >= out value.
    fn calc_token_in_amount(
        &self,
        token_in_pool_asset: &Asset,
        token_out_pool_asset: &Asset,
        amount_constraint: &AmountConstraint,
    ) -> Result<Uint128, ContractError> {
        let token_in_amount = match amount_constraint {
            AmountConstraint::ExactIn(in_amount) => in_amount.to_owned(),
            AmountConstraint::ExactOut(out_amount) => token_out_pool_asset.convert_amount(
                out_amount.to_owned(),
                token_in_pool_asset.normalization_factor(),
                Rounding::UP,
            )?,
        };

        Ok(token_in_amount)
    }

    fn get_pool_asset_by_denom(&self, denom: &'_ str) -> Result<&'_ Asset, ContractError> {
        self.pool_assets
            .iter()
            .find(|pool_asset| pool_asset.denom() == denom)
            .ok_or_else(|| ContractError::InvalidTransmuteDenom {
                denom: denom.to_string(),
                expected_denom: self
                    .pool_assets
                    .iter()
                    .map(|pool_asset| pool_asset.denom().to_string())
                    .collect(),
            })
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
    fn test_transmute_exact_in_succeed() {
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
    fn test_transmute_exact_out_succeed() {
        let mut pool =
            TransmuterPool::new(Asset::unchecked_equal_assets(&[ETH_USDC, COSMOS_USDC])).unwrap();

        pool.join_pool(&[Coin::new(170_000, COSMOS_USDC)]).unwrap();

        assert_eq!(
            pool.transmute(
                AmountConstraint::exact_out(70_000u128),
                &ETH_USDC,
                &COSMOS_USDC
            )
            .unwrap(),
            Coin::new(70_000, COSMOS_USDC)
        );

        assert_eq!(
            pool.transmute(AmountConstraint::exact_out(0u128), &ETH_USDC, &COSMOS_USDC)
                .unwrap(),
            Coin::new(0, COSMOS_USDC)
        );

        assert_eq!(
            pool.pool_assets,
            Asset::unchecked_equal_assets_from_coins(&[
                Coin::new(70_000, ETH_USDC),
                Coin::new(100_000, COSMOS_USDC),
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

    #[test]
    fn test_transmute_exact_in_round_down_token_out() {
        let mut deps = mock_dependencies();
        // a:b = 1:3
        deps.querier.update_balance(
            "creator",
            vec![
                Coin::new(70_000 * 3 * 10u128.pow(14), "ua"),
                Coin::new(70_000 * 10u128.pow(8), "ub"),
            ],
        );

        let mut pool = TransmuterPool::new(vec![
            AssetConfig {
                denom: "ua".to_string(),
                normalization_factor: Uint128::from(3 * 10u128.pow(6)),
            }
            .checked_init_asset(deps.as_ref())
            .unwrap(),
            AssetConfig {
                denom: "ub".to_string(),
                normalization_factor: Uint128::one(),
            }
            .checked_init_asset(deps.as_ref())
            .unwrap(),
        ])
        .unwrap();

        pool.join_pool(&[Coin::new(70_000 * 10u128.pow(8), "ub")])
            .unwrap();

        // Transmute with ExactIn, where the output needs to be rounded down
        let result = pool
            .transmute(
                AmountConstraint::exact_in(3 * 10u128.pow(14) + 1), // Add 1 to trigger rounding
                &"ua",
                &"ub",
            )
            .unwrap();

        // Check that the output is as expected, rounded down
        assert_eq!(result, Coin::new(10u128.pow(8), "ub"));

        let result = pool
            .transmute(
                AmountConstraint::exact_in(3 * 10u128.pow(14) - 1), // Sub 1 to trigger rounding
                &"ua",
                &"ub",
            )
            .unwrap();

        // Check that the output is as expected, rounded down
        assert_eq!(result, Coin::new(10u128.pow(8) - 1, "ub"));
    }

    #[test]
    fn test_transmute_exact_out_round_up_token_in() {
        let mut deps = mock_dependencies();
        // a:b = 1:3
        deps.querier.update_balance(
            "creator",
            vec![
                Coin::new(70_000 * 3 * 10u128.pow(14), "ua"),
                Coin::new(70_000 * 10u128.pow(8), "ub"),
            ],
        );

        let mut pool = TransmuterPool::new(vec![
            AssetConfig {
                denom: "ua".to_string(),
                normalization_factor: Uint128::from(3 * 10u128.pow(6)),
            }
            .checked_init_asset(deps.as_ref())
            .unwrap(),
            AssetConfig {
                denom: "ub".to_string(),
                normalization_factor: Uint128::one(),
            }
            .checked_init_asset(deps.as_ref())
            .unwrap(),
        ])
        .unwrap();

        pool.join_pool(&[Coin::new(70_000 * 3 * 10u128.pow(14), "ua")])
            .unwrap();

        // Transmute with ExactOut, where the input needs to be rounded up
        let result = pool
            .transmute(
                AmountConstraint::exact_out(3 * 10u128.pow(14) - 1), // Sub 1 to trigger rounding
                &"ub",
                &"ua",
            )
            .unwrap();

        // Check that output is exact
        assert_eq!(result, Coin::new(3 * 10u128.pow(14) - 1, "ua"));

        let updated_ub = pool
            .pool_assets
            .iter()
            .find(|asset| asset.denom() == "ub")
            .unwrap()
            .amount();

        // Check that the input is as expected, rounded up
        assert_eq!(updated_ub, Uint128::from(10u128.pow(8)));

        // Transmute with ExactOut, where the input needs to be rounded up
        let result = pool
            .transmute(
                AmountConstraint::exact_out(3 * 10u128.pow(14) + 1), // Add 1 to trigger rounding
                &"ub",
                &"ua",
            )
            .unwrap();

        // Check that output is exact
        assert_eq!(result, Coin::new(3 * 10u128.pow(14) + 1, "ua"));

        let updated_ub = pool
            .pool_assets
            .iter()
            .find(|asset| asset.denom() == "ub")
            .unwrap()
            .amount()
            - updated_ub;

        // Check that the input is as expected, rounded up
        assert_eq!(updated_ub, Uint128::from(10u128.pow(8) + 1));
    }
}
