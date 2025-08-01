use cosmwasm_std::{to_json_binary, Response, StdError, Uint128};
use serde::Serialize;

use cosmwasm_schema::cw_serde;

pub enum Entrypoint {
    Exec,
    Sudo,
}

pub fn set_data_if_sudo<T>(
    response: Response,
    entrypoint: &Entrypoint,
    data: &T,
) -> Result<Response, StdError>
where
    T: Serialize + ?Sized,
{
    Ok(match entrypoint {
        Entrypoint::Sudo => response.set_data(to_json_binary(data)?),
        Entrypoint::Exec => response,
    })
}

#[cw_serde]
/// Fixing token in amount makes token amount out varies
pub struct SwapExactAmountInResponseData {
    pub token_out_amount: Uint128,
}

#[cw_serde]
/// Fixing token out amount makes token amount in varies
pub struct SwapExactAmountOutResponseData {
    pub token_in_amount: Uint128,
}
