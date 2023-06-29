mod rpc_server;
pub use rpc_server::RpcApi;

mod engine;
pub use engine::Engine;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BroadcastTxQuery {
    tx: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AbciQueryQuery {
    path: String,
    data: String,
    height: Option<usize>,
    prove: Option<bool>,
}
