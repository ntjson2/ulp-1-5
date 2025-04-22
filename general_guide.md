# ULP 1.5 - General Guide (Event Monitoring Architecture)

This guide provides an overview of setting up and running the ULP 1.5 Cross-DEX Arbitrage Bot, which uses an event-driven architecture for low-latency opportunity detection.

## 1. Architecture Overview

Instead of polling DEX pools periodically via RPC, this version subscribes to on-chain events using a WebSocket connection.

*   **WebSocket Provider:** Connects to an L2 node's WebSocket endpoint (e.g., `ws://localhost:8545` for local Anvil, or a provider's `wss://...` URL).
*   **Event Subscriptions:** Subscribes to `newHeads` (new blocks) and `logs` (specifically targeting `Swap` events from relevant DEX liquidity pools).
*   **Event Handler (`event_handler.rs`):** Receives events pushed by the node. Decodes `Swap` events to understand price/reserve changes.
*   **State Cache (`AppState`):** An in-memory, thread-safe map (`DashMap`) stores the latest known state (`PoolState`: sqrtPrice/reserves, tokens, etc.) for each monitored pool, updated by the event handler.
*   **Arbitrage Check:** When a `Swap` event updates a pool's state significantly, the `event_handler` triggers a check comparing the updated pool's price against cached prices of other pools for the same token pair.
*   **Optimization & Execution (`simulation.rs`, `gas.rs`, `main.rs`):** If the check reveals a potential arbitrage spread exceeding the configured threshold, the bot:
    1.  Performs a liquidity pre-check.
    2.  Searches for the optimal flash loan amount using detailed simulation and gas estimation.
    3.  Performs a final profitability check based on the optimal amount and current gas conditions.
    4.  If profitable, constructs and sends the `flashLoan` transaction using an HTTP-based `SignerMiddleware` client.
*   **On-Chain Executor (`ArbitrageExecutor.huff`):** Receives the flash loan, executes swaps atomically, verifies profit, and repays the loan.

## 2. Setup

### Prerequisites

*   Rust Toolchain (`rustup`, `cargo`)
*   Foundry (`anvil`, `cast`) for local testing and deployment.
*   Huff Compiler (`huffc`).
*   Node access:
    *   **Local Testing:** Anvil running with a fork of the target L2 (e.g., Optimism): `anvil --fork-url <OPTIMISM_MAINNET_RPC_URL> --chain-id 10`
    *   **Live:** WebSocket (`wss://`) and HTTP (`https://`) RPC endpoints for the target L2 (e.g., from Alchemy, Infura).

### Configuration (`.env`)

Create a `.env` file in the project root (where `Cargo.toml` resides). Key variables:

*   `LOCAL_RPC_URL`: **Must be a WebSocket endpoint** (`ws://` or `wss://`) for the event monitoring.
*   `LOCAL_PRIVATE_KEY`: Private key for the bot's wallet (used for deployment and sending transactions).
*   `DEPLOY_EXECUTOR`: `true` to deploy `ArbitrageExecutor.huff` on startup, `false` to use an existing deployment.
*   `EXECUTOR_BYTECODE_PATH`: Path to the compiled Huff bytecode (e.g., `./build/ArbitrageExecutor.bin`) if `DEPLOY_EXECUTOR=true`.
*   `ARBITRAGE_EXECUTOR_ADDRESS`: Address of the deployed executor contract if `DEPLOY_EXECUTOR=false`.
*   `WETH_ADDRESS`, `USDC_ADDRESS`: Addresses of the *specific* token pair to target.
*   `WETH_DECIMALS`, `USDC_DECIMALS`: Decimals for the target tokens.
*   `VELO_V2_ROUTER_ADDR`: Address of the Velodrome V2 Router (used for encoding `userData`).
*   `BALANCER_VAULT_ADDRESS`: Address of the Balancer V2 Vault.
*   `QUOTER_V2_ADDRESS`: Address of the Uniswap V3 QuoterV2 contract.
*   `MIN_LOAN_AMOUNT_WETH`, `MAX_LOAN_AMOUNT_WETH`: Range for optimal amount search.
*   `OPTIMAL_LOAN_SEARCH_ITERATIONS`: Number of samples for optimization search.
*   `MAX_PRIORITY_FEE_PER_GAS_GWEI`: EIP-1559 tip.
*   `GAS_LIMIT_BUFFER_PERCENTAGE`: Buffer added to gas estimate.
*   `MIN_FLASHLOAN_GAS_LIMIT`: Minimum gas limit floor.
*   `RUST_LOG`: Logging level (e.g., `info`, `debug`, `warn`, `error`, or `info,ulp1_5=debug`).

### Compilation

1.  **Compile Huff:** `huffc contracts/ArbitrageExecutor.huff -b -o build/ArbitrageExecutor.bin` (Adjust paths if needed). Ensure the `build` directory exists.
2.  **Compile Rust:** `cargo build`

## 3. Running the Bot

### Polling/Event Monitoring Mode (Default)

1.  Ensure Anvil is running (with fork if testing locally) or you have live L2 endpoints configured.
2.  Ensure `.env` is configured correctly (especially the `LOCAL_RPC_URL` must be WebSocket).
3.  Ensure the Huff contract is deployed (either via `DEPLOY_EXECUTOR=true` or by providing the address).
4.  Fund the bot's wallet (`LOCAL_PRIVATE_KEY`) with native gas token (e.g., ETH on Optimism/Anvil) if deploying or sending transactions.
5.  Run the bot, setting the log level:
    ```bash
    RUST_LOG=info,ulp1_5=debug cargo run --bin ulp1_5
    ```
6.  The bot will connect, optionally deploy, load initial state (currently basic), subscribe to events, and enter the main event loop. It will log new blocks and eventually process `Swap` events (once fully implemented) to check for arbitrage.
7.  Use `Ctrl+C` to stop the bot.

### Withdrawal Mode

1.  Ensure `.env` has the correct `LOCAL_RPC_URL` (HTTP or WS is fine here), `LOCAL_PRIVATE_KEY`, and `ARBITRAGE_EXECUTOR_ADDRESS`.
2.  Run the bot with withdrawal flags:
    ```bash
    cargo run --bin ulp1_5 -- --withdraw-token <TOKEN_ADDRESS> --withdraw-recipient <YOUR_ADDRESS>
    ```
    (Replace `<TOKEN_ADDRESS>` and `<YOUR_ADDRESS>` accordingly).
3.  The bot will attempt the withdrawal and exit.

## 4. Node Scanner (Deprecated for Real-time Bot)

The separate Node.js scanner (`node_scanner/`) using subgraphs was useful for *offline analysis* to find pairs that exist on multiple DEXs. However, the real-time event-driven Rust bot **does not** currently read the JSON output from that scanner. The event-driven approach relies on different mechanisms for discovering and monitoring pools:

*   **TODO: Dynamic Discovery:** Subscribe to `PoolCreated` events from DEX factories.
*   **TODO: Initial Loading:** Query factories or load a pre-defined list of pools at startup.

The Node.js scanner can still be useful for research but is not part of the bot's critical path anymore.

## 5. Key Concepts

*   **Event-Driven:** Reacting to on-chain events (Swaps) instead of constant polling reduces latency and potentially RPC load.
*   **WebSocket Subscription:** Used for receiving real-time events.
*   **State Cache:** Essential for quickly comparing prices when an event arrives. Needs to be kept consistent.
*   **Atomicity:** The entire arbitrage (borrow, swap, swap, repay) relies on the single-transaction execution managed by the Balancer flash loan callback and the Huff contract's internal profit check.
*   **Optimization:** Finding the optimal trade size is crucial due to slippage.
*   **Gas Management:** Using EIP-1559 and appropriate limits is vital for transaction inclusion and cost-effectiveness.