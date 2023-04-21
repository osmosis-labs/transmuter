# Transmuter

![CI](https://github.com/osmosis-labs/transmuter/actions/workflows/rust.yml/badge.svg)

A CosmWasm contract to enable 1-direction 1:1 conversion of one asset to another.

For more information about the contract, please refer to [this document](./contracts/transmuter/README.md).

## Interacting with the contract via beaker console on testnet

First, make sure [beaker](https://github.com/osmosis-labs/beaker#installation) is installed.

Current testnet contract is deployed via the following command:

```sh
# no need to run this to play along with the rest of the guide
# since it has already been deployed
beaker wasm deploy transmuter --signer-account test1 --network testnet
```

You can connect beaker console to testnet with the following command:

```sh
beaker console --network testnet
```

This will connect with the contract that is deployed on testnet. The reference to the contract address and code id can be found in [.beaker/state.json](.beaker/state.json).
Instantiate message can be found [here](contracts/transmuter/instantiate-msgs/default.json).

### Setup

```js
osmo_denom = "uosmo";
xosmo_denom = "factory/osmo1cyyzpxplxdzkeea7kwsydadg87357qnahakaks/uxosmo";
provider = test1;
user = test2;

transmuter_provider = transmuter.signer(provider);
transmuter_user = transmuter.signer(user);
```

### Checking the contract state

checking pool liquidity

```js
await transmuter.getTotalPoolLiquidity()
```

checking shares for each address

```js
await transmuter.getShares({ address: provider.address });
await transmuter.getShares({ address: user.address });
```

checking total shares

```js
await transmuter.getTotalShares();
```

### Join pool

```js
await transmuter_provider.joinPool(
  "auto", // gas
  undefined, // memo
  [{ amount: "1000000", denom: xosmo_denom }] // funds
);
```

### Transmute

Transmutation is a 1-direction 1:1 conversion of one asset to another.

For this, we keep the interface for transmute to follow the same convention with `swapExactAmountIn` and `swapExactAmountOut` from `poolmanager` module as we will integrate this contract with `poolmanager` module in the future. (see [future work](#future-work))

```js
await transmuter_user.swapExactAmountIn(
  {
    tokenIn: { amount: "200000", denom: osmo_denom },
    tokenOutDenom: xosmo_denom,
    tokenOutMinAmount: "200000",
  }, // argument
  "auto", // gas
  undefined, // memo
  [{ amount: "200000", denom: osmo_denom }] // funds
);
```

or equivalently

```js
await transmuter_user.swapExactAmountOut(
  {
    tokenInDenom: osmo_denom,
    tokenInMaxAmount: "200000",
    tokenOut: { amount: "200000", denom: xosmo_denom }
  },
  "auto", // gas
  undefined, // memo
  [{ amount: "200000", denom: osmo_denom }] // funds
)
```

### Exit pool

Exit pool

```js
await transmuter_provider.exitPool({
  tokensOut: [
    { amount: "100000", denom: xosmo_denom },
    { amount: "100000", denom: osmo_denom },
  ],
});
```

## Future work

This contract is intended to be able to plug into Osmosis as a CosmWasm pool type and abstract parts of it's interaction through [`poolmanager` module](https://github.com/osmosis-labs/osmosis/tree/main/x/poolmanager).
To transmute, it will go through `poolmanager`'s `MsgSwapExactAmountIn` and `MsgSwapExactAmountOut` messages and will route the calls through sudo endpoint of the contract.

This is still a work in progress.
