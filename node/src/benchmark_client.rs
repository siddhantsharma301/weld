// Copyright(C) Facebook, Inc. and its affiliates.
use anyhow::{Context, Result};
use bytes::BufMut as _;
use bytes::BytesMut;
use clap::{crate_name, crate_version, App, AppSettings};
use env_logger::Env;
use futures::future::join_all;
use futures::sink::SinkExt as _;
use log::{info, warn};
use std::net::SocketAddr;
use tokio::net::TcpStream;
use tokio::time::{interval, sleep, Duration, Instant};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

use anvil_rpc::request::RequestParams;
use ethers::{
    abi::ethereum_types::{Secret, H520},
    core::rand::Rng,
    prelude::*,
};
use evm_abci::types::QueryResponse;
use yansi::Paint;

#[tokio::main]
async fn main() -> Result<()> {
    let matches = App::new(crate_name!())
        .version(crate_version!())
        .about("Benchmark client for Narwhal and Tusk.")
        .args_from_usage("<ADDR> 'The network address of the node where to send txs'")
        .args_from_usage("--size=<INT> 'The size of each transaction in bytes'")
        .args_from_usage("--rate=<INT> 'The rate (txs/s) at which to send the transactions'")
        .args_from_usage("--nodes=[ADDR]... 'Network addresses that must be reachable before starting the benchmark.'")
        .setting(AppSettings::ArgRequiredElseHelp)
        .get_matches();

    env_logger::Builder::from_env(Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    let target = matches
        .value_of("ADDR")
        .unwrap()
        .parse::<SocketAddr>()
        .context("Invalid socket address format")?;
    let size = matches
        .value_of("size")
        .unwrap()
        .parse::<usize>()
        .context("The size of transactions must be a non-negative integer")?;
    let rate = matches
        .value_of("rate")
        .unwrap()
        .parse::<u64>()
        .context("The rate of transactions must be a non-negative integer")?;
    let nodes = matches
        .values_of("nodes")
        .unwrap_or_default()
        .into_iter()
        .map(|x| x.parse::<SocketAddr>())
        .collect::<Result<Vec<_>, _>>()
        .context("Invalid socket address format")?;

    info!("Node address: {}", target);

    // NOTE: This log entry is used to compute performance.
    info!("Transactions size: {} B", size);

    // NOTE: This log entry is used to compute performance.
    info!("Transactions rate: {} tx/s", rate);

    let client = Client {
        target,
        size,
        rate,
        nodes,
    };

    // Wait for all nodes to be online and synchronized.
    client.wait().await;

    // Start the benchmark.
    client.send().await.context("Failed to submit transactions")
}

struct Client {
    target: SocketAddr,
    size: usize,
    rate: u64,
    nodes: Vec<SocketAddr>,
}

impl Client {
    pub async fn send(&self) -> Result<()> {
        const PRECISION: u64 = 20; // Sample precision.
        const BURST_DURATION: u64 = 1000 / PRECISION;

        // The transaction size must be at least 16 bytes to ensure all txs are different.
        if self.size < 9 {
            return Err(anyhow::Error::msg(
                "Transaction size must be at least 9 bytes",
            ));
        }

        // Connect to the mempool.
        let stream = TcpStream::connect(self.target)
            .await
            .context(format!("failed to connect to {}", self.target))?;

        // Submit all transactions.
        let burst = self.rate / PRECISION;
        let mut tx = BytesMut::with_capacity(self.size);
        let mut counter = 0;
        // let mut r = rand::thread_rng().gen();
        let mut rng = rand::thread_rng();
        let addresses = get_accounts("http://127.0.0.1:3002").await?;
        let mut transport = Framed::new(stream, LengthDelimitedCodec::new());
        let interval = interval(Duration::from_millis(BURST_DURATION));
        tokio::pin!(interval);

        let host_1 = "http://127.0.0.1:3002";
        let host_2 = "http://127.0.0.1:3009";
        let host_3 = "http://127.0.0.1:3016";
        let host_4 = "http://127.0.0.1:3023";
        let hosts = [host_1, host_2, host_3, host_4];

        // NOTE: This log entry is used to compute performance.
        info!("Start sending transactions");

        'main: loop {
            interval.as_mut().tick().await;
            let now = Instant::now();

            for x in 0..burst {
                let host = rng.gen_range(0..4);
                let from = rng.gen_range(0..10);
                let mut to = rng.gen_range(0..10);
                while to == from {
                    to = rng.gen_range(0..10);
                }
                let amount = rng.gen_range(1..10);
                let units = rng.gen_range(1..5);
                let value = ethers::utils::parse_units(amount, units)?;
                match send_transaction(hosts[host], addresses[from], addresses[to], value.into(), counter).await {
                    Err(_) => {},
                    Ok(_) => {}
                }
                counter += 1;
                // if x == counter % burst {
                //     // NOTE: This log entry is used to compute performance.
                //     info!("Sending sample transaction {}", counter);

                //     tx.put_u8(0u8); // Sample txs start with 0.
                //     tx.put_u64(counter); // This counter identifies the tx.
                // } else {
                //     r += 1;
                //     tx.put_u8(1u8); // Standard txs start with 1.
                //     tx.put_u64(r); // Ensures all clients send different txs.
                // };

                // tx.resize(self.size, 0u8);
                // let bytes = tx.split().freeze();
                // if let Err(e) = transport.send(bytes).await {
                //     warn!("Failed to send transaction: {}", e);
                //     break 'main;
                // }
            }
            if now.elapsed().as_millis() > BURST_DURATION as u128 {
                // NOTE: This log entry is used to compute performance.
                warn!("Transaction rate too high for this client");
            }
        }
        Ok(())
    }

    pub async fn wait(&self) {
        // Wait for all nodes to be online.
        info!("Waiting for all nodes to be online...");
        join_all(self.nodes.iter().cloned().map(|address| {
            tokio::spawn(async move {
                while TcpStream::connect(address).await.is_err() {
                    sleep(Duration::from_millis(10)).await;
                }
            })
        }))
        .await;
    }
}

async fn get_accounts(host: &str) -> Result<Vec<Address>> {
    let client = reqwest::Client::new();
    let params = serde_json::to_string(&RequestParams::Array(vec![]))?;
    let res = client
        .get(format!("{}/rpc_query", host))
        .query(&[("method", "eth_accounts"), ("params", params.as_str())])
        .send()
        .await?;

    let val = res.bytes().await?;
    let addresses_result = serde_json::from_slice(&val).map_err(Into::into);
    match addresses_result {
        Ok(addresses) => Ok(addresses),
        Err(err) => Err(err),
    }
}

async fn send_transaction(host: &str, from: Address, to: Address, value: U256, counter: usize) -> Result<()> {
    let readable_value = get_readable_eth_value(value)?;
    // debug!(
    //     "{} sends TX to {} transferring {} to {}...",
    //     Paint::new(from).bold(),
    //     Paint::red(host).bold(),
    //     Paint::new(format!("{} ETH", readable_value)).bold(),
    //     Paint::red(to).bold()
    // );
    info!(
        "sample transaction {:?}",
        counter
    );

    // let r = rand::thread_rng().gen_range(0..150_000);
    let tx = TransactionRequest::new()
        .from(from)
        .to(to)
        .value(value)
        .gas(21000 + counter)
        .nonce(counter);

    let tx = serde_json::to_string(&tx)?;

    let client = reqwest::Client::new();
    client
        .get(format!("{}/broadcast_tx", host))
        .query(&[("tx", tx)])
        .send()
        .await?;

    Ok(())
}

fn get_readable_eth_value(value: U256) -> Result<f64> {
    let value_string = ethers::utils::format_units(value, "ether")?;
    Ok(value_string.parse::<f64>()?)
}