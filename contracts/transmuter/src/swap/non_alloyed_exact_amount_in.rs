use cosmwasm_std::{to_json_binary, Addr, BankMsg, Coin, Deps, DepsMut, Response, Uint128};

use crate::{
    contract::Transmuter,
    swap::{common::SwapExactAmountInResponseData, rebalancing_adjustment_for_exact_in},
    transmuter_pool::TransmuterPool,
    ContractError,
};
impl Transmuter {
    pub fn swap_non_alloyed_exact_amount_in(
        &self,
        token_in: Coin,
        token_out_denom: &str,
        token_out_min_amount: Uint128,
        sender: Addr,
        mut deps: DepsMut,
    ) -> Result<Response, ContractError> {
        let pool = self.pool.load(deps.storage)?;
        let std_norm_factor = pool.std_norm_factor()?;
        let token_out_norm_factor = pool
            .get_pool_asset_by_denom(token_out_denom)?
            .normalization_factor();

        let run_pool = |deps: Deps, pool: TransmuterPool| {
            self.out_amt_given_in(deps, pool, token_in, token_out_denom)
        };

        let rebalancing_adjustment = rebalancing_adjustment_for_exact_in(
            token_out_min_amount,
            std_norm_factor,
            token_out_norm_factor,
        );

        let (mut pool, actual_token_out, _adjustment) = self.rebalancer_pass(
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
            amount: vec![actual_token_out.clone()],
        };

        let swap_result = SwapExactAmountInResponseData {
            token_out_amount: actual_token_out.amount,
        };

        Ok(Response::new()
            .add_message(send_token_out_to_sender_msg)
            .set_data(to_json_binary(&swap_result)?))
    }
}
