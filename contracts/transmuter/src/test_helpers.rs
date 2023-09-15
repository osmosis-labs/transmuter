#![cfg(test)]
use std::marker::PhantomData;

use cosmwasm_std::{
    from_slice,
    testing::{MockApi, MockQuerier, MockStorage},
    to_binary, Binary, Coin, ContractResult, CustomQuery, Empty, OwnedDeps, Querier, QuerierResult,
    QueryRequest, SystemError, SystemResult,
};
use osmosis_std::types::cosmos::bank::v1beta1::{
    DenomUnit, Metadata, QueryDenomMetadataRequest, QueryDenomMetadataResponse,
};
use serde::de::DeserializeOwned;

type StargateHandler = Box<dyn for<'a> Fn(&'a String, &'a Binary) -> ContractResult<Binary>>;

pub struct StargateMockQuerier<C: DeserializeOwned = Empty> {
    stargate_handler: StargateHandler,
    mock_querier: MockQuerier<C>,
}

impl<C: DeserializeOwned> StargateMockQuerier<C> {
    pub fn new() -> Self {
        Self {
            stargate_handler: Box::new(|_, _| {
                panic!("This should never be called. Use the update_stargate method to set it")
            }),
            mock_querier: MockQuerier::<C>::new(&[]),
        }
    }

    // set a new balance for the given address and return the old balance
    pub fn update_balance(
        &mut self,
        addr: impl Into<String>,
        balance: Vec<Coin>,
    ) -> Option<Vec<Coin>> {
        self.mock_querier.update_balance(addr, balance)
    }
}

impl<C: CustomQuery + DeserializeOwned> Querier for StargateMockQuerier<C> {
    fn raw_query(&self, bin_request: &[u8]) -> QuerierResult {
        let request: QueryRequest<C> = match from_slice(bin_request) {
            Ok(v) => v,
            Err(e) => {
                return SystemResult::Err(SystemError::InvalidRequest {
                    error: format!("Parsing query request: {}", e),
                    request: bin_request.into(),
                })
            }
        };
        self.handle_query(&request)
    }
}

impl<C: CustomQuery + DeserializeOwned> StargateMockQuerier<C> {
    pub fn update_stargate<H>(&mut self, stargate_handler: H)
    where
        H: Fn(&String, &Binary) -> ContractResult<Binary> + 'static,
    {
        self.stargate_handler = Box::from(stargate_handler);
    }

    pub fn handle_query(&self, request: &QueryRequest<C>) -> QuerierResult {
        match &request {
            QueryRequest::Stargate { path, data } => match (*self.stargate_handler)(path, data) {
                ok @ ContractResult::Ok(_) => SystemResult::Ok(ok),
                ContractResult::Err(error) => SystemResult::Err(SystemError::InvalidRequest {
                    error,
                    request: data.clone(),
                }),
            },
            _ => self.mock_querier.handle_query(request),
        }
    }
}

pub fn mock_dependencies_with_stargate_query(
) -> OwnedDeps<MockStorage, MockApi, StargateMockQuerier, Empty> {
    OwnedDeps {
        storage: MockStorage::new(),
        api: MockApi::default(),
        querier: StargateMockQuerier::new(),
        custom_query_type: PhantomData,
    }
}

pub fn pass_with_default_denom_metadata_handler(
    path: &String,
    data: &Binary,
) -> ContractResult<Binary> {
    if path == "/cosmos.bank.v1beta1.Query/DenomMetadata" {
        let request: QueryDenomMetadataRequest = TryFrom::try_from(data.clone()).unwrap();

        // if request.denom in existing_denoms
        // if existing_denoms.clone().contains(&request.denom) {
        ContractResult::Ok(
            to_binary(&QueryDenomMetadataResponse {
                metadata: Some(Metadata {
                    description: "".to_string(),
                    base: request.denom.clone(),
                    display: "".to_string(),
                    name: "".to_string(),
                    symbol: "".to_string(),
                    denom_units: vec![DenomUnit {
                        denom: request.denom,
                        exponent: 0,
                        aliases: vec![],
                    }],
                }),
            })
            .unwrap(),
        )
        // } else {
        //     ContractResult::Err(format!(
        //         "rpc error: code = NotFound desc = client metadata for denom {}",
        //         request.denom
        //     ))
        // }
    } else {
        unreachable!()
    }
}
