use cosmwasm_std::{Addr, Coin, Deps, StdResult, Storage, Uint128};
use cw_storage_plus::Item;

use crate::{
    asset::{convert_amount, Rounding},
    ContractError,
};

// TODO: make this a state var
const ALLOYED_DENOM_NORMALIZATION_FACTOR: Uint128 = Uint128::one();

/// Alloyed asset represents the shares of the pool
/// and since the pool is a 1:1 multi-asset pool, it act
/// as a composite of the underlying assets and assume 1:1
/// value to the underlying assets.
pub struct AlloyedAsset<'a> {
    alloyed_denom: Item<'a, String>,
}

impl<'a> AlloyedAsset<'a> {
    pub const fn new(alloyed_denom_namespace: &'a str) -> Self {
        Self {
            alloyed_denom: Item::new(alloyed_denom_namespace),
        }
    }

    /// get the alloyed denom
    pub fn get_alloyed_denom(&self, store: &dyn Storage) -> StdResult<String> {
        self.alloyed_denom.load(store)
    }

    /// set the alloyed denom
    pub fn set_alloyed_denom(
        &self,
        store: &mut dyn Storage,
        alloyed_denom: &String,
    ) -> StdResult<()> {
        self.alloyed_denom.save(store, alloyed_denom)
    }

    /// get the total supply of alloyed asset
    /// which is the total shares of the pool
    pub fn get_total_supply(&self, deps: Deps) -> StdResult<Uint128> {
        let alloyed_denom = self.get_alloyed_denom(deps.storage)?;

        deps.querier
            .query_supply(alloyed_denom)
            .map(|coin| coin.amount)
    }

    /// get the balance of alloyed asset for a given address
    pub fn get_balance(&self, deps: Deps, address: &Addr) -> StdResult<Uint128> {
        let alloyed_denom = self.get_alloyed_denom(deps.storage)?;

        deps.querier
            .query_balance(address, alloyed_denom)
            .map(|coin| coin.amount)
    }

    /// calculate the amount of alloyed asset to mint/burn
    /// `tokens` is a slice of (coin, normalization factor) pair
    pub fn amount_from(
        tokens: &[(Coin, Uint128)],
        rounding: Rounding,
    ) -> Result<Uint128, ContractError> {
        let mut total = Uint128::zero();
        for (coin, coin_normalization_factor) in tokens {
            total = total.checked_add(convert_amount(
                coin.amount,
                *coin_normalization_factor,
                ALLOYED_DENOM_NORMALIZATION_FACTOR,
                &rounding,
            )?)?;
        }
        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use cosmwasm_std::testing::mock_dependencies;

    use super::*;

    #[test]
    fn test_alloyed_assets_balance_and_supply() {
        let alloyed_assets = AlloyedAsset::new("alloyed_assets");
        let mut deps = mock_dependencies();

        let alloyed_denom = "alloyed_denom".to_string();
        alloyed_assets
            .set_alloyed_denom(&mut deps.storage, &alloyed_denom)
            .unwrap();

        deps.querier.update_balance(
            "osmo1addr1",
            vec![Coin {
                denom: alloyed_denom.clone(),
                amount: Uint128::from(400_000_000_000_000_000_000u128),
            }],
        );

        deps.querier.update_balance(
            "osmo1addr2",
            vec![Coin {
                denom: alloyed_denom.clone(),
                amount: Uint128::from(600_000_000_000_000_000_000u128),
            }],
        );

        assert_eq!(
            alloyed_assets
                .get_balance(deps.as_ref(), &Addr::unchecked("osmo1addr1"))
                .unwrap(),
            Uint128::from(400_000_000_000_000_000_000u128)
        );

        assert_eq!(
            alloyed_assets
                .get_balance(deps.as_ref(), &Addr::unchecked("osmo1addr2"))
                .unwrap(),
            Uint128::from(600_000_000_000_000_000_000u128)
        );

        assert_eq!(
            alloyed_assets.get_total_supply(deps.as_ref()).unwrap(),
            Uint128::from(1_000_000_000_000_000_000_000u128)
        );
    }

    #[test]
    fn test_amount_from() {
        let alloyed_assets = AlloyedAsset::new("alloyed_assets");
        let mut deps = mock_dependencies();

        let alloyed_denom = "alloyed_denom".to_string();
        alloyed_assets
            .set_alloyed_denom(&mut deps.storage, &alloyed_denom)
            .unwrap();

        // same normalization factor
        let amount = AlloyedAsset::amount_from(
            &[(Coin::new(100, "ua"), ALLOYED_DENOM_NORMALIZATION_FACTOR)],
            Rounding::Up,
        )
        .unwrap();

        assert_eq!(amount, Uint128::from(100u128));

        // different normalization factor
        let amount = AlloyedAsset::amount_from(
            &[
                (Coin::new(100, "ua"), Uint128::from(2u128)),
                (Coin::new(100, "ub"), Uint128::from(3u128)),
            ],
            Rounding::Up,
        );

        assert_eq!(amount.unwrap(), Uint128::from(84u128));

        let amount = AlloyedAsset::amount_from(
            &[
                (Coin::new(100, "ua"), Uint128::from(2u128)),
                (Coin::new(100, "ub"), Uint128::from(3u128)),
            ],
            Rounding::Down,
        );

        assert_eq!(amount.unwrap(), Uint128::from(83u128));
    }
}
