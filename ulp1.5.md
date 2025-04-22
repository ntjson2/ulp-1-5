# ULP 1.5 Status: Cross-DEX Arbitrage System (Event Monitoring Architecture)

## 1. Project Objective & Key Use Case

**Objective:** Develop "ULP 1.5", a high-efficiency, low-latency pure arbitrage system using an **event-driven architecture** optimized for L2 execution (Optimism focus initially, extensible). The system captures price discrepancies between different DEXs (Uniswap V3, Velodrome V2 initially) using Balancer V2 flash loans for capital.

**Use Case:** Execute atomic 2-way cross-DEX arbitrage trades (e.g., WETH/USDC between UniV3 and VeloV2) by:
    1. Connecting to an L2 node via WebSocket (`ws`/`wss`).
    2. Subscribing to relevant on-chain events (e.g., `Swap` events from target DEX pools, `PoolCreated` from factories).
    3. Maintaining an **in-memory state cache** (`AppState`) of relevant pool data (prices/reserves), updated in near real-time based on received events.
    4. **Triggering an arbitrage check** when a monitored pool's state changes significantly (e.g., after a `Swap` event).
    5. Comparing the updated pool's price against other cached pool prices for the same pair.
    6. If a profitable spread exceeding a threshold is detected:
        a. Perform a **liquidity pre-check** to estimate feasibility.
        b. Run an **optimal loan amount search** using simulations (`QuoterV2`/`getAmountsOut`) and gas estimation (`estimate_gas`) to find the highest profit trade size.
        c. Perform a **final profitability check** using the optimal amount and estimated EIP-1559 costs.
        d. **Send the `flashLoan` transaction** to Balancer V2, targeting the deployed Huff executor, using optimized EIP-1559 parameters and a buffered gas limit.
    7. The **on-chain Huff executor** (`ArbitrageExecutor.huff`) receives the loan, executes the two swaps atomically, checks for profit, repays the loan+fee, and retains profit.
    8. Monitor the transaction outcome (success/revert) via RPC/WebSocket.

**Constraint:** Focus solely on pure arbitrage based on existing price differences derived from on-chain events and state. No front-running, sandwich attacks, or complex MEV beyond capturing the discovered spread atomically.

## 2. Core Logic & Architecture (Event-Driven)

The system primarily consists of:

1.  **On-Chain Huff Executor (`ArbitrageExecutor.huff` v2.1.1+):**
    *   Ultra-low gas contract deployed to the target L2.
    *   Receives Balancer V2 flash loans (`receiveFlashLoan`).
    *   Parses `userData` (pool addresses, token addresses, DEX types/flags, routers) encoded by the off-chain bot.
    *   Conditionally executes two swaps across specified DEXs (UniV3, VeloV2 supported).
    *   Includes **on-chain profit check**; reverts if `final_balance < loan + fee`.
    *   Approves Balancer Vault for repayment upon success.
    *   Includes `withdrawToken` function for owner profit retrieval.
    *   Uses `LOG1` for basic on-chain debugging.

2.  **Off-Chain Rust Bot (`ulp1_5` binary):**
    *   **Modular Structure:** Code organized into modules (`config`, `utils`, `simulation`, `bindings`, `encoding`, `deploy`, `gas`, `event_handler`, `main`).
    *   **Configuration (`config.rs`, `.env`):** Loads WebSocket/HTTP RPC URLs, private key, contract addresses (or deployment options), token info, optimization parameters, gas strategy parameters.
    *   **Connectivity:**
        *   Establishes persistent **WebSocket connection** (`Provider<Ws>`) for receiving events.
        *   Maintains a separate **HTTP Signer Client** (`SignerMiddleware`) for sending transactions and potentially state-fetching/deployment.
    *   **Event Subscription (`main.rs`):** Subscribes to `newHeads` and `logs` (specifically `Swap` events from target pools) using `eth_subscribe`.
    *   **State Management (`event_handler.rs`, `main.rs`):** Uses a concurrent `DashMap` (`AppState`) to store the latest known `PoolState` (reserves/sqrtPrice, tokens, etc.) for monitored pools.
    *   **Event Handling (`event_handler.rs`):**
        *   Receives block headers and log events asynchronously.
        *   Decodes relevant events (currently placeholder/basic logic for UniV3/VeloV2 Swaps).
        *   Updates the `AppState` cache based on decoded event data (e.g., new sqrtPrice for UniV3, triggers `getReserves` poll for VeloV2).
        *   Triggers `check_for_arbitrage` upon state updates.
    *   **Arbitrage Check (`event_handler.rs`):** Compares the price of the updated pool (derived from cached state) against other cached pools for the same pair.
    *   **Optimization & Simulation (`simulation.rs`):**
        *   If potential arb found, `find_optimal_loan_amount` is called.
        *   Uses iterative sampling, calling `calculate_net_profit`.
        *   `calculate_net_profit` uses `simulate_swap` (which calls UniV3 Quoter or VeloV2 Router via RPC) and `estimate_flash_loan_gas` (`gas.rs`) to determine net profit for a given loan size.
    *   **Execution (`main.rs`):**
        *   Performs liquidity pre-check.
        *   If optimization yields a profitable amount and passes a final cost check:
            *   Encodes `userData` (`encoding.rs`).
            *   Calculates EIP-1559 fees (`max_fee`, `max_priority_fee`).
            *   Calculates buffered gas limit.
            *   Constructs and sends the `flashLoan` transaction via the Signer Client.
            *   Waits for and logs the transaction receipt status.
    *   **Deployment (`deploy.rs`):** Optionally deploys the Huff contract on startup.

## 3. Key Technologies & Patterns

*   **Runtime:** `tokio` for asynchronous operations (event streams, RPC calls, tasks).
*   **EVM Interaction:** `ethers-rs` (WebSocket Provider, SignerMiddleware, contract bindings, ABI encoding/decoding, utils).
*   **State:** `dashmap` for concurrent in-memory caching of pool states.
*   **Logging:** `tracing` and `tracing-subscriber` for structured logging.
*   **Configuration:** `dotenv` and custom `config.rs` module.
*   **Error Handling:** `eyre` for convenient error reporting.
*   **Concurrency:** `tokio::select!` for handling multiple event streams; `tokio::spawn` for handling events concurrently without blocking the main loop.
*   **On-Chain:** Huff for gas-optimized execution logic.

## 4. Current Status & TODOs

*   **Implemented:** WebSocket connection, basic block/log stream handling (placeholders), modular structure, config loading, contract bindings, Huff deployment, swap simulation (`simulate_swap`), profit calculation (`calculate_net_profit`), optimal amount search (`find_optimal_loan_amount`), gas estimation (`estimate_flash_loan_gas`), EIP-1559 fee calculation, gas limit buffering, transaction sending, withdrawal CLI mode.
*   **Core TODOs:**
    *   **Dynamic Pool Discovery/Loading:** Replace hardcoded initial pool list with dynamic loading (e.g., querying factories via RPC or loading from a pre-populated file/DB).
    *   **Full Event Decoding:** Implement robust decoding for all relevant `Swap` event variations across target DEXs within `handle_log_event`. Decode necessary data to update `PoolState` accurately. Handle `PoolCreated` events.
    *   **Arbitrage Trigger Logic:** Implement the actual call to `find_optimal_loan_amount` and subsequent execution logic within `check_for_arbitrage` or a separate task queue triggered by it.
    *   **Robust State Management:** Implement logic for fetching full initial state for newly discovered pools. Handle potential state inconsistencies or missed events (e.g., periodic RPC refresh). Add handling for blockchain reorgs affecting cached state.
    *   **WebSocket Error Handling:** Implement robust reconnection and resubscription logic if the WS connection or subscriptions fail.
    *   **Profit Withdrawal:** Test and refine the withdrawal CLI mode.
    *   **Refined Liquidity Check:** Improve accuracy, especially for UniV3.
    *   **Gas Strategy Tuning:** Test and refine EIP-1559 parameters and gas limit buffer.
    *   **Testing:** Implement automated tests for state manipulation (slippage, gas spikes).

## 5. Gas Optimization Notes

*   **Huff:** Remains the primary on-chain optimization.
*   **Off-Chain Calculation:** Keeping simulation, optimization, and complex checks off-chain minimizes gas.
*   **Event-Driven:** Reduces constant RPC polling compared to the previous architecture.
*   **On-Chain Profit Check:** Prevents executing inevitably failing swaps, saving gas on reverts *within* the flash loan.