# ULP 1.5 Arbitrage Bot - Project Summary & State (Scalable Core v4)

## 1. Project Goal & History

*   **Initial Goal:** High-frequency, high-profit ($40k+/day) L2 arbitrage using advanced techniques (co-location, complex routing).
*   **Pivot (Scalable Core):** Refocused on building a robust, efficient core system first.
    *   **Phase 1 Goal:** Achieve reliable $400-$800/day profit on standard cloud infrastructure using $10k-$500k flash loans (Balancer V2, 0-fee assumed).
    *   **Phase 2 Goal:** Enable scaling towards $40k+/day by leveraging the core architecture with future infrastructure/strategy upgrades (e.g., <5ms latency via co-location, larger capital, advanced routing).
*   **Current Focus:** Target L2s (Optimism, Base initially), WETH/USDC pair, 2-way arbitrage paths via Uniswap V3, Velodrome V2, and Aerodrome.

## 2. Core Architecture & Features

*   **Backend:** Event-driven Rust application using `tokio` and `ethers-rs`.
*   **Data Source:** WebSocket subscriptions to DEX Swap/PoolCreated events via public/paid RPC endpoints.
*   **State Management (`state.rs`):**
    *   `AppState`: Central shared state using `Arc`.
    *   `PoolState`: Map (`DashMap`) holding detailed pool context (tokens, fees, stability, etc.), fetched on startup/discovery.
    *   `PoolSnapshot`: Map (`DashMap`) acting as an **in-memory hot-cache** holding frequently updated data (reserves, sqrtPrice, tick), updated directly from `Swap` events or reserve fetches.
*   **Arbitrage Flow:**
    1.  **Event Handling (`event_handler.rs`):** Receives swap events, rapidly updates the `PoolSnapshot` hot-cache. Spawns non-blocking task for `check_for_arbitrage`. Handles `PoolCreated` events, triggers state fetching.
    2.  **Pathfinding (`path_optimizer.rs`):** Triggered by `check_for_arbitrage`. Reads *only* from the `PoolSnapshot` hot-cache for speed. Compares updated pool price (derived from cache) against other cached pool prices. Identifies 2-way routes exceeding a basic percentage threshold. Generates `RouteCandidate` structs.
    3.  **Simulation (`simulation.rs`):** Triggered by `check_for_arbitrage` for promising `RouteCandidate`s (spawned task).
        *   Uses `Arc<AppState>` and `Arc<Client>` to initialize DEX Quoters/Routers *internally* based on config addresses.
        *   Calls `QuoterV2::quote_exact_input_single` or Router `getAmountsOut` via **RPC** for accurate swap amount predictions.
        *   Performs **dynamic loan sizing** based on V2/Aero reserves (using `PoolSnapshot` data passed down) via `calculate_dynamic_max_loan`. UniV3 sizing uses a configurable placeholder (defaults to config max).
        *   Performs iterative search (`find_optimal_loan_amount`) within dynamic bounds to find max profit.
        *   Estimates gas cost using `eth_estimateGas`.
        *   Calculates net profit (Gross - Gas).
    4.  **Transaction (`transaction.rs`):** Triggered if simulation finds positive profit.
        *   Calculates `minProfitWei` threshold (simulated profit - buffer).
        *   Generates unique `salt`.
        *   Encodes `userData` (pools, direction, flags, `minProfitWei`, `salt`).
        *   Fetches dynamic EIP-1559 gas prices (with fallback).
        *   Estimates final gas limit (with buffer/minimum).
        *   Retrieves next nonce from `NonceManager`.
        *   Constructs and signs the `flashLoan` transaction.
        *   **Private Submission:** Attempts submission sequentially via configured primary/secondary private relays (supports Alchemy `alchemy_sendPrivateTransaction` and Flashbots `eth_sendPrivateRawTransaction` methods based on URL heuristic) with fallback to public `eth_sendRawTransaction`.
        *   **Monitoring:** Basic monitoring awaits transaction confirmation with timeout, logs success/revert/drop status and gas used. Basic nonce error detection and cache invalidation implemented. Includes `ALERT:` log prefixes for critical events.
*   **On-Chain (`ArbitrageExecutor.huff` v2.3.0):**
    *   Receives Balancer flash loan.
    *   Performs 2-way swaps (UniV3 / VeloV2-style).
    *   Checks received `salt` against storage mapping (`SALT_SEEN_MAPPING_SLOT`) to prevent replays.
    *   Checks `final_balance >= loan_amount + minProfitWei` (loaded from `userData`).
    *   Approves Balancer repayment only if checks pass.
    *   Reverts otherwise, ensuring atomicity.
*   **Testing:** Local simulation workflow documented using Anvil forks (`local_simulator.rs`).

## 3. Current Task & Next Steps

*   **Last Completed:** MU2 Task 4.3 (Implemented specific Flashbots relay logic in `transaction.rs`).
*   **Current State:** Codebase includes core features for Phase 1. Most structural elements are in place. Compiles unsuccessfully after recent error fixing.
*   **Immediate Next Task (j8):** **MU2 Task 5.3 (Testing & Refinement) - Enhance `README.md` with detailed Anvil Testing Procedures.** Document specific `cast send` commands or test scenarios to verify:
    *   Event detection and state updates.
    *   Pathfinding logic.
    *   Simulation accuracy.
    *   Dynamic loan sizing behavior.
    *   Transaction submission (publicly against Anvil fork).
    *   Huff contract execution (profit/salt guards) via Anvil tracing/debugging.
*   **Subsequent Tasks:**
    *   Perform actual Anvil testing based on the documented procedures.
    *   Implement robust transaction monitoring & nonce error recovery.
    *   Refine `minProfitWei` buffer calculation.
    *   (Lower Priority) Implement UniV3 dynamic loan sizing.
    *   (Lower Priority) Add more DEXs (Ramses).
    *   (Lower Priority) Integrate external alerting.

## 4. Project Goal Alignment

The current state provides a solid foundation for **Phase 1** ($400-$800/day). Key elements for **Phase 2** ($40k+/day) like the hot-cache structure and modular transaction submission are present, allowing future integration of ultra-low latency data sources/submission paths and advanced routing without a full rewrite. The immediate focus on testing ensures the core logic is sound before scaling or adding complexity.