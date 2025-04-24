Okay, here is the complete handoff package formatted in Markdown.

```markdown
# BEGIN HANDOFF PACKAGE FOR ULP 1.5 (Elite Performance Refocus)

**(Instructions for AI receiving this context):**
*   This document contains the complete project state, code, and history for the ULP 1.5 Cross-DEX Arbitrage Bot up to the last interaction.
*   The project initially started with an RPC polling strategy but **pivoted to an event-driven WebSocket architecture**. Subsequently, based on user goals, the strategy was further refined towards **elite performance and competitiveness**, incorporating recommendations for near real-time detection and multi-DEX routing, aiming for significantly higher profit targets.
*   Please base all further analysis and development on this **new high-performance strategic direction** outlined in the updated documentation below (`ulp1.5.md`) and the provided codebase (which represents the state *before* full implementation of these advanced features). Ignore earlier descriptions involving basic RPC polling, simple WebSocket event handling, or the separate Node.js scanner (which has been archived/deprecated).
*   The Rust code provided compiled successfully (`cargo check`) in its last known state *before* attempting the elite strategy refactor, but significant changes are now required.
*   The immediate next step requested by the user is **MU1 - Task Bundle 8: Design and Implement Advanced Detection & Routing**, which involves planning and coding the core logic for near real-time data acquisition (Optimized RPC, Advanced WS, or Mempool) and multi-DEX pathfinding.
*   Please review the provided code and updated documentation thoroughly.

---

**1. Project Summary (Modernized `ulp1.5.md` - Elite Performance Refocus)**

# ULP 1.5 Status: Elite Cross-DEX Arbitrage System (High-Performance Architecture)

## 1. Project Objective & Use Case (Revised)

**Objective:** Develop "ULP 1.5", a **high-frequency, high-performance pure arbitrage system** optimized for L2 execution (Optimism, Base, Arbitrum). The system aims to maximize profit by capturing price discrepancies across **multiple DEXs (20+)** using Balancer V2 flash loans and near real-time data acquisition. Target profit potential: $1k - $40k/day.

**Use Case:** Execute atomic cross-DEX arbitrage trades (2-way and potentially simple 3-way) involving pairs like WETH/USDC across protocols such as Uniswap V3, Velodrome V2, Ramses, Aerodrome, SushiSwap, etc. by:
    1. **Near Real-Time Detection:** Monitoring blockchain state (via optimized RPC polling, WebSocket events, or potentially mempool scanning) across numerous DEXs and pairs for price divergences.
    2. **Pathfinding & Optimization:** Identifying the most profitable multi-hop arbitrage path (e.g., A->B on DEX X, B->A on DEX Y; or A->B->C->A across DEXs X, Y, Z) and calculating the **optimal flash loan amount** to maximize net profit considering fees, estimated gas, and simulated slippage.
    3. **Capital Acquisition:** Initiating a Balancer V2 flash loan for the optimal amount of the starting asset (e.g., WETH).
    4. **Atomic Execution:** Triggering the **ultra-low gas Huff executor contract** (`ArbitrageExecutor.huff`) which receives the loan and executes the pre-calculated sequence of swaps across the specified DEXs within a single transaction.
    5. **On-Chain Verification:** The Huff contract performs a final profit check (`final_balance > loan + fee`) before approving repayment.
    6. **Repayment & Profit:** Balancer Vault automatically repays the loan+fee upon successful execution; profit remains in the executor contract.
    7. **Submission & Monitoring:** Submitting transactions via **priority RPC endpoints** (e.g., Alchemy) or potentially L2 private relays (like Flashbots). Monitoring transaction inclusion and outcome.

**Constraint:** Focus solely on pure arbitrage based on *existing* price differences detectable via on-chain state or near real-time events. No predatory MEV (sandwiching, generalized front-running). Simple back-running or non-predatory "just-in-time" (JIT) liquidity provision / rebalancing via flash loans *might* be considered if aligned with pure arb principles.

## 2. Core Logic & Architecture (High-Performance)

The system requires highly optimized off-chain and on-chain components:

1.  **On-Chain Huff Executor (`ArbitrageExecutor.huff` - Needs Enhancement):**
    *   Gas-optimized contract receiving Balancer V2 flash loans.
    *   **Current:** Supports conditional 2-way swaps (UniV3/VeloV2) based on `userData`. Includes profit check and withdrawal.
    *   **TODO Enhancement:** Needs to be made more generic or extended to support:
        *   Arbitrary numbers of swaps (at least 3 for triangular).
        *   Routing calls to a wider variety of DEX router/pool interfaces (UniV3, Velo/Aero, Curve, Balancer, Sushi etc.).
        *   Potentially more complex `userData` structure to encode multi-hop paths and DEX types/routers per hop.
        *   (Optional) Fallback paths or minimum output checks within Huff for added robustness.

2.  **Off-Chain Rust Bot (`ulp1_5` binary - Major Refactor Needed):**
    *   **Modular Structure:** Maintain existing modules, likely add `state_cache`, `pathfinder`, `tx_submitter`, `mempool_monitor` (optional).
    *   **Configuration (`config.rs`, `.env`):** Expand to include multiple RPC endpoints (WS/HTTP, primary/failover, potentially mempool access), addresses for numerous DEX factories/routers, token lists, pathfinding parameters, advanced gas settings.
    *   **Data Acquisition (CRITICAL TODO):** Replace current simple event/poll model with a high-throughput method:
        *   **Option A (Optimized RPC):** Batch RPC calls (`eth_call` with multicall contract, or direct state reads if possible) polling `slot0`/`reserves` for hundreds of pools very frequently (sub-second). Requires highly optimized RPC provider usage and potentially self-hosted nodes.
        *   **Option B (WebSocket Events - Advanced):** Subscribe to `Swap` events for *all* potentially relevant pools (hundreds/thousands). Requires extremely efficient event decoding and state updating. Robust handling of dropped connections and reorgs is critical.
        *   **Option C (Mempool Monitoring - MEV-like):** Subscribe to pending transactions via WebSocket (`newPendingTransactions`) or dedicated mempool services (Flashbots, BloXroute). Decode pending swaps to anticipate state changes. Highest performance, highest complexity, borders on MEV.
    *   **State Cache (`state_cache.rs` - TODO):** Robust, high-performance, concurrent in-memory cache (`DashMap` or specialized structure) holding the latest prices/liquidity for all monitored pools across target DEXs. Needs efficient updates from the chosen data acquisition method.
    *   **Pathfinding (`pathfinder.rs` - TODO):**
        *   Constantly evaluate potential 2-way and 3-way arbitrage paths using data from the state cache.
        *   Needs efficient graph traversal or matrix-based calculation to check hundreds/thousands of potential paths (e.g., Pair AB on DEX X vs DEX Y, Pair AC on DEX X vs DEX Z, Pair BC on DEX Y vs DEX Z -> Triangular A->B->C->A).
        *   Must quickly estimate gross profit for promising paths.
    *   **Optimization & Simulation (`simulation.rs`):**
        *   Triggered by the pathfinder for high-potential paths.
        *   `find_optimal_loan_amount` remains crucial for maximizing profit vs slippage.
        *   `calculate_net_profit` needs access to real-time gas estimates and accurate simulation.
        *   `simulate_swap` must be extended or generic enough to support quoting on all target DEX types.
    *   **Gas Estimation (`gas.rs`):** Must accurately estimate gas for potentially complex multi-hop paths within the Huff contract.
    *   **Transaction Construction & Submission (`tx_submitter.rs` - TODO):**
        *   Encode complex `userData` for the multi-hop Huff executor.
        *   Implement robust EIP-1559 fee calculation.
        *   Set appropriate buffered gas limits.
        *   Submit via primary RPC (e.g., Alchemy Priority) with failover to secondary RPC or private relay. Handle nonce management carefully.
    *   **Monitoring & Confirmation (`main.rs`):** Track submitted transactions, handle confirmations/reverts. Log extensively.

## 3. Key Technologies & Patterns (Enhanced)

*   **Runtime:** `tokio` (highly optimized for I/O).
*   **EVM Interaction:** `ethers-rs` (WS/HTTP Providers, SignerMiddleware, bindings, ABI utils). Potential use of lower-level RPC libraries or direct socket interaction for latency gains.
*   **State:** `dashmap` or potentially more specialized concurrent data structures. Possibly `redis` or other in-memory DB for state persistence/sharing across processes if scaling horizontally.
*   **Logging:** `tracing` framework mandatory.
*   **Data Acquisition:** WebSockets (`eth_subscribe`) or highly optimized batched RPC polling. Mempool data feeds (optional, advanced).
*   **On-Chain:** Huff (essential for gas edge).
*   **Pathfinding:** Graph algorithms or optimized matrix operations.
*   **Transaction Submission:** Priority RPCs, potential use of private relays (e.g., Flashbots Protect/MEV-Share equivalents on L2s if available).

## 4. Current Status & TODOs (Major Refactor Required)

*   **Foundation:** Modular Rust structure exists, basic WS connection, Huff contract deployment, single-pair simulation/optimization/execution logic implemented (but based on slower event model). Config loading exists.
*   **Current Code State:** Compiles cleanly but represents the *previous* simple event-monitoring architecture, **not** the high-performance multi-DEX goal.
*   **Major TODOs (High Priority):**
    1.  **Implement High-Speed Data Acquisition:** Choose and implement Option A, B, or C for getting pool state data across many DEXs/pairs near real-time. This is the most critical change.
    2.  **Implement State Cache:** Design and build the concurrent `AppState` cache updated by the chosen data acquisition method.
    3.  **Implement Pathfinder:** Develop the logic to efficiently scan the state cache for 2-way and 3-way arbitrage opportunities across multiple DEXs/pairs. It should output potential paths ranked by estimated gross profit.
    4.  **Enhance Huff Executor:** Generalize the Huff contract to handle arbitrary 2-hop or 3-hop paths across different DEX interfaces passed via `userData`.
    5.  **Enhance `simulation`/`gas`:** Adapt `simulate_swap` and `estimate_flash_loan_gas` to handle multi-hop paths and different DEX types accurately.
    6.  **Implement Tx Submitter:** Create robust logic for sending transactions with EIP-1559 params, gas limits, and failover/retry mechanisms.
    7.  **Integrate Components:** Connect the Pathfinder output to trigger Optimization -> Simulation -> Final Check -> Tx Submission flow.

## 5. Infrastructure Recommendation (Revised)

*   **Node Access:** Self-hosted, optimized L2 nodes (e.g., Erigon/Nethermind/Reth) with direct WebSocket/IPC access strongly recommended for lowest latency data acquisition. Supplement with high-tier managed RPC providers (Alchemy/Infura/etc.) for redundancy and potentially sending transactions. Consider mempool services (BloXroute, etc.) if pursuing Option C.
*   **Bot Server:** Low-latency VPS co-located geographically close to main L2 sequencers and your primary nodes/RPC providers (e.g., US East). Need sufficient CPU/RAM for Rust bot computation (especially pathfinding).
*   **Monitoring:** Prometheus + Grafana essential for tracking bot health, RPC/WS status, detected opportunities, attempted trades, success rates, P&L, slippage, latency.

```

---

**2. Current Codebase**

*(Instruction to AI: The user needs to provide the complete file contents corresponding to the state after MU1 Task Bundle 7, where `cargo check` passed cleanly. These are the files to request or assume are available):*

*   `Cargo.toml`
*   `bot/src/main.rs`
*   `bot/src/config.rs`
*   `bot/src/utils.rs`
*   `bot/src/simulation.rs`
*   `bot/src/bindings.rs`
*   `bot/src/encoding.rs`
*   `bot/src/deploy.rs`
*   `bot/src/gas.rs`
*   `bot/src/event_handler.rs`
*   `contracts/ArbitrageExecutor.huff`
*   `abis/ArbitrageExecutor.json`
*   `abis/BalancerVault.json`
*   `abis/IUniswapV3Factory.json`
*   `abis/IVelodromeFactory.json` *(User must ensure this is valid)*
*   `abis/QuoterV2.json`
*   `abis/UniswapV3Pool.json`
*   `abis/VelodromeRouter.json`
*   `abis/VelodromeV2Pool.json`

---

**3. Configuration (`.env` Structure - Expanded)**

```dotenv
# --- Network & Keys ---
WS_RPC_URL="ws://..." # REQUIRED: Primary WebSocket endpoint for events/fast polling
HTTP_RPC_URL="http://..." # REQUIRED: Reliable HTTP endpoint for signer/fallbacks
HTTP_RPC_URL_SECONDARY="http://..." # Optional: Failover HTTP endpoint
# PRIVATE_RELAY_URL="http://..." # Optional: e.g., Flashbots URL if used on L2
LOCAL_PRIVATE_KEY="0x..." # Bot's private key

# --- Deployment Settings ---
DEPLOY_EXECUTOR="true" # or "false"
EXECUTOR_BYTECODE_PATH="./build/ArbitrageExecutor.bin" # if DEPLOY_EXECUTOR=true
# ARBITRAGE_EXECUTOR_ADDRESS="0x..." # if DEPLOY_EXECUTOR=false

# --- Factory Addresses (Add for all target DEXs/Chains) ---
# Optimism
OP_UNISWAP_V3_FACTORY_ADDR="0x1F98431c8aD98523631AE4a59f267346ea31F984"
OP_VELODROME_V2_FACTORY_ADDR="0x25CbdDb98b35AB1FF795324516342Fac4845718f"
# OP_SUSHI_V3_FACTORY_ADDR="0x..." # Example
# Base
# BASE_AERODROME_V2_FACTORY_ADDR="0x..." # Example
# BASE_UNISWAP_V3_FACTORY_ADDR="0x..." # Example
# Arbitrum
# ARB_RAMSES_V2_FACTORY_ADDR="0x..." # Example
# ARB_UNISWAP_V3_FACTORY_ADDR="0x..." # Example

# --- Target Token Pair (Set ONLY if restricting to one pair initially) ---
# TARGET_TOKEN_A="0x4200000000000000000000000000000000000006" # e.g., WETH
# TARGET_TOKEN_B="0x7f5c764cbc14f9669b88837ca1490cca17c31607" # e.g., USDC (6 decimals)
# TARGET_TOKEN_A_DECIMALS="18"
# TARGET_TOKEN_B_DECIMALS="6"

# --- Other Contract Addresses (Per Chain) ---
OP_VELO_V2_ROUTER_ADDR="0xa062ae8a9c5e11aaa026fc2670b0d65ccc8b2858"
OP_QUOTER_V2_ADDRESS="0xb27308f9F90D607463bb33eA1BeBb41C27CE5AB6"
OP_BALANCER_VAULT_ADDRESS="0xBA12222222228d8Ba445958a75a0704d566BF2C9"
# Add others for Base, Arbitrum as needed

# --- Optimization Settings ---
MIN_LOAN_AMOUNT_ASSET="0.1" # Make asset name dynamic later (e.g., WETH)
MAX_LOAN_AMOUNT_ASSET="50.0"
OPTIMAL_LOAN_SEARCH_ITERATIONS="8"

# --- Gas Pricing Settings ---
MAX_PRIORITY_FEE_PER_GAS_GWEI="0.5"
GAS_LIMIT_BUFFER_PERCENTAGE="25"
MIN_FLASHLOAN_GAS_LIMIT="250000"

# --- Bot Parameters ---
HEALTH_CHECK_INTERVAL_SECS="60"
# MIN_NET_PROFIT_USD="5.0" # Example

# --- Logging ---
RUST_LOG="info,ulp1_5=debug" # Adjust level (e.g., trace for deep debug)
```

---

**4. Build/Run Instructions**

1.  **Prerequisites:** Rust, Foundry (`anvil`, `cast`), `huffc`.
2.  **Environment:** Create/populate `.env`. Ensure ABI JSON files are in `./abis/`. Ensure `WS_RPC_URL` is correct.
3.  **Compile Huff:** `huffc ./contracts/ArbitrageExecutor.huff -b -o ./build/ArbitrageExecutor.bin`.
4.  **Build Rust:** `cargo build --release`.
5.  **Run Anvil (Local):** `anvil --fork-url <YOUR_HTTP_RPC> --chain-id <ID>`
6.  **Fund Wallet (Anvil):** Send test ETH if deploying/sending.
7.  **Run Bot:** `RUST_LOG=info,ulp1_5=debug cargo run --release --bin ulp1_5` (Withdrawal mode temporarily removed during refactor, can be added back later).

---

**5. Current Status Summary**

*   **Build:** `cargo check` passed cleanly (as of user's last confirmation).
*   **Architecture:** Foundational event-driven structure exists but is **incomplete and non-functional** for the high-performance goal. Key components like data acquisition, state cache, pathfinding, and the execution trigger are placeholders or missing.
*   **Functionality Implemented:** Basic WS connection, block/log subscription *setup*, simulation/optimization *functions* defined (but not properly triggered by events), basic gas/tx logic defined.
*   **Gap:** Requires **major implementation effort** to build the high-speed data handling, state management, pathfinding, and integration logic described in the updated Architecture section. The Huff contract also needs generalization.

---

**6. Guidance for AI & Next Steps**

*   **Focus:** Implement the **High-Performance Architecture** described in the updated `ulp1.5.md`.
*   **Immediate Task (MU1 Task Bundle 8): Design and Implement Advanced Detection & Routing:**
    1.  **Choose Data Acquisition Strategy:** Decide: Optimized RPC polling, Advanced WS Events, or Mempool. Discuss trade-offs.
    2.  **Implement State Cache:** Design `AppState`/`PoolState` for multi-DEX data. Implement update logic for the chosen data strategy.
    3.  **Implement Pathfinder:** Design algorithm (`pathfinder.rs`) to scan cache for 2/3-way arbs.
    4.  **Refactor Execution Trigger:** Design how Pathfinder results trigger `find_optimal_loan_amount` and transaction submission (e.g., dedicated task queue).
    5.  **Plan Huff Contract V3:** Outline required changes for multi-hop/multi-DEX support.

---

**END OF HANDOFF PACKAGE (Elite Performance Refocus)**
```