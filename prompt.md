# ULP 1.5 Arbitrage Bot - Project Transfer & Continuation Prompt

## 1. Project Goal & Vision

The ultimate goal of ULP 1.5 is to develop a highly performant and profitable arbitrage bot operating across multiple Layer 2 (L2) networks. It leverages Balancer V2 flash loans for capital efficiency and executes atomic arbitrage trades using a custom, gas-optimized Huff contract (`ArbitrageExecutor.huff`).

## 2. Project History & Pivot

*   **Initial Vision:** Aimed for very high-frequency trading ($40k+/day target) potentially requiring advanced infrastructure (e.g., co-location).
*   **Pivot ("Scalable Core"):** The strategy shifted to first building a robust, reliable, and efficient core system deployable on standard cloud infrastructure. This core system is designed to be scalable for future performance enhancements.

## 3. Current Phase Goals

*   **Phase 1 (Current Focus):** Achieve reliable $400-$800/day profit using flash loans ($10k-$500k range) on standard infrastructure. Prove the core logic and execution flow.
*   **Phase 2 (Future):** Scale towards the original $40k+/day vision by leveraging the core architecture, potentially incorporating ultra-low latency data/execution paths, larger capital, more complex routing, and broader L2/DEX support.

## 4. Core Architecture & Technology

*   **Language/Runtime:** Rust using `tokio` for asynchronous operations.
*   **Blockchain Interaction:** `ethers-rs` library for interacting with Ethereum-compatible L2 networks.
*   **Data Source:** WebSocket (`ws`) subscriptions to node RPC endpoints for real-time `Swap` and `PoolCreated` events. HTTP (`http`) endpoint used for signing client and potentially some state lookups.
*   **State Management (`state.rs`):**
    *   `AppState`: Central, shared state wrapped in `Arc`. Contains configuration, contract instances, and maps for pool data.
    *   `PoolState` (DashMap): Stores quasi-static pool details fetched on startup or discovery (DEX type, tokens, fees, stability flag).
    *   `PoolSnapshot` (DashMap): Acts as an in-memory "hot-cache" storing frequently updated data derived from events (reserves, sqrtPrice, tick). Crucial for low-latency pathfinding.
*   **Arbitrage Pipeline:**
    1.  **Event Handling (`event_handler.rs`):** Listens for `Swap` and `PoolCreated` events. Updates `PoolSnapshot` hot-cache immediately upon `Swap`. Spawns non-blocking tasks (`check_for_arbitrage`) for further processing. Handles `PoolCreated` by fetching and caching full pool state.
    2.  **Pathfinding (`path_optimizer.rs`):** Triggered after a `Swap` potentially changes prices. Reads *only* from the `PoolSnapshot` hot-cache to quickly compare the price of the updated pool against other cached pools. Identifies potential 2-way (A->B->A) arbitrage routes exceeding a basic percentage threshold. Generates `RouteCandidate` structs.
    3.  **Simulation (`simulation.rs`):** Evaluates promising `RouteCandidate`s in spawned tasks. Uses RPC calls (`QuoterV2` for UniV3, Router `getAmountsOut` for Velo/Aero) to get accurate swap simulations. Performs dynamic loan sizing based on V2/Aero reserves (from snapshots) capped by config. Estimates gas costs (`eth_estimateGas`) and calculates net profit.
    4.  **Transaction (`transaction.rs`):** If simulation shows profit, this module constructs and submits the transaction. Encodes `userData` including swap details, profit threshold (`minProfitWei`), and a unique `salt` nonce for replay protection. Fetches EIP-1559 gas prices. Calculates final gas limit. Uses `NonceManager` for sequential nonce handling. Submits transaction via private relays (Alchemy/Flashbots preferred) with public RPC fallback. Monitors transaction confirmation status.
*   **On-Chain Executor (`ArbitrageExecutor.huff` v2.3.0):**
    *   Receives Balancer V2 flash loan (`receiveFlashLoan`).
    *   Decodes `userData` passed from the bot.
    *   **Salt Nonce Guard:** Checks if the `salt` in `userData` has been seen before (using `sload` on a mapping) to prevent replays; reverts if seen. Marks the salt as seen (`sstore`) if new.
    *   Performs the two swaps (Swap A -> Swap B) on specified pools (UniV3 or Velo-style). Handles routing logic differences between DEX types.
    *   **Profit Check:** Verifies `final_token0_balance >= loan_amount + minProfitWei` (where `minProfitWei` comes from `userData`). Reverts if check fails.
    *   Approves Balancer Vault to pull the repayment amount (`loan_amount + fee(0)`) only if all checks pass.
    *   Returns control to Balancer Vault. Atomicity ensures the entire sequence reverts if any step fails or checks don't pass.

## 5. Current Implementation Details

*   **Target L2s:** Optimism, Base (with ABIs/logic primarily based on Optimism Velodrome V2, reused for Base Aerodrome).
*   **Target DEXs:** Uniswap V3, Velodrome V2, Aerodrome.
*   **Target Pair:** WETH/USDC (Addresses and decimals configured in `.env`).
*   **Flash Loans:** Balancer V2 (Assumed 0 fee).
*   **Executor Features:** Includes profit check and salt nonce guard.

## 6. Configuration (`.env`, `config.rs`)

*   Network RPC URLs (WS, HTTP)
*   Private Key
*   Contract Addresses (Factories, Routers, Executor, Tokens, Balancer Vault, Quoter)
*   Token Decimals
*   Deployment Options (Deploy executor or use existing address)
*   Optimization Parameters (Loan amounts, search iterations, timeouts)
*   Gas Settings (Max priority fee, fallback, buffer, minimum limit)
*   Private Relay URLs

## 7. Testing Strategy

*   Primary testing method relies on local network forks using **Foundry's Anvil**.
*   Fork a target L2 (e.g., `anvil --fork-url <OPTIMISM_RPC>`).
*   Configure `.env` to point to the local Anvil instance (`http://127.0.0.1:8545`).
*   Deploy the `ArbitrageExecutor.huff` contract to the Anvil fork (either manually via `cast send --create` or automatically via bot config).
*   Run the bot against the Anvil fork.
*   Trigger `Swap` events on the fork using `cast send` against relevant pool/router contracts to simulate market activity and observe the bot's reaction (event detection, pathfinding, simulation, transaction submission attempts against Anvil).
*   Use Anvil's tracing (`cast run --debug` or `--steps-tracing`) to verify Huff contract execution logic (guards, swaps, profit check).

## 8. Current Project Status (End of Previous Session)

*   **Compilation:** The codebase **compiles successfully** (`cargo check` passes).
*   **Warnings:** Two acceptable warnings remain:
    *   `dead_code` for `DexType::Unknown` variant (kept for robustness).
    *   `unused_field` for `PoolState::token1` (kept for context).
*   **Functionality:** Core logic for event handling, pathfinding, simulation (with dynamic V2 sizing), transaction submission (with private relay support), and on-chain execution (with guards) is implemented.

## 9. Immediate Next Task

*   **MU2 Task 5.3 (Testing & Refinement) - Enhance `README.md` with detailed Anvil Testing Procedures.**
    *   Document specific `cast send` commands or test scenarios needed to verify the following on an Anvil fork:
        *   Event detection (`Swap`, `PoolCreated`) and corresponding state updates (`PoolSnapshot`, `PoolState`).
        *   Pathfinding logic correctly identifying routes based on simulated price changes.
        *   Simulation accuracy compared to actual swap results on Anvil.
        *   Dynamic loan sizing behavior (V2/Aero reserve limits, UniV3 config limits).
        *   Transaction submission flow (using public submission against Anvil).
        *   Huff contract execution correctness via Anvil tracing/debugging (verifying profit guard, salt guard, swap execution).

## 10. Subsequent Tasks

*   Perform the actual Anvil testing based on the procedures documented in the README.
*   Implement more robust transaction monitoring (beyond basic confirmation) and potentially nonce error recovery/resynchronization strategies.
*   Refine the `minProfitWei` buffer calculation in `transaction.rs`.
*   (Lower Priority) Implement accurate UniV3 dynamic loan sizing based on tick liquidity.
*   (Lower Priority) Add support for more DEXs (e.g., Ramses on Arbitrum).
*   (Lower Priority) Integrate external alerting mechanisms for success/failure/errors.

## 11. Interaction Instructions

*   Please review this context.
*   The immediate goal is to work on **Task 9: Enhance `README.md` with Anvil Testing Procedures.**
*   If errors arise during development or testing, we can revert to the structured "j9 go" error-fixing process if helpful.
*   Provide complete file contents when modifications are requested.