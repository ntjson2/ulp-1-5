
Strike a balance: create a foundation that is simple and cheap to run initially but incorporates architectural choices that *facilitate* scaling towards the higher profit targets later, without implementing the most complex/expensive parts (like co-location or full RouteGraph) right now.

We'll focus on:

1.  **Clean Architecture:** Modular code that allows replacing components later (e.g., swapping public RPC data source for a custom one).
2.  **Efficient Data Structures:** Using `DashMap` for state is good. We can *add* the hot-cache snapshot later if needed.
3.  **Optimized Core Logic:** Keep the Huff executor, event-driven approach, and efficient simulation.
4.  **Private Transactions:** Include this early as it's crucial for almost any level of serious arbitrage.
5.  **Local Simulation:** Add capabilities for local testing/simulation using Anvil.

---

## REVISED V3: PROJECT_FEASIBILITY_OPTIMIZED_CORE.md

**Objective:** This document outlines the plan for the ULP 1.5 cross-DEX arbitrage bot, focusing on building an **optimized core system** that is initially simple and inexpensive to run (targeting **$400 - $800 / day**) but architected for future scalability towards higher profit targets ($40k+/day). It leverages efficient algorithms, essential MEV protection, and supports local simulation, while deferring capital-intensive optimizations like co-location.

---

## 1. Project Goal (Optimized Core)

Develop an efficient, reliable, and **scalable** arbitrage system (**ULP 1.5 Optimized Core**) for L2 execution (Optimism, Base, Arbitrum initially). The system will capture and execute profitable arbitrage opportunities (initially 2-way paths) across key DEXs (UniV3, VeloV2) using Balancer V2 0-fee flash loans ($10k - $500k initial range) and a gas-optimized Huff executor. It will run on standard cloud infrastructure but incorporate architectural patterns allowing future performance enhancements. Local simulation via Anvil fork is a key development and testing feature.

**Primary KPI (Initial):** Achieve a target average daily profit ranging from **$400 USD to $800 USD** running on standard cloud infrastructure.
**Secondary Goal:** Ensure the architecture allows future upgrades (e.g., faster data sources, advanced routing, larger capital) to target $40k+/day without a full rewrite.

---

## 2. Profit Target Feasibility Analysis (Optimized Core)

### 2.1 Arguments For Feasibility ($400 - $800 / day):

*   Combines the advantages of the "Scaled Down" plan: L2 environment, DEX inefficiency, 0-fee loans, moderate loan size benefits.
*   **Optimized Core:** Even without co-location, using an efficient Rust implementation, `DashMap` for state, selective event processing, optimized simulation, and a gas-efficient Huff contract provides a strong baseline.
*   **Private Transactions:** Including Flashbots Protect/MEV-Share from the start is a significant advantage over purely public bots and essential for consistency, even at this scale.
*   **Scalable Design:** Key components (state management, event handling, transaction submission) are designed to potentially accommodate faster data or more complex routing later.
*   **Local Simulation:** Enables thorough testing and strategy refinement before deploying capital, reducing risk.

### 2.2 Arguments Against / Challenges ($400 - $800 / day):

*   **Competition:** Remains the primary challenge. Other bots exist with varying levels of sophistication.
*   **Latency (Standard Cloud):** The main limitation compared to high-end setups. Will miss the fastest opportunities. Requires focusing on spreads lasting slightly longer.
*   **Gas Costs & Frequency:** Still requires efficient execution and multiple successful trades daily.
*   **Infrastructure Reliability:** Dependence on public/paid RPCs and standard cloud VPS introduces potential reliability issues.

### 2.3 Arguments For Future Scalability ($40k+ / day):

*   **Modular Design:** Allows swapping data sources (e.g., public WS -> private node WS), transaction submission logic (e.g., adding more sophisticated MEV strategies), and pathfinding (e.g., adding RouteGraph).
*   **Core Efficiency:** The optimized Rust core and Huff executor remain valuable at higher scales.
*   **Private Tx Foundation:** Already in place, crucial for larger trades.

### 2.4 Feasibility Verdict (Optimized Core):

*   **$400 - $800 / day:** **Feasible and Realistic.** This target is achievable with the Optimized Core stack on standard infrastructure by focusing on readily available L2 opportunities and basic MEV protection.
*   **Future $40k+ / day:** **Achievable with Significant Upgrades.** The Optimized Core provides a *foundation*. Reaching the higher target **requires** successfully implementing the deferred "High-Leverage" upgrades: co-location for <20ms latency, potentially a RouteGraph or similar fast pathfinding for complex routes, advanced MEV strategies, and scaling capital deployment significantly ($5M+). The core architecture is designed *not to preclude* these upgrades.

---

## 3. Project Setup Requirements (Optimized Core)

*   **Hardware:**
    *   Development Machine (Linux/WSL/macOS).
    *   Standard Cloud VPS (e.g., AWS EC2, GCP, DO) for initial deployment. *No co-location initially.*
*   **Software:**
    *   Rust Toolchain.
    *   Foundry (`anvil`, `cast`). **Crucial for local simulation/testing.**
    *   Huff Compiler.
    *   Git.
    *   Monitoring/Alerting tools.
*   **Services:**
    *   Reliable Public/Paid Websocket RPC (paid recommended).
    *   Reliable Public/Paid HTTP RPC.
    *   Flashbots Protect RPC / MEV-Share endpoint (Alchemy recommended).
*   **Configuration (`.env`):** As before, including RPCs, keys, addresses, gas params, loan amounts ($10k-$500k range), etc. **Must include settings for local Anvil testing.**

---

## 4. Project Structure & Workflow (Optimized Core)

### 4.1 Components:

*   **On-Chain (`ArbitrageExecutor.huff`):** Optimized for 2-way swaps initially. Includes `minProfitWei` check loaded from `userData` for on-chain guardrails (Good practice even for smaller scale). Extensible later for multi-hop via dispatch logic if needed.
*   **Off-Chain (Rust Bot - `ulp1_5` binary):**
    *   **Core Modules:** `main.rs`, `config.rs`, `bindings.rs`, `event_handler.rs`, `AppState`, `PoolState`, `simulation.rs`, `gas.rs`, `encoding.rs`, `deploy.rs`.
    *   **`event_handler.rs`:** Efficiently updates `AppState` from WS events. Uses non-blocking tasks (`tokio::spawn`) for potentially long operations like simulation.
    *   **`AppState`:** Contains `Arc<DashMap<Address, PoolState>>`. *No RouteGraph initially.* Structure allows adding snapshots later.
    *   **`path_optimizer.rs`:** Performs efficient 2-way route identification by comparing updated pool state against relevant entries in the `DashMap`.
    *   **`simulation.rs`:** Optimized simulation using Quoters/Routers, calculates net profit considering gas (0 Balancer fee assumed).
    *   **`transaction.rs`:** Handles constructing and submitting transactions. Implements logic to use **private endpoints first**, falling back to public RPC if needed. Manages nonces and gas prices.
    *   **`local_simulator.rs` (New - Optional):** Contains functions or logic specifically for interacting with a local Anvil fork for testing purposes (e.g., setting balances, sending test transactions, advancing blocks). Could also be integrated into test suites.

### 4.2 Workflow (Optimized Core):

1.  **Initialization:** Load config, connect RPCs (including private), init `AppState`, fetch initial pool states, deploy/load executor, subscribe to WS events. Log Balancer 0 fee confirmation.
2.  **Event Loop:** Process WS events (Blocks, Logs).
3.  **Event Handling:** On `Swap`, update `AppState`, trigger `check_for_arbitrage` (non-blocking). On `PoolCreated`, add pool.
4.  **Arbitrage Check (`check_for_arbitrage` -> `find_top_routes`):**
    *   Calculate price for updated pool.
    *   Iterate `AppState` map, calculate price for relevant comparison pools.
    *   Identify pairs exceeding threshold.
    *   Generate and sort `RouteCandidate` list.
5.  **Simulation & Optimization (`find_optimal_loan_amount`):**
    *   For top candidate(s), spawn simulation task:
        *   Fetch current gas price dynamically.
        *   Simulate path for loan amounts ($10k-$500k).
        *   Calculate net profit = Gross Profit - Gas Cost.
        *   Find optimal loan amount & max profit.
6.  **Execution (`transaction.rs`):**
    *   If profitable optimal amount found:
        *   Encode `userData` (including a calculated `minProfitWei` based on simulation + buffer).
        *   Construct flash loan tx.
        *   Submit via **private endpoint** (e.g., `alchemy_sendPrivateTransaction`). Fallback to public on error.
        *   Monitor tx outcome.
7.  **On-Chain Execution (`ArbitrageExecutor.huff`):** Receives loan, executes swaps, checks `final_balance - loan >= minProfitWei`, approves Balancer if OK, else reverts.
8.  **Profit Withdrawal:** Manual via owner function.
9.  **Local Simulation Workflow:**
    *   Run `anvil --fork-url <MAINNET_L2_RPC>`.
    *   Configure `.env` to point `HTTP_RPC_URL` and `WS_RPC_URL` to `http://127.0.0.1:8545` and `ws://127.0.0.1:8545`.
    *   Set `DEPLOY_EXECUTOR=true` or deploy manually using `cast` and set address in `.env`. Use Anvil private key.
    *   Run the bot (`cargo run`). It connects to Anvil.
    *   Use `cast` or scripts (`local_simulator.rs` functions if implemented) to manually trigger swaps on the Anvil fork (e.g., `cast send <POOL_ADDRESS> "swap(...)" --private-key <ANVIL_KEY>`) to observe the bot's reaction, simulation, and potential (mock) execution attempts against the fork.
    *   Advance blocks on Anvil (`cast rpc evm_mine`) if needed.

---

## 5. Key Success Factors (Optimized Core)

*   **Efficiency & Reliability:** Core Rust logic, simulation accuracy, stable infrastructure (VPS, RPCs).
*   **Gas Optimization:** Huff contract, accurate gas estimation + limits.
*   **MEV Protection:** Effective use of private transaction submission.
*   **Robust Testing:** Thorough local simulation using Anvil forks.

---

## 6. Revised MU2 Sequence (Optimized Core Focus)

1.  **MU2.1 - Core Logic & Simulation:** Complete `path_optimizer.rs` (2-way). Ensure `simulation.rs` accurately calculates net profit (0 fee, gas cost). Integrate `event_handler` -> `path_optimizer` -> `simulation`. (Essentially finish Tasks 2.x + 3.0b/c + integrate calls). Implement basic local Anvil testing setup/docs. **Goal:** Bot identifies and accurately simulates opportunities locally.
2.  **MU2.2 - Huff v2 & Transaction Submission:** Refine `ArbitrageExecutor.huff` for 2-way swaps *with* `minProfitWei` check loaded from `userData`. Implement `transaction.rs` with private endpoint submission (Flashbots Protect/MEV-Share) + public fallback, dynamic gas pricing, and nonce management. **Goal:** Bot can successfully construct and submit potentially profitable transactions privately.
3.  **MU2.3 - Deployment, Monitoring & Basic DEX Expansion:** Deploy to cloud VPS. Set up logging/monitoring. Add support for 1-2 additional key DEXs (e.g., Aerodrome on Base if targeting Base). **Goal:** Bot runs reliably, executes small test trades successfully, handles >2 DEX types.
4.  **MU2.4 - Scaling & Refinement (Post-Initial):** Increase loan size limits based on performance. Add more DEXs. Optimize gas usage further. Potentially explore adding `PoolSnapshot` hot-cache or more complex routing *if* performance analysis indicates bottlenecks.

---

## 7. Prompt for AI Analysis (Optimized Core)

**Analyze the ULP 1.5 project described above, focusing on the *Revised V3: Optimized Core* goals and architecture.**

**Focus Areas:**

1.  **Feasibility Critique:** Evaluate the realism of the initial profit goal ($400-$800/day) using the Optimized Core stack ($10k-$500k loans, standard cloud VPS, private tx). Is the architecture suitable for *both* this initial goal *and* future scaling towards $40k+/day with planned upgrades (co-location, RouteGraph etc.)?
2.  **Architectural Soundness:** Does the Optimized Core provide a robust foundation? Is the balance between initial simplicity/cost and future scalability appropriate? Are the chosen components (DashMap, Huff, private tx) suitable?
3.  **Strategy Viability:** How effective are the core strategies at this initial scale? Is the deferral of co-location and RouteGraph acceptable initially? Is the local simulation workflow practical?
4.  **Risk Assessment:** What are the primary risks for this Optimized Core version? (e.g., latency impact, reliance on RPCs, competition in the mid-range).
5.  **Revised MU2 Sequence:** Does the revised 4-step MU2 sequence logically build the Optimized Core and prepare for future scaling?

**Provide a concise summary of your findings for this *Optimized Core* version, highlighting strengths, weaknesses, potential bottlenecks, and recommendations.**
```

---

**MU2 Task 3.0a (Feasibility Doc - Optimized Core V3) Complete.**

*   **File Changes:** Created `PROJECT_FEASIBILITY_OPTIMIZED_CORE.md` (content above).
*   **Project Estimation:** 31% (Strategic plan refined for optimized core with scalability path).
*   **Next Task:** We resume the adjusted plan: **MU2 Task 3.0b: Update `simulation.rs`** to remove flash loan fee logic (as planned before the feasibility detour).

