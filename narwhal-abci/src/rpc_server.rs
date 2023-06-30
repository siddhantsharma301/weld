use crate::BroadcastTxQuery;

use eyre::WrapErr;
use futures::SinkExt;
use tendermint_proto::abci::ResponseQuery;
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot::{channel as oneshot_channel, Sender as OneShotSender};

use tokio::net::TcpStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use warp::{Filter, Rejection};

use std::net::SocketAddr;

use evm_abci::types::RpcRequest;

/// Simple HTTP API server which listens to messages on:
/// * `broadcast_tx`: forwards them to Narwhal's mempool/worker socket, which will proceed to put
/// it in the consensus process and eventually forward it to the application.
/// * `abci_query`: forwards them over a channel to a handler (typically the application).
pub struct RpcApi<T> {
    mempool_address: SocketAddr,
    tx: Sender<(OneShotSender<T>, RpcRequest)>,
}

impl<T: Send + Sync + std::fmt::Debug> RpcApi<T> {
    pub fn new(
        mempool_address: SocketAddr,
        tx: Sender<(OneShotSender<T>, RpcRequest)>,
    ) -> Self {
        Self {
            mempool_address,
            tx,
        }
    }
}

impl RpcApi<ResponseQuery> {
    pub fn routes(self) -> impl Filter<Extract = (impl warp::Reply,), Error = Rejection> + Clone {
        let route_broadcast_tx = warp::path("broadcast_tx")
            .and(warp::query::<BroadcastTxQuery>())
            .and_then(move |req: BroadcastTxQuery| async move {
                log::warn!("broadcast_tx: {:?}", req);

                let stream = TcpStream::connect(self.mempool_address)
                    .await
                    .wrap_err(format!(
                        "ROUTE_BROADCAST_TX failed to connect to {}",
                        self.mempool_address
                    ))
                    .unwrap();
                let mut transport: Framed<TcpStream, LengthDelimitedCodec> = Framed::new(stream, LengthDelimitedCodec::new());

                if let Err(e) = transport.send(req.tx.clone().into()).await {
                    Ok::<_, Rejection>(format!("ERROR IN: broadcast_tx: {:?}. Err: {}", req, e))
                } else {
                    Ok::<_, Rejection>(format!("broadcast_tx: {:?}", req))
                }
            });

        let route_rpc_query = warp::path("rpc_query")
            .and(warp::query::<RpcRequest>())
            .and_then(move |req: RpcRequest| {
                let rpc_query = self.tx.clone();
                async move {
                    log::warn!("rpc_query: {:?}", req);

                    let (tx, rx) = oneshot_channel();
                    match rpc_query.send((tx, req.clone())).await {
                        Ok(_) => {},
                        Err(err) => log::error!("Error forwarding rpc query: {}", err),
                    };
                    let resp = rx.await.unwrap();
                    // Return the value
                    Ok::<_, Rejection>(resp.value)
                }
            });

        route_broadcast_tx.or(route_rpc_query)
    }
}
