use cosmwasm_std::{coin, ensure, Addr, Coin, Deps, DepsMut, Env, Response, Uint128};
use osmosis_std::types::osmosis::tokenfactory::v1beta1::MsgMint;

use crate::{
    alloyed_asset::swap_to_alloyed,
    contract::Transmuter,
    swap::{
        common::{
            set_data_if_sudo, Adjustment, Entrypoint, SwapExactAmountInResponseData,
            SwapExactAmountOutResponseData,
        },
        rebalancing_adjustment_for_exact_in, rebalancing_adjustment_for_exact_out,
    },
    transmuter_pool::TransmuterPool,
    ContractError,
};

#[derive(Debug)]
pub enum SwapToAlloyedConstraint<'a> {
    ExactIn {
        tokens_in: &'a [Coin],
        token_out_min_amount: Uint128,
    },
    ExactOut {
        token_in_denom: &'a str,
        token_in_max_amount: Uint128,
        token_out_amount: Uint128,
    },
}

impl Transmuter {
    /// Swap tokens to alloyed asset. (eg. swap nBTC -> allBTC or join pool pool nBTC, wBTC -> allBTC)
    ///
    /// Send token to the contract and mint equal value of alloyed asset to the sender.
    pub fn swap_tokens_to_alloyed_asset(
        &self,
        entrypoint: Entrypoint,
        constraint: SwapToAlloyedConstraint,
        sender: Addr,
        mut deps: DepsMut,
        env: Env,
    ) -> Result<Response, ContractError> {
        let alloyed_denom = self.alloyed_asset.get_alloyed_denom(deps.storage)?;
        let alloyed_norm_factor = self.alloyed_asset.get_normalization_factor(deps.storage)?;

        let (pool, tokens_in, out_amount, response) = match constraint {
            SwapToAlloyedConstraint::ExactIn {
                tokens_in,
                token_out_min_amount,
            } => self.swap_tokens_to_alloyed_asset_exact_in(
                entrypoint,
                &sender,
                tokens_in.to_owned(),
                token_out_min_amount,
                alloyed_denom.clone(),
                alloyed_norm_factor,
                deps.branch(),
                &env,
            )?,

            SwapToAlloyedConstraint::ExactOut {
                token_in_denom,
                token_in_max_amount,
                token_out_amount,
            } => self.swap_tokens_to_alloyed_asset_exact_out(
                entrypoint,
                &sender,
                token_in_denom,
                token_in_max_amount,
                token_out_amount,
                deps.branch(),
            )?,
        };

        // ensure funds not empty
        ensure!(
            !tokens_in.is_empty(),
            ContractError::AtLeastSingleTokenExpected {}
        );

        // ensure funds does not have zero coin
        ensure!(
            tokens_in.iter().all(|coin| coin.amount > Uint128::zero()),
            ContractError::ZeroValueOperation {}
        );

        // no need for cleaning up drained corrupted assets here
        // since this function will only adding more underlying assets
        // rather than removing any of them

        self.pool.save(deps.storage, &pool)?;

        let alloyed_asset_mint_msg = create_alloyed_asset_mint_msg(
            out_amount,
            alloyed_denom,
            &sender,
            &env.contract.address,
        );

        Ok(response.add_message(alloyed_asset_mint_msg))
    }

    fn swap_tokens_to_alloyed_asset_exact_in(
        &self,
        entrypoint: Entrypoint,
        sender: &Addr,
        tokens_in: Vec<Coin>,
        token_out_min_amount: Uint128,
        alloyed_denom: String,
        alloyed_norm_factor: Uint128,
        mut deps: DepsMut,
        env: &Env,
    ) -> Result<(TransmuterPool, Vec<Coin>, Uint128, Response), ContractError> {
        let pool: TransmuterPool = self.pool.load(deps.storage)?;
        let response = Response::new();

        let std_norm_factor = pool.std_norm_factor()?;
        let token_out_norm_factor = alloyed_norm_factor;

        let tokens_in_with_norm_factor = pool.pair_coins_with_normalization_factor(&tokens_in)?;
        let out_amount_before_fee = swap_to_alloyed::out_amount_via_exact_in(
            tokens_in_with_norm_factor,
            token_out_min_amount,
            self.alloyed_asset.get_normalization_factor(deps.storage)?,
        )?;

        let run_pool = |_deps: Deps, mut pool: TransmuterPool| {
            pool.join_pool(&tokens_in)?;
            let token_out = coin(out_amount_before_fee.u128(), alloyed_denom.clone());

            Ok((pool, token_out))
        };

        let rebalancing_adjustment = rebalancing_adjustment_for_exact_in(
            token_out_min_amount,
            std_norm_factor,
            token_out_norm_factor,
        );

        let (pool, token_out, adjustment) = self.rebalancer_pass(
            deps.branch(),
            pool,
            &sender,
            run_pool,
            rebalancing_adjustment,
        )?;

        let response = set_data_if_sudo(
            response,
            &entrypoint,
            &SwapExactAmountInResponseData {
                token_out_amount: token_out.amount,
            },
        )?;

        let response = match adjustment {
            // fee is deducted from the minting token out, mint directly to the contract as it's already recorded as such in the incentive pool
            Adjustment::DeductFee { fee } => response.add_message(MsgMint {
                sender: env.contract.address.to_string(),
                amount: Some(fee.into()),
                mint_to_address: env.contract.address.to_string(),
            }),
            // incentive doesn't require minting or burning anything
            Adjustment::CreditIncentive { .. } => response,
            Adjustment::None => response,
        };

        Ok((pool, tokens_in, token_out.amount, response))
    }

    fn swap_tokens_to_alloyed_asset_exact_out(
        &self,
        entrypoint: Entrypoint,
        sender: &Addr,
        token_in_denom: &str,
        token_in_max_amount: Uint128,
        token_out_amount: Uint128,
        mut deps: DepsMut,
    ) -> Result<(TransmuterPool, Vec<Coin>, Uint128, Response), ContractError> {
        let pool: TransmuterPool = self.pool.load(deps.storage)?;
        let response = Response::new();

        let std_norm_factor = pool.std_norm_factor()?;
        let token_in_norm_factor = pool
            .get_pool_asset_by_denom(token_in_denom)?
            .normalization_factor();
        let in_amount_before_fee = swap_to_alloyed::in_amount_via_exact_out(
            token_in_norm_factor,
            token_in_max_amount,
            token_out_amount,
            self.alloyed_asset.get_normalization_factor(deps.storage)?,
        )?;
        let token_in = coin(in_amount_before_fee.u128(), token_in_denom);

        let run_pool = |_deps: Deps, mut pool: TransmuterPool| {
            pool.join_pool(&[token_in.clone()])?;
            Ok((pool, token_in))
        };

        let rebalancing_adjustment = rebalancing_adjustment_for_exact_out(
            token_in_max_amount,
            std_norm_factor,
            token_in_norm_factor,
        );

        let (pool, token_in, _adjustment) = self.rebalancer_pass(
            deps.branch(),
            pool,
            &sender,
            run_pool,
            rebalancing_adjustment,
        )?;

        // Unlike exact in case where token out is alloyed, there is no separate mint target required here
        // Because fee is collected from the token_in
        let response = set_data_if_sudo(
            response,
            &entrypoint,
            &SwapExactAmountOutResponseData {
                token_in_amount: token_in.amount,
            },
        )?;

        Ok((pool, vec![token_in], token_out_amount, response))
    }
}

/// Create MsgMint for minting alloyed asset.
fn create_alloyed_asset_mint_msg(
    out_amount: Uint128,
    alloyed_denom: String,
    mint_to_address: &Addr,
    contract_address: &Addr,
) -> MsgMint {
    let alloyed_asset_out = coin(out_amount.u128(), alloyed_denom);

    MsgMint {
        sender: contract_address.to_string(),
        amount: Some(alloyed_asset_out.into()),
        mint_to_address: mint_to_address.to_string(),
    }
}
