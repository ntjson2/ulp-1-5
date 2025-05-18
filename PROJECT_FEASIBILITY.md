# REVISED V5: PROJECT_STATUS_ULP1.5.md

**Objective:** This document outlines the **current status and plan** for the ULP 1.5 cross-DEX arbitrage bot, built around a **Scalable Core system**. This system has been developed for initial deployment targeting **$400 - $800 / day** profit, with an architecture designed to support future upgrades towards **<5ms latency** and **$40k+/day** potential. Local simulation via Anvil is the primary method for current testing and verification.

---

## 1. Project Goal (Scalable Core)

Develop an efficient, reliable, and **highly scalable** arbitrage system (**ULP 1.5 Scalable Core**) for L2 execution.

*   **Phase 1 Goal (Current Focus):** Achieve and verify consistent profitability in the range of **$400 - $800 USD / day** using Balancer V2 0-fee flash loans ($10k - $500k range) on standard cloud infrastructure. Execute primarily 2-way arbitrage paths across key DEXs (UniV3, VeloV2/Aero). Current development is focused on **testing and refining** this phase.
*   **Phase 2 Goal (Enabled by Architecture):** Scale towards **$40k - $100k+ USD** average daily profit by leveraging the core architecture with infrastructure upgrades (co-location/direct data feeds for <5ms latency), potentially advanced multi-hop routing, and significantly larger flash loan capital ($1M - $5M+).

The core Rust application and Huff contract have been architected to support the transition to Phase 2 performance requirements without fundamental rewrites.

---

## 2. Profit Target Feasibility Analysis (Scalable Core)

### 2.1 Arguments For Feasibility (Phase 1: $400 - $800 / day):

*   **L2 Advantages:** Utilizes low fees and faster blocks on L2s where DEX inefficiencies persist.
*   **0-Fee Loans:** Leverages Balancer V2's 0-fee flash loans, making smaller spreads viable.
*   **Optimized Software Core:** Implemented with efficient Rust code, `DashMap` for concurrent state access, an **in-memory `PoolSnapshot` hot-cache** for fast lookups, optimized simulation logic, and a gas-efficient Huff contract.
*   **Essential MEV Protection:** Incorporates private transaction submission (via configurable RPCs) with public fallback.
*   **Moderate Loan Size:** Targeting $10k-$500k helps manage slippage.
*   **Local Simulation:** Anvil testing framework (`local_simulator.rs`, `integration_test.rs`) allows rigorous testing.

### 2.2 Arguments For Feasibility (Phase 2: $40k+ / day):

*   **Scalable Architecture:** The implemented core includes:
    *   **In-Memory Cache (`PoolSnapshot`):** Enables rapid state lookups.
    *   **Modular Components:** Key modules (`state`, `event_handler`, `path_optimizer`, `simulation`, `transaction`) allow independent refinement.
    *   **Huff Efficiency:** `ArbitrageExecutor.huff` minimizes on-chain gas/time, critical for high frequency.
    *   **Private Tx:** Foundational.
*   **Infrastructure Upgrade Path:** The design allows swapping standard RPCs/VPS for co-location/direct feeds.
*   **Market Opportunity:** Assumes large, fleeting opportunities exist and can be captured with Phase 2 speed/capital.

### 2.3 Challenges & Risks:

*   **Phase 1:** Competition, standard cloud latency (~50-150ms), infrastructure reliability (RPCs/VPS).
*   **Phase 2:** Extreme competition, difficulty achieving/maintaining <5ms edge, heightened execution/MEV risk, capital/slippage management, infrastructure cost/complexity, strategy decay.

### 2.4 Feasibility Verdict (Scalable Core):

*   **Phase 1 ($400 - $800 / day):** **Feasible.** The current implementation provides the necessary foundation. Success hinges on thorough testing, tuning, and reliable operation.
*   **Phase 2 ($40k+ / day):** **Challenging but Architecturally Enabled.** Requires significant investment in Phase 2 infrastructure and potentially advanced strategies. The current architecture provides the necessary starting point.

---

## 3. Project Setup Requirements (Current)

*   **Hardware:** Development Machine (WSL used), Standard Cloud VPS (Target for Phase 1 deployment).
*   **Software:** Rust toolchain, Foundry (Anvil), Huffc (`0.3.2` used), Git.
*   **Services:** Reliable WS & HTTP RPC endpoints (paid recommended), Private RPC endpoint (Flashbots Protect / MEV-Share compatible).
*   **Configuration (`.env`):** Contains RPCs, private keys, contract/token addresses (chain-specific!), gas parameters, loan amount limits, profit buffer settings, health check thresholds.

---

## 4. Project Structure & Workflow (Implemented)

### 4.1 Components:

*   **On-Chain (`ArbitrageExecutor.huff`):** Implemented and compiles. Handles 2-way swaps (UniV3/Velo-style). Includes `minProfitWei` check and `salt` nonce guard.
*   **Off-Chain (Rust Bot - `ulp1_5` binary):**
    *   **Core Modules:** `main.rs`, `config.rs`, `bindings.rs`, `gas.rs`, `encoding.rs`, `deploy.rs`, `utils.rs`.
    *   **`state.rs`:** Implements `AppState`, `PoolState` (detailed pool data), `PoolSnapshot` (**In-Memory Hot-Cache** with minimal data like reserves/sqrtPrice). Populated initially and updated by events.
    *   **`event_handler.rs`:** Listens to WS `Swap` and `PoolCreated` events. Updates the `PoolSnapshot` hot-cache rapidly. Spawns `check_for_arbitrage` task non-blockingly.
    *   **`path_optimizer.rs`:** Operates exclusively on the **`PoolSnapshot` hot-cache**. Identifies potential 2-way routes exceeding a threshold based on cached data. Generates `RouteCandidate` list (including factory addresses).
    *   **`simulation.rs`:** Simulates routes using **RPC calls** to DEX Quoters/Routers (`simulate_swap`) for accuracy. Calculates gross profit, estimates gas, calculates net profit, and finds optimal loan amount (`find_optimal_loan_amount`). UniV3 dynamic sizing is currently a **placeholder** using configured max loan.
    *   **`transaction.rs`:** Constructs flash loan transaction, encodes `userData` (including `minProfitWei` threshold and salt). Manages gas price fetching (EIP-1559) and nonce using `NonceManager`. Handles **private RPC submission** with public fallback. Monitors transactions using **polling**. Includes basic nonce error handling (resetting cache).
    *   **`local_simulator.rs`:** Framework for interacting with local Anvil fork (used by tests). Includes helpers for setup, triggering swaps, and fetching basic data.
    *   **`tests/integration_test.rs`:** Contains integration tests using `local_simulator` and Anvil. Includes tests for setup, swap triggers, a sequential full arbitrage cycle simulation, and placeholders for direct Huff verification.

### 4.2 Workflow (Implemented):

1.  **Initialization:** Load `.env` (`config.rs`). Connect WS/HTTP providers (`main.rs`). Init `AppState` (incl. empty cache), `NonceManager`. Deploy or load executor address. Fetch initial pool states via RPC, populating both `PoolState` map and `PoolSnapshot` hot-cache (`state.rs`). Subscribe to WS `Swap` and `PoolCreated` events (`main.rs`).
2.  **Event Loop (`main.rs`):** Uses `tokio::select!` to process WS events (logs, blocks) and run health checks.
3.  **Event Handling (`event_handler.rs`):** On `Swap`: Decode, **update `PoolSnapshot` hot-cache**, spawn `check_for_arbitrage`. On `PoolCreated`: Fetch state, add to both maps.
4.  **Arbitrage Check (`check_for_arbitrage` -> `path_optimizer::find_top_routes`):** Reads updated pool snapshot from hot-cache. Iterates through hot-cache comparing prices/reserves. Generates sorted `RouteCandidate` list if threshold met.
5.  **Simulation & Optimization (`simulation::find_optimal_loan_amount`):** Spawns simulation task for top candidate(s). Fetches gas price. Iteratively calls `calculate_net_profit` (which uses **RPC quotes** via `simulate_swap`). Calculates optimal loan, max net profit, and `minProfitWei` threshold.
6.  **Execution (`transaction::submit_arbitrage_transaction`):** If simulation shows profit: gets nonce, encodes `userData` (with `minProfitWei`, salt), constructs flash loan tx, signs, submits (private preferred), monitors via polling. Handles basic nonce errors.
7.  **On-Chain Execution (`ArbitrageExecutor.huff`):** Receives flash loan, performs swaps, checks `value >= required_return` (loan + fee + minProfit), approves repayment if profitable, otherwise reverts. Includes salt guard.
8.  **Profit Withdrawal:** Manual via `WITHDRAW_TOKEN` function (not tested).
9.  **Local Simulation Workflow (`tests/integration_test.rs`):** Uses Anvil forks and `local_simulator.rs` to test components and the sequential full cycle.
10. **Health Check (`main.rs`):** Periodically checks WS stream lag against configured thresholds (`CRITICAL_BLOCK_LAG_SECONDS`, `CRITICAL_LOG_LAG_SECONDS`). Calls `panic!` if thresholds exceeded (for external restart).

---

## 5. Key Success Factors (Current Focus)

*   **Cache Accuracy/Speed:** Ensuring the `PoolSnapshot` updates reliably and quickly.
*   **Simulation Accuracy:** Validating that `simulation.rs` RPC calls accurately predict on-chain results.
*   **Transaction Reliability:** Success rate of private + public submission, robustness of nonce management and gas estimation.
*   **Huff Logic Correctness:** Verifying the on-chain profit check and salt guard work as expected under various conditions.
*   **Testing Coverage:** Ensuring integration tests adequately cover main success and failure paths.

---

## 6. Current MU2 Sequence Status

*   **MU2.1 - Core State & Cache:** **Complete.**
*   **MU2.2 - Pathfinding & Simulation:** **Complete** (with placeholder for UniV3 dynamic sizing).
*   **MU2.3 - Huff v2 (`minProfitWei`) & Private Transactions:** **Complete** (Huff compiled, tx logic implemented).
*   **MU2.4 - Deployment, Monitoring & Basic DEX Expansion:** **In Progress (Testing Phase).** Core logic built, health checks added. Needs comprehensive Anvil test execution, refinement based on testing, initial deployment trial, and enhanced monitoring/alerting. Velo/Aero support added.
*   **MU2.5+ (Phase 2 Prep):** **Not Started.**

---

## 7. Current Status & Next Steps

*   **Status:** The core Rust application and Huff contract are implemented. Code compiles cleanly (`cargo check`). Integration test structure exists. Focus has been on fixing compilation issues (Rust & Huff) and implementing the sequential test cycle.
*   **Estimated Overall Completion:** ~89% (core dev done, testing needed).
*   **IMMEDIATE NEXT STEP:**  
    1. **Execute Anvil Integration Tests:**  
       ```bash
       cargo test --features local_simulation -- --ignored --nocapture
       ```  
    2. **Implement WS Event Loop Test:**  
       Develop and run `run_event_loop_ws_test` to validate real-time WS event handling and arbitrage triggering.
*   **Near-Term:** Refine based on test results, implement external alerting, prepare cloud VPS deployment and monitoring.
*   **Future:** Address lower-priority TODOs (UniV3 sizing), add more DEXs, explore Phase 2 infrastructure.