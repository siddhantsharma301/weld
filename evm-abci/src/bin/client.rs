use anvil_rpc::request::RequestParams;
use ethers::prelude::*;
use evm_abci::types::QueryResponse;
use eyre::Result;
use yansi::Paint;

fn get_readable_eth_value(value: U256) -> Result<f64> {
    let value_string = ethers::utils::format_units(value, "ether")?;
    Ok(value_string.parse::<f64>()?)
}

async fn query_balance(host: &str, address: Address) -> Result<()> {
    let client = reqwest::Client::new();
    let res = client
        .get(format!("{}/rpc_query", host))
        .query(&[
            ("method", "eth_getBalance"),
            (
                "params",
                serde_json::to_string(&RequestParams::Array(vec![
                    serde_json::to_value(address)?,
                    serde_json::to_value("latest")?,
                ]))?
                .as_str(),
            ),
        ])
        .send()
        .await?;

    let val = res.bytes().await?;
    let val: QueryResponse = QueryResponse::Balance(serde_json::from_slice(&val)?);
    let val = val.as_balance();
    let readable_value = get_readable_eth_value(val)?;
    println!(
        "{}'s balance: {}",
        Paint::new(address).bold(),
        Paint::green(format!("{} ETH", readable_value)).bold()
    );
    Ok(())
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

async fn send_transaction(host: &str, from: Address, to: Address, value: U256) -> Result<()> {
    let readable_value = get_readable_eth_value(value)?;
    println!(
        "{} sends TX to {} transferring {} to {}...",
        Paint::new(from).bold(),
        Paint::red(host).bold(),
        Paint::new(format!("{} ETH", readable_value)).bold(),
        Paint::red(to).bold()
    );

    let tx = TransactionRequest::new()
        .from(from)
        .to(to)
        .value(value)
        .gas(21000);

    let tx = serde_json::to_string(&tx)?;

    let client = reqwest::Client::new();
    client
        .get(format!("{}/broadcast_tx", host))
        .query(&[("tx", tx)])
        .send()
        .await?;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // the ABCI port on the various narwhal primaries
    let host_1 = "http://127.0.0.1:3002";
    let host_2 = "http://127.0.0.1:3009";
    let host_3 = "http://127.0.0.1:3016";

    let value = ethers::utils::parse_units(1, 18)?;

    let addresses = get_accounts(host_1).await?;

    // Reduce the balance of address 0 and wait for state transition
    send_transaction(
        host_2,
        addresses[0],
        addresses[8],
        ethers::utils::parse_units(98.5, 18)?.into(),
    )
    .await?;
    println!("Waiting for consensus...");
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    // TODO: Query initial balances from host_1
    query_balance(host_1, addresses[0]).await?;
    query_balance(host_1, addresses[1]).await?;
    query_balance(host_1, addresses[2]).await?;

    println!("===============================");

    // Send conflicting transactions
    println!(
        "{} sends {} transactions:",
        Paint::new("Alice").bold(),
        Paint::red(format!("conflicting")).bold()
    );
    send_transaction(host_2, addresses[0], addresses[1], value.into()).await?;
    send_transaction(host_3, addresses[0], addresses[2], value.into()).await?;

    println!("===============================");

    println!("Waiting for consensus...");
    // Takes ~5 seconds to actually apply the state transition?
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    println!("===============================");

    // TODO: Query final balances from host_2
    query_balance(host_2, addresses[0]).await?;
    query_balance(host_2, addresses[1]).await?;
    query_balance(host_2, addresses[2]).await?;

    println!("===============================");

    // TODO: Query final balances from host_3
    query_balance(host_3, addresses[0]).await?;
    query_balance(host_3, addresses[1]).await?;
    query_balance(host_3, addresses[2]).await?;

    Ok(())
}
