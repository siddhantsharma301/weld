use anvil_core::eth::EthRequest;
use ethereum_types::{Address, U256, Signature};
use ethers_core::types::transaction::request::TransactionRequest;
use ethers_providers::{Http, Middleware, Provider};
use evm_abci::types::RpcRequest;
use std::net::SocketAddr;
use tokio::sync::mpsc::Receiver;
use tokio::sync::oneshot::Sender as OneShotSender;

// Tendermint Types
use tendermint_proto::abci::ResponseQuery;

// Narwhal types
use narwhal_crypto::Digest;
use narwhal_primary::Certificate;

pub struct Engine {
    /// The path to the Primary's store, so that the Engine can query each of the Primary's workers
    /// for the data corresponding to a Certificate
    pub store_path: String,
    /// Messages received from the RPC Server to be forwarded to the engine.
    pub rx_abci_queries: Receiver<(OneShotSender<ResponseQuery>, RpcRequest)>,
    pub client: Provider<Http>,
}

impl Engine {
    pub fn new(
        app_address: SocketAddr,
        store_path: &str,
        rx_abci_queries: Receiver<(OneShotSender<ResponseQuery>, RpcRequest)>,
    ) -> Self {
        // Instantiate a new client to not be locked in an Info connection
        let client =
            Provider::<Http>::try_from(String::from("http://") + &app_address.to_string()).unwrap();

        Self {
            store_path: store_path.to_string(),
            rx_abci_queries,
            client,
        }
    }

    /// Receives an ordered list of certificates and apply any application-specific logic.
    pub async fn run(&mut self, mut rx_output: Receiver<Certificate>) -> eyre::Result<()> {
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
        // drive the app through the event loop
        let tx_count = self.reconstruct_and_deliver_txs(certificate).await?;
        log::info!("Tx count {}", tx_count);
        self.commit(tx_count).await?;
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
        let request_json = serde_json::from_value::<EthRequest>(serde_json::json!({
            "method": req.method.clone(),
            "params": serde_json::from_str::<Vec<String>>(&req.params)?
        }));

        let response = match request_json {
            Ok(eth_request) => {
                match eth_request {
                    EthRequest::EthGetBalance(_, _) => {
                        let result: U256 = self.client.request(req.method.clone().as_str(), serde_json::from_str::<Vec<String>>(&req.params)?).await.unwrap();
                        serde_json::to_vec(&result).map_err(Into::into)
                    },
                    EthRequest::EthAccounts(_) => {
                        let result: Vec<Address> = self.client.request(req.method.clone().as_str(), serde_json::from_str::<Vec<String>>(&req.params)?).await.unwrap();
                        serde_json::to_vec(&result).map_err(Into::into)
                    },
                    EthRequest::EthBlockNumber(_) => {
                        let result: U256 = self.client.request(req.method.clone().as_str(), serde_json::from_str::<Vec<String>>(&req.params)?).await.unwrap();
                        serde_json::to_vec(&result).map_err(Into::into)
                    },
                    EthRequest::EthGasPrice(_) => {
                        let result: U256 = self.client.request(req.method.clone().as_str(), serde_json::from_str::<Vec<String>>(&req.params)?).await.unwrap();
                        serde_json::to_vec(&result).map_err(Into::into)
                    },
                    EthRequest::EthSign(_, _) => {
                        let result: Signature = self.client.request(req.method.clone().as_str(), serde_json::from_str::<Vec<String>>(&req.params)?).await.unwrap();
                        serde_json::to_vec(&result).map_err(Into::into)
                    },
                    EthRequest::EthGetTransactionCount(_, _) => {
                        let result: U256 = self.client.request(req.method.clone().as_str(), serde_json::from_str::<Vec<String>>(&req.params)?).await.unwrap();
                        serde_json::to_vec(&result).map_err(Into::into)
                    },
                    EthRequest::EthGetUnclesCountByNumber(_) => {
                        let result: U256 = self.client.request(req.method.clone().as_str(), serde_json::from_str::<Vec<String>>(&req.params)?).await.unwrap();
                        serde_json::to_vec(&result).map_err(Into::into)
                    },
                    EthRequest::EthGetUnclesCountByHash(_) => {
                        let result: U256 = self.client.request(req.method.clone().as_str(), serde_json::from_str::<Vec<String>>(&req.params)?).await.unwrap();
                        serde_json::to_vec(&result).map_err(Into::into)
                    }
                    _ => eyre::bail!("lol we don't support this")
                }        
            },
            Err(err) => Err(err),
        };
        if let Ok(response) = response {
            if let Err(err) = tx.send(ResponseQuery {
                value: response,
                ..Default::default()
            }) {
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
    async fn deliver_batch(&mut self, batch: Vec<u8>) -> eyre::Result<usize> {
        // Deserialize and parse the message.
        let mut count = 0;
        log::info!("Batch: {:?}", batch);
        match bincode::deserialize(&batch) {
            Ok(WorkerMessage::Batch(batch)) => {
                for tx in batch {
                    let res = self.deliver_tx(tx).await.map_err(|e| eyre::eyre!(e));
                    log::error!("Response: {:?}", res);
                    count += 1;
                }
            }
            _ => {
                log::error!("wtf");
                eyre::bail!("unrecognized message format")
            }
        };
        Ok(count)
    }

    /// Reconstructs the batch corresponding to the provided Primary's certificate from the Workers' stores
    /// and proceeds to deliver each tx to the App over ABCI's DeliverTx endpoint.
    async fn reconstruct_and_deliver_txs(
        &mut self,
        certificate: Certificate,
    ) -> eyre::Result<usize> {
        #[allow(clippy::needless_collect)]
        let batches = certificate
            .clone()
            .header
            .payload
            .into_iter()
            .map(|(digest, worker_id)| {
                let res = self.reconstruct_batch(digest, worker_id);
                res
            })
            .collect::<Vec<_>>();

        // Deliver
        let mut total_count = 0;
        for batch in batches {
            // this will throw an error if the deserialization failed anywhere
            let batch = batch.map_err(|e| eyre::eyre!(e))?;
            total_count += self
                .deliver_batch(batch)
                .await
                .map_err(|e| eyre::eyre!(e))?;
        }

        Ok(total_count)
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
    async fn deliver_tx(&mut self, tx: Transaction) -> eyre::Result<()> {
        let bytes = serde_json::from_slice::<TransactionRequest>(&tx).unwrap();
        self.client.send_transaction(bytes, None).await?;
        Ok(())
    }

    /// Calls the `Commit` hook on the ABCI app.
    async fn commit(&mut self, tx_count: usize) -> eyre::Result<()> {
        self.client
            .request("anvil_mine", vec![U256::from(tx_count), U256::from(0)])
            .await?;
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
