<img src="https://i.imgur.com/cZoHDUX.png" />

### Weld is a **proof-of-concept testnet framework** integrating Narwhal/Bullshark's high-throughput consensus with EVM execution powered by Foundry's Anvil.

We've built on [previous work from Paradigm](https://www.paradigm.xyz/2022/07/experiment-narwhal-bullshark-cosmos-stack) to run a simple ABCI app consisting of EVM execution on top of Narwhal/Bullshark, but by integrating with Anvil, we have added support for **Ethereum's JSON-RPC APIs**.

Components:
1. Client-facing RPC shim sends new transactions to N/B for ordering
2. Reliable stream of hashes of batches of transactions from Bullshark
3. Reconstruction of the ledger by querying Narwhal workers' stores for the confirmed batches of transactions
4. Shim delivers the reconstructed ledger to Anvil

The RPC shim redirects all other RPC calls (such as `eth_getBalance`, `eth_getTransactionCount`, etc.) to Anvil's Ethereum JSON-RPC.

<img src="https://www.paradigm.xyz/static/experiment-narwhal-bullshark-cosmos-stack/anvil-node.png" />
(image credit: Paradigm)

## Why Narwhal/Bullshark?
Narwhall/Bullshark are a part of a class of DAG-based BFT protocols that emphasize high throughput for proof-of-stake blockchains. It is fundamentally different than existing algorithms such as Gasper (Ethereum 2.0) and Tendermint, giving users more performant and customizable testnets. By using Foundry's Anvil, we allow users to pick their _own_ consensus algorithms, giving them control over the consensus _and_ execution layer. If users want to use N/B with a different execution environment, they can interface with the RPC that the consensus layer exposes.

## How did we integrate N/B with Anvil?

We had to find a way to imitate ABCI's `BeginBlock`, `DeliverTx`, `EndBlock`, and `Commit` hooks using a RPC server instead. We did this by using Anvil's `--no-mining` mode, delivering transactions from each batch to Anvil and then manually forcing Anvil to mine a block once the entire batch has been delivered.

## Execution and Consensus Layer Flexibility
Using the RPC shim, we can easily swap out the execution layer with any VM implementations. We can also swap out the consensus layer with any other consensus algorithm that exposes an RPC interface. This allows us to create a testnet with any combination of execution and consensus layers. For example, we can use Tendermint with Anvil or we can use Solana VM with N/B.

## Demo

Setup/dependencies (from the main folder of the repository):
* [Rust](https://www.rust-lang.org/)
* [Python Poetry](https://python-poetry.org/)
* [tmux](https://github.com/tmux/tmux)
* `cd demo && poetry install`

Run demo (from the main folder of the repository):
1. 1st terminal: `cd demo && cargo build && poetry run fab local`
2. 2nd terminal (after the testbed has started in 1st terminal): `cargo run --bin client`

The second command will produce output like this:
<img src="https://i.imgur.com/iNoymdG.gif" />

The demo consensus network is run by four nodes (each running on localhost), whose RPC endpoints are reachable on TCP ports 3002, 3009, 3016, and 3023, respectively. There are three accounts, `0xf39f` (initially 1.5 ETH), `0x7099` (initially 100 ETH), and `0x3c44` (initially 100 ETH). `0xf39f` performs a double spend, sending 1 ETH each to `0x7099` and `0x3c44` in two different transactions that get input to the nodes at ports 3009 and 3016, respectively. Note that only one transaction can make it. Eventually, nodes reach consensus on which transaction gets executed in Anvil, and the application state is updated in lockstep across all nodes. The update is reflected in subsequent balance queries.

## Future Work

* Implementing benchmarks for metrics like TPS, identifying bottlenecks between consensus and execution, etc.
* Migrating from `ethers-rs` to [Alloy](https://github.com/alloy-rs/core)

## Acknowledgments
We want to thank [Georgios K.](https://twitter.com/gakonst), [Andrew K.](https://twitter.com/a_kirillo), [Joachim N.](https://twitter.com/jneu_net), and the Foundry team for their initial work.
