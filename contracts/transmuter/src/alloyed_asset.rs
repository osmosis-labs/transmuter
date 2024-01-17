use cosmwasm_std::{ensure, Addr, Coin, Deps, StdResult, Storage, Uint128};
use cw_storage_plus::Item;

use crate::{
    asset::{convert_amount, Rounding},
    ContractError,
};

/// Alloyed asset represents the shares of the pool
/// and since the pool is a 1:1 multi-asset pool, it act
/// as a composite of the underlying assets and assume 1:1
/// value to the underlying assets.
pub struct AlloyedAsset<'a> {
    alloyed_denom: Item<'a, String>,
    normalization_factor: Item<'a, Uint128>,
}

impl<'a> AlloyedAsset<'a> {
    pub const fn new(
        alloyed_denom_namespace: &'a str,
        normalization_factor_namespace: &'a str,
    ) -> Self {
        Self {
            alloyed_denom: Item::new(alloyed_denom_namespace),
            normalization_factor: Item::new(normalization_factor_namespace),
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

    /// get alloyed denom normalization factor
    pub fn get_normalization_factor(&self, store: &dyn Storage) -> StdResult<Uint128> {
        self.normalization_factor.load(store)
    }

    pub fn set_normalization_factor(
        &self,
        store: &mut dyn Storage,
        factor: Uint128,
    ) -> StdResult<()> {
        self.normalization_factor.save(store, &factor)
    }

    /// calculate the amount of alloyed asset to mint/burn
    /// `tokens` is a slice of (coin, normalization factor) pair
    pub fn amount_from(
        tokens: &[(Coin, Uint128)],
        alloyed_denom_normalization_factor: Uint128,
        rounding: Rounding,
    ) -> Result<Uint128, ContractError> {
        let mut total = Uint128::zero();
        for (coin, coin_normalization_factor) in tokens {
            total = total.checked_add(convert_amount(
                coin.amount,
                *coin_normalization_factor,
                alloyed_denom_normalization_factor,
                &rounding,
            )?)?;
        }
        Ok(total)
    }
}

pub mod swap_to_alloyed {
    use super::*;

    pub fn out_amount_via_exact_in(
        tokens_in_with_norm_factor: Vec<(Coin, Uint128)>,
        token_out_min_amount: Uint128,
        alloyed_denom_normalization_factor: Uint128,
    ) -> Result<Uint128, ContractError> {
        // swap token for alloyed asset output keeps alloyed asset value <= tokens_in value
        // if conversion has remainder to not over mint alloyed asset
        let out_amount = AlloyedAsset::amount_from(
            &tokens_in_with_norm_factor,
            alloyed_denom_normalization_factor,
            Rounding::Down,
        )?;

        ensure!(
            out_amount >= token_out_min_amount,
            ContractError::InsufficientTokenOut {
                min_required: token_out_min_amount,
                amount_out: out_amount
            }
        );

        Ok(out_amount)
    }

    /// With exact out, only one token in is allowed
    /// Since it needs to calculate the exact amount of token in
    /// returns token in amount
    pub fn in_amount_via_exact_out(
        token_in_norm_factor: Uint128,
        token_in_max_amount: Uint128,
        token_out_amount: Uint128,
        alloyed_denom_normalization_factor: Uint128,
    ) -> Result<Uint128, ContractError> {
        let token_in_amount = convert_amount(
            token_out_amount,
            alloyed_denom_normalization_factor,
            token_in_norm_factor,
            &Rounding::Up,
        )?;

        ensure!(
            token_in_amount <= token_in_max_amount,
            ContractError::ExcessiveRequiredTokenIn {
                limit: token_in_max_amount,
                required: token_in_amount
            }
        );

        Ok(token_in_amount)
    }
}

pub mod swap_from_alloyed {
    use super::*;

    pub fn out_amount_via_exact_in(
        amount_in: Uint128,
        alloyed_denom_normalization_factor: Uint128,
        token_out_norm_factor: Uint128,
        token_out_min_amount: Uint128,
    ) -> Result<Uint128, ContractError> {
        // swap token from alloyed asset output keeps token_out value <= alloyed asset burnt value (amount_in)
        // makes sure it's not under burning alloyed asset
        let out_amount = convert_amount(
            amount_in,
            alloyed_denom_normalization_factor,
            token_out_norm_factor,
            &Rounding::Down,
        )?;

        ensure!(
            out_amount >= token_out_min_amount,
            ContractError::InsufficientTokenOut {
                min_required: token_out_min_amount,
                amount_out: out_amount
            }
        );

        Ok(out_amount)
    }

    /// With exact out, only one token in is allowed
    /// Since it needs to calculate the exact amount of token in
    /// returns token in amount
    pub fn in_amount_via_exact_out(
        token_in_max_amount: Uint128,
        alloyed_denom_normalization_factor: Uint128,
        tokens_out_with_norm_factor: Vec<(Coin, Uint128)>,
    ) -> Result<Uint128, ContractError> {
        let token_in_amount = AlloyedAsset::amount_from(
            &tokens_out_with_norm_factor,
            alloyed_denom_normalization_factor,
            Rounding::Up,
        )?;

        ensure!(
            token_in_amount <= token_in_max_amount,
            ContractError::ExcessiveRequiredTokenIn {
                limit: token_in_max_amount,
                required: token_in_amount
            }
        );

        Ok(token_in_amount)
    }
}

#[cfg(test)]
mod tests {
    use cosmwasm_std::testing::mock_dependencies;

    use super::*;

    #[test]
    fn test_alloyed_assets_balance_and_supply() {
        let alloyed_assets =
            AlloyedAsset::new("alloyed_assets", "alloyed_assets_normalization_factor");
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
        let alloyed_assets =
            AlloyedAsset::new("alloyed_denom", "alloyed_denom_normalization_factor");
        let mut deps = mock_dependencies();

        let alloyed_denom = "alloyed_denom".to_string();
        alloyed_assets
            .set_alloyed_denom(&mut deps.storage, &alloyed_denom)
            .unwrap();

        // same normalization factor
        let amount = AlloyedAsset::amount_from(
            &[(Coin::new(100, "ua"), Uint128::one())],
            Uint128::one(),
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
            Uint128::one(),
            Rounding::Up,
        );

        assert_eq!(amount.unwrap(), Uint128::from(84u128));

        let amount = AlloyedAsset::amount_from(
            &[
                (Coin::new(100, "ua"), Uint128::from(2u128)),
                (Coin::new(100, "ub"), Uint128::from(3u128)),
            ],
            Uint128::one(),
            Rounding::Down,
        );

        assert_eq!(amount.unwrap(), Uint128::from(83u128));
    }
}
