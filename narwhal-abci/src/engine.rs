use anvil_core::eth::EthRequest;
use anvil_rpc::response;
use ethereum_types::{U256, Address};
use ethers_providers::{Provider, Http};
use std::net::SocketAddr;
use evm_abci::types::RpcRequest;
use tokio::sync::mpsc::Receiver;
use tokio::sync::oneshot::Sender as OneShotSender;

// Tendermint Types
use tendermint_abci::{Client as AbciClient, ClientBuilder};
use tendermint_proto::abci::{
    RequestBeginBlock, RequestDeliverTx, RequestEndBlock, RequestInfo, RequestInitChain,
    RequestQuery, ResponseQuery,
};
use tendermint_proto::types::Header;

// Narwhal types
use narwhal_crypto::Digest;
use narwhal_primary::Certificate;

/// The engine drives the ABCI Application by concurrently polling for:
/// 1. Calling the BeginBlock -> DeliverTx -> EndBlock -> Commit event loop on the ABCI App on each Bullshark
///    certificate received. It will also call Info and InitChain to initialize the ABCI App if
///    necessary.
/// 2. Processing Query & Broadcast Tx messages received from the Primary's ABCI Server API and forwarding them to the
///    ABCI App via a Tendermint protobuf client.
pub struct Engine {
    /// The address of the app
    pub app_address: SocketAddr,
    /// The path to the Primary's store, so that the Engine can query each of the Primary's workers
    /// for the data corresponding to a Certificate
    pub store_path: String,
    /// Messages received from the RPC Server to be forwarded to the engine.
    pub rx_abci_queries: Receiver<(OneShotSender<ResponseQuery>, RpcRequest)>,
    /// The last block height, initialized to the application's latest block by default
    pub last_block_height: i64,
    pub client: Provider<Http>,
    pub req_client: Provider<Http>,
}

impl Engine {
    pub fn new(
        app_address: SocketAddr,
        store_path: &str,
        rx_abci_queries: Receiver<(OneShotSender<ResponseQuery>, RpcRequest)>,
    ) -> Self {
        let mut client = ClientBuilder::default().connect(&app_address).unwrap();

        let last_block_height = client
            .info(RequestInfo::default())
            .map(|res| res.last_block_height)
            .unwrap_or_default();

        // Instantiate a new client to not be locked in an Info connection
        let client = Provider::<Http>::try_from(String::from("http://") + &app_address.to_string()).unwrap();
        let req_client = Provider::<Http>::try_from(String::from("http://") + &app_address.to_string()).unwrap();

        Self {
            app_address,
            store_path: store_path.to_string(),
            rx_abci_queries,
            last_block_height,
            client,
            req_client,
        }
    }

    /// Receives an ordered list of certificates and apply any application-specific logic.
    pub async fn run(&mut self, mut rx_output: Receiver<Certificate>) -> eyre::Result<()> {
        // self.init_chain()?;

        loop {
            tokio::select! {
                Some(certificate) = rx_output.recv() => {
                    self.handle_cert(certificate).await?;
                },
                Some((tx, req)) = self.rx_abci_queries.recv() => {
                    self.handle_rpc_query(tx, req).await?;
                }
                else => break,
            }
        }

        Ok(())
    }

    /// On each new certificate, increment the block height to proposed and run through the
    /// BeginBlock -> DeliverTx for each tx in the certificate -> EndBlock -> Commit event loop.
    async fn handle_cert(&mut self, certificate: Certificate) -> eyre::Result<()> {
        // increment block
        let proposed_block_height = self.last_block_height + 1;

        // save it for next time
        self.last_block_height = proposed_block_height;

        // drive the app through the event loop
        let tx_count = self.reconstruct_and_deliver_txs(certificate).await?;
        self.commit(tx_count)?;
        Ok(())
    }

    /// Handles ABCI queries coming to the primary and forwards them to the ABCI App. Each
    /// handle call comes with a Sender channel which is used to send the response back to the
    /// Primary and then to the client.
    ///
    /// Client => Primary => handle_cert => ABCI App => Primary => Client
    async fn handle_rpc_query(
        &mut self,
        tx: OneShotSender<ResponseQuery>,
        req: RpcRequest,
    ) -> eyre::Result<()> {
        let params = serde_json::from_str(&req.params)?;
        let request_result = serde_json::from_value::<EthRequest>(serde_json::json!({
            "method": req.method.clone(),
            "params": serde_json::from_str(&req.params)?
        }));

        let response = match request_result {
            Ok(eth_request) => {
                match eth_request {
                    EthRequest::EthGetBalance(_, _) => {
                        let result: U256 = self.client.request(req.method.clone().as_str(), params).await.unwrap();
                        serde_json::to_vec(&result).map_err(Into::into)
                    },
                    EthRequest::EthAccounts(params) => {
                        let result: Vec<Address> = self.client.request(req.method.clone().as_str(), params).await.unwrap();
                        serde_json::to_vec(&result).map_err(Into::into)
                    },
                    EthRequest::EthGetUnclesCountByHash(_) => {
                        let result: U256 = self.client.request(req.method.clone().as_str(), params).await.unwrap();
                        serde_json::to_vec(&result).map_err(Into::into)
                    }
                    _ => eyre::bail!("lol we don't support this")
                }        
            },
            Err(err) => Err(err)
        };
        if let Ok(response) = response {
            if let Err (err) = tx.send(ResponseQuery{value: response, ..Default::default()}) {
                eyre::bail!("{:?}", err);
            }    
        }
        Ok(())
    }

    /// Opens a RocksDB handle to a Worker's database and tries to read the batch
    /// stored at the provided certificate's digest.
    fn reconstruct_batch(&self, digest: Digest, worker_id: u32) -> eyre::Result<Vec<u8>> {
        // Open the database to each worker
        // TODO: Figure out if this is expensive
        let db = rocksdb::DB::open_for_read_only(
            &rocksdb::Options::default(),
            self.worker_db(worker_id),
            true,
        )?;

        // Query the db
        let key = digest.to_vec();
        match db.get(&key) {
            Ok(Some(res)) => Ok(res),
            Ok(None) => eyre::bail!("digest {} not found", digest),
            Err(err) => eyre::bail!(err),
        }
    }

    /// Calls DeliverTx on the ABCI app
    /// Deserializes a raw abtch as `WorkerMesssage::Batch` and proceeds to deliver
    /// each transaction over the DeliverTx API.
    fn deliver_batch(&mut self, batch: Vec<u8>) -> eyre::Result<()> {
        // Deserialize and parse the message.
        match bincode::deserialize(&batch) {
            Ok(WorkerMessage::Batch(batch)) => {
                batch.into_iter().try_for_each(|tx| {
                    self.deliver_tx(tx)?;
                    Ok::<_, eyre::Error>(())
                })?;
            }
            _ => eyre::bail!("unrecognized message format"),
        };
        Ok(())
    }

    /// Reconstructs the batch corresponding to the provided Primary's certificate from the Workers' stores
    /// and proceeds to deliver each tx to the App over ABCI's DeliverTx endpoint.
    fn reconstruct_and_deliver_txs(&mut self, certificate: Certificate) -> eyre::Result<()> {
        // Try reconstructing the batches from the cert digests
        //
        // NB:
        // This is maybe a false positive by Clippy, without the `collect` the Iterator fails
        // iterator fails to compile because we're mutably borrowing in the `try_for_each`
        // when we've already immutably borrowed in the `.map`.
        #[allow(clippy::needless_collect)]
        let batches = certificate
            .header
            .payload
            .into_iter()
            .map(|(digest, worker_id)| self.reconstruct_batch(digest, worker_id))
            .collect::<Vec<_>>();

        // Deliver
        batches.into_iter().try_for_each(|batch| {
            // this will throw an error if the deserialization failed anywhere
            let batch = batch?;
            self.deliver_batch(batch)?;
            Ok::<_, eyre::Error>(())
        })?;

        Ok(())
    }

    /// Helper function for getting the database handle to a worker associated
    /// with a primary (e.g. Primary db-0 -> Worker-0 db-0-0, Wroekr-1 db-0-1 etc.)
    fn worker_db(&self, id: u32) -> String {
        format!("{}-{}", self.store_path, id)
    }
}

// Tendermint Lifecycle Helpers
impl Engine {
    /// Calls the `DeliverTx` hook on the ABCI app.
    fn deliver_tx(&mut self, tx: Transaction) -> eyre::Result<()> {
        self.client.deliver_tx(RequestDeliverTx { tx })?;
        Ok(())
    }

    /// Calls the `Commit` hook on the ABCI app.
    fn commit(&mut self, tx_count: usize) -> eyre::Result<()> {
        self.client.commit()?;
        Ok(())
    }
}

// Helpers for deserializing batches, because `narwhal::worker` is not part
// of the public API. TODO -> make a PR to expose it.
pub type Transaction = Vec<u8>;
pub type Batch = Vec<Transaction>;
#[derive(serde::Deserialize)]
pub enum WorkerMessage {
    Batch(Batch),
}
