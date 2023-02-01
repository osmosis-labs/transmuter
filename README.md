# Transmuter

![CI](https://github.com/osmosis-labs/transmuter/actions/workflows/rust.yml/badge.svg)

A CosmWasm contract to enable 1-direction 1:1 conversion of one asset to another.

For more information about the contract, please refer to [this document](./contracts/transmuter/README.md).

## Interacting with the contract via beaker console on testnet

Make sure [beaker](https://github.com/osmosis-labs/beaker#installation) is installed. Then:

```sh
beaker console --network testnet
```

This will connect with the contract that is deployed on testnet. The reference to the contract address and code id can be found in [.beaker/state.json](.beaker/state.json).
Instantiate message can be found [here](./contracts/transmuter/instantiate-msg/default.json).

### Setup

```js
in_denom = "uosmo";
out_denom = "factory/osmo1cyyzpxplxdzkeea7kwsydadg87357qnahakaks/uxosmo";
admin = test1;
user = test2;

transmuter_admin = transmuter.signer(admin);
transmuter_user = transmuter.signer(user);
```

### Checking the contract state

checking pool

```js
await transmuter.pool();
```

checking admin

```js
await transmuter.admin();
```

### Supplying out_denom reserve

```js
// supply(gas, memo, funds)
await transmuter_admin.supply("auto", undefined, [
  { amount: "1000000", denom: out_denom },
]);
```

### Transmute

```js
// transmute(gas, memo, funds)
await transmuter_user.transmute("auto", undefined, [
  { amount: "200000", denom: in_denom },
]);
```

### Withdraw

Only admin can withdraw

```js
await transmuter_admin.withdraw({
  coins: [
    { amount: "100000", denom: out_denom },
    { amount: "100000", denom: in_denom },
  ],
});
```

## Future work

This contract is intended to be able to plug into Osmosis as a CosmWasm pool type and abstract parts of it's interaction through [`poolmanager` module](https://github.com/osmosis-labs/osmosis/tree/main/x/poolmanager).
To transmute, it will go through `poolmanager`'s `MsgSwapExactAmountIn` and `MsgSwapExactAmountOut` messages and will route the calls through sudo endpoint of the contract.

This is still a work in progress.
