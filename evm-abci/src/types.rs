use ethers::{
    abi::ethereum_types::Signature,
    types::{Address, TransactionRequest, U256},
};

use foundry_evm::revm::primitives::ExecutionResult;

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct RpcRequest {
    pub method: String,
    pub params: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Query {
    EthCall(TransactionRequest),
    Balance(Address),
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum QueryResponse {
    Tx(ExecutionResult),
    Number(U256),
    Sign(Signature),
}

impl QueryResponse {
    pub fn as_tx(&self) -> &ExecutionResult {
        match self {
            QueryResponse::Tx(inner) => inner,
            _ => panic!("not a tx"),
        }
    }

    pub fn as_balance(&self) -> U256 {
        match self {
            QueryResponse::Number(inner) => *inner,
            _ => panic!("not a balance"),
        }
    }

    pub fn as_signature(&self) -> Signature {
        match self {
            QueryResponse::Sign(inner) => *inner,
            _ => panic!("not a signature"),
        }
    }
}
