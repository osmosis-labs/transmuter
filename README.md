# Transmuter


![CI](https://github.com/osmosis-labs/transmuter/actions/workflows/rust.yml/badge.svg)

A CosmWasm contract to enable 1-direction 1:1 conversion of one asset to another.

For more information about the contract, please refer to [this document](./contracts/transmuter/README.md).

## Future work

This contract is intended to be able to plug into Osmosis as a CosmWasm pool type and abstract parts of it's interaction through [`poolmanager` module](https://github.com/osmosis-labs/osmosis/tree/main/x/poolmanager).
To transmute, it will go through `poolmanager`'s `MsgSwapExactAmountIn` and `MsgSwapExactAmountOut` messages and will route the calls through sudo endpoint of the contract.

This is still a work in progress.
