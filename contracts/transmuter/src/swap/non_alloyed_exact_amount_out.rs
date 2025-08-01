use cosmwasm_std::{to_json_binary, Addr, BankMsg, Coin, Deps, DepsMut, Response, Uint128};

use crate::{
    contract::Transmuter,
    swap::{common::SwapExactAmountOutResponseData, rebalancing_adjustment_for_exact_out},
    transmuter_pool::TransmuterPool,
    ContractError,
};

impl Transmuter {
    pub fn swap_non_alloyed_exact_amount_out(
        &self,
        token_in_denom: &str,
        token_in_max_amount: Uint128,
        token_out: Coin,
        sender: Addr,
        mut deps: DepsMut,
    ) -> Result<Response, ContractError> {
        let pool = self.pool.load(deps.storage)?;
        let std_norm_factor = pool.std_norm_factor()?;
        let token_in_norm_factor = pool
            .get_pool_asset_by_denom(token_in_denom)?
            .normalization_factor();

        let run_pool = |deps: Deps, pool: TransmuterPool| {
            self.in_amt_given_out(deps, pool, token_out.clone(), token_in_denom.to_string())
        };

        let rebalancing_adjustment = rebalancing_adjustment_for_exact_out(
            token_in_max_amount,
            std_norm_factor,
            token_in_norm_factor,
        );

        let (mut pool, actual_token_in, _adjustment) = self.rebalancer_pass(
            deps.branch(),
            pool,
            &sender,
            run_pool,
            rebalancing_adjustment,
        )?;

        self.clean_up_drained_corrupted_assets(deps.storage, &mut pool)?;

        // save pool
        self.pool.save(deps.storage, &pool)?;

        let send_token_out_to_sender_msg = BankMsg::Send {
            to_address: sender.to_string(),
            amount: vec![token_out],
        };

        let swap_result = SwapExactAmountOutResponseData {
            token_in_amount: actual_token_in.amount,
        };

        Ok(Response::new()
            .add_message(send_token_out_to_sender_msg)
            .set_data(to_json_binary(&swap_result)?))
    }
}
