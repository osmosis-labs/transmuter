// Ignore integration tests for code coverage since there will be problems with dynamic linking libosmosistesttube
// and also, tarpaulin will not be able read coverage out of wasm binary anyway
#![cfg(all(not(tarpaulin), not(feature = "skip-integration-test")))]

mod cases;
mod modules;
mod test_env;
