use anvil::spawn;
use std::net::SocketAddr;

use clap::Parser;

#[derive(Debug, Clone, Parser)]
struct Args {
    #[clap(default_value = "0.0.0.0:26658")]
    host: String,
    #[clap(long, short)]
    demo: bool,
}

use tracing_error::ErrorLayer;

use tracing_subscriber::prelude::*;

/// Initializes a tracing Subscriber for logging
#[allow(dead_code)]
pub fn subscriber() {
    tracing_subscriber::Registry::default()
        .with(tracing_subscriber::EnvFilter::new("evm-app=trace"))
        .with(ErrorLayer::default())
        .with(tracing_subscriber::fmt::layer())
        .init()
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let args = Args::parse();

    let addr = args.host.parse::<SocketAddr>().unwrap();

    let node_config = anvil::NodeConfig {
        host: Some(addr.ip()),
        port: addr.port(),
        no_mining: true,
        ..Default::default()
    };

    let (_, join_handle) = spawn(node_config).await;

    match join_handle.await.unwrap() {
        Ok(_) => {
            println!("Successfully listening on address: {}", addr);
        }
        Err(err) => {
            println!("Error: {}", err);
        }
    }

    Ok(())
}
