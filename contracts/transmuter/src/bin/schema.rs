#![cfg(not(tarpaulin_include))]

use cosmwasm_schema::write_api;

use transmuter::contract::sv::{ExecMsg, InstantiateMsg, QueryMsg};

fn main() {
    write_api! {
        instantiate: InstantiateMsg,
        execute: ExecMsg,
        query: QueryMsg,
    }
}
