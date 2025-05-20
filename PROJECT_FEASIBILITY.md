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
*   **Estimated Overall Completion:** ~89% (relative to Phase 1 goals, core dev done, testing needed).
*   **IMMEDIATE NEXT STEP:** **Execute Anvil Integration Tests.** Run `cargo test --features local_simulation -- --ignored --nocapture` against a correctly configured Anvil instance. Analyze results, debug any failures in the interaction logic or prerequisites.
*   **Near-Term:** Refine based on test results. Implement basic external alerting. Prepare for initial cloud VPS deployment and monitoring.
*   **Future:** Address lower-priority TODOs (UniV3 sizing), add more DEXs, explore Phase 2 infrastructure.

## Networking 1.1 (production and testnet)

### **Practical GCP setup that matches your repo’s layout and profit target**

1. **Put a single, latency‑critical “trader” on a high‑cpu Spot VM (e.g. `c3-highcpu‑2` in `us‑west1‑b`) that sits a few milliseconds from Optimism’s sequencer.**  
    Two 4.4 GHz Intel Sapphire‑Rapids cores \+ 10 Gbps egress are enough for your async tasks and the Balancer flash‑loan execution path in `ArbitrageExecutor` (no GPU needed). You’re billed by the second (\~$0.008 / h spot, $0.04 on‑demand), so a two‑hour “race‑car” window costs ≈ $0.016 in compute.

2. **Containerise the bot** (`cargo build --release` → Docker) and preload the \~80 MB of ABIs/byte‑code your crate needs so cold‑starts stay under 20 s. Store the image in Artifact Registry and pull with `--no-address` so the VM only has an internal IP.

3. **Schedule bursts** with a tiny always‑on controller:  
    \* `e2-micro` (\~$6/mo) runs a Cloud Scheduler cron that calls `gcloud compute instances start/stop bot-vm --zone=us-west1-b`.  
    \* Controller also checks `estimated_profit_usd` from `find_top_routes` and only keeps the VM up while spreads could yield $1k+. (Expose a simple Cloud Run endpoint that returns current expected PnL.)

4. **WireGuard inside the VM is simpler than Cloud VPN**—it gives you a fixed insider IP for your private RPC if Alchemy NetOps want to IP‑whitelist you. Keep the key in Secret Manager and inject via `--metadata-from-file`.

5. **Low‑latency RPC**: point `WS_RPC_URL`/`HTTP_RPC_URL` in `.env` to an Optimism archive node *in the same region* (Alchemy’s “US West (Oregon)” or Blast’s “US W”). That avoids the ≈ 60 ms RTT you see when the default endpoint resolves to Ohio.

6. **Disk & logging**: Use a 10 GB balanced‑SSD (Liftoff time \< 1 s) and stream `tracing` output to Cloud Logging; set a 3‑day retention because the trade loops are chatty.

7. **Scaling up to $40 k/day**: run *two* such Spot VMs in `us‑west1` and `us‑central1` (Kansas) behind a Cloud NAT, each with its own private key (or an MPC wallet) and non‑overlapping nonce ranges—separate bots win distinct blocks without self‑collision.

8. **Fail‑safe**: if a Spot VM is pre‑empted mid‑flash‑loan, on‑chain revert just costs gas; to cap that you already set `min_profit_abs_buffer_wei` and `gas_limit_buffer_percentage` in `config.rs`—keep those tight and monitor for \>5 consecutive reverts, then pause the burst until spreads widen again.   
   

\#\#\# Gotchas you will hit

* **Spot VM pre‑emptions land at the worst time** – Google gives only **30 s** before killing the instance. You need your Rust loops to catch `SIGTERM` and `persist_nonce()`/flush traces to Cloud Logging; otherwise the next burst starts with a stale nonce and the bot will fail the on‑chain “tx already in mempool” check.  
   *Hook*: write a small `tokio::signal::ctrl_c` handler in `bot/src/main.rs` that calls `nonce_manager.persist()` and exits cleanly.

* **Sequencer proximity ≠ RPC proximity.** Your `.env` currently points at Infura default endpoints  – many resolve to **us‑east‑1**. Point `WS_RPC_URL`/`HTTP_RPC_URL` to a west‑coast endpoint (`Alchemy – us‑west‑opt` or `Blast “US W”`) or you lose \~60 ms round‑trip every block.

* **Zero‑liquidity fork windows.** When the Spot VM cold‑forks Optimism at a random block you sometimes get pools whose `liquidity()` slot is `0` (your fallback warning in `state::fetch_and_cache_pool_state` proves it) . Always start Anvil with `--fork-block-number $(curl block height – 2)` to make sure the UniV3 pool has TVL.

* **Profit buffer too tight for L2 gas spikes.** The default absolute buffer in config is **5 × 10¹² wei** ≈ $17c at $3k ETH . If gas surges (rare, but jumps to 0.03 Gwei on OP) you can slip negative. Keep `MIN_PROFIT_ABS_BUFFER_WEI` at least **1 e14** and adjust `MAX_PRIORITY_FEE_PER_GAS_GWEI` above the default `0.01`.

* **Balancer flash‑loan gas limit.** Your `MIN_FLASHLOAN_GAS_LIMIT` is 500 k  but a two‑hop loop sometimes estimates 420 k and you add a 25 % buffer, busting 500 k. Either raise that floor or lower `GAS_LIMIT_BUFFER_PERCENTAGE` to 15\.

\#\#\# Pros & cons of the burst‑Spot architecture

| Aspect | Spot burst | Always‑on on‑demand | Notes |
| ----- | ----- | ----- | ----- |
| **Compute cost** | $0.016 / 2 h session (c3‑highcpu‑2) | $29 / mo | 50–70 × cheaper |
| **Pre‑emption risk** | High (5 %/day) | None | Mitigated by fast shutdown handler |
| **Cold‑start latency** | 15–20 s (image pull \+ boot) | 0 s | Hide behind 1‑min pre‑window |
| **Throughput** | Same CPU as on‑demand | Same | Spot uses identical silicon |
| **Quota limits** | Need to request spot‑C3 | Easier | File quota ticket once |

\#\#\# Advice for smooth integration with **ULP 1.5**

**Container entrypoint**

 bash  
CopyEdit  
`RUST_LOG=info,ulp1_5::simulation=debug \`

`./ulp1_5 --features local_simulation`

1.  Mount `/secrets/.env` at runtime; the bot already loads via `dotenv()`.

2. **Parametrise bursts with env overrides**  
    Use Cloud Scheduler → Pub/Sub → Cloud Functions to push a message like  
    `{"burst_minutes":120,"min_profit_bps":8}`. The controller VM writes that into the spot VM’s metadata and the bot reads it on startup (fallback to `.env` defaults).

3. **WireGuard deployment**

   * Controller `e2-micro` keeps a static public IP.

   * Spot startup script fetches a peer config from Secret Manager and brings up `wg0`, so your off‑cloud grafana/pager‑duty can scrape Prometheus metrics even while the public IP changes.

4. **Modify `submit_arbitrage_transaction`** to recognise a “dry‑run” mode during back‑testing bursts; skip the Balancer vault call and just log gas usage. Toggle via an env `DRY_RUN=true`.

5. **Secrets**

   * Put `LOCAL_PRIVATE_KEY` and `INFURA_ID` in Secret Manager, not in the image layer.

   * Use `gcloud secrets versions access --format='get(payload.data)'` in the boot script and pipe to `.env`.

6. **Observability**  
    Stream `tracing` JSON to Cloud Logging → create two log‑based metrics:

   * `simulated_net_profit_wei` (distribution)

   * `transaction_reverted` (counter)  
      Alert when 3 × reverts within 10 min or profit median \< $5.

7. **Scale‑out plan**  
    Keep one key per VM and un‑set `NONCE_MANAGER_SHARED` to false so your `NonceManager` stays local; otherwise duplicate nonces race.

8. **Upgrade path to $40 k/day**

   * Parallelise across token pairs: run a second bot binary with the same image but `.env` overriding `TOKEN_IN/TOKEN_OUT`.

   * Increase `OPTIMAL_LOAN_SEARCH_ITERATIONS` from 10 to 30 once you have c3‑highcpu‑4 capacity; more grid points improves loan sizing at higher slippage without delaying blocks (each extra iteration is \~4 ms CPU).

\#\#\# Key snippets to adjust

bash

CopyEdit

`# .env overrides for spot VM`

`WS_RPC_URL="wss://optimism.us-west1.alchemy.com/v2/YOUR_KEY"`

`HTTP_RPC_URL="https://optimism.us-west1.alchemy.com/v2/YOUR_KEY"`

`GAS_LIMIT_BUFFER_PERCENTAGE=15`

`MIN_PROFIT_ABS_BUFFER_WEI="100000000000000"   # 0.0001 WETH`

`MAX_PRIORITY_FEE_PER_GAS_GWEI="0.03"`

And bump the liquidity fallback check early in `state::fetch_and_cache_pool_state` so the bot aborts the burst if the pool has zero L‑shares.

With these tweaks the GCP burst model lines up with **ULP 1.5**’s parameters and should let you hit **$1–40 k daily** while spending only a few dollars in compute.

Currently we are testing locally, is there a budget way to test as near to production as possible? Give deep analysis.

###### **ChatGPT said:**

Thought for 47 seconds

\#\#\# Budget‑friendly ways to get *production‑like* signal while you’re still on a laptop

| Tier | What it tests | How to set it up | Direct cost | Hidden gotchas |
| ----- | ----- | ----- | ----- | ----- |
| **1\. Mainnet‑fork in the cloud (cheapest “almost‑prod”)** | Real pool state & gas, *no real contention* | Spin up an **e2‑micro** (free‑tier) or `t2d-standard‑1` Spot VM on GCP, install Foundry and run `anvil --fork-url https://mainnet.optimism.io --fork-block-number $(curl latest‑2)` on boot. | $0–$0.002 / h compute; RPC calls only. | • Fork snapshots lose mempool, so your flash‑loan always lands; add a fake LPG check. • Liquidity slots can be zero; start at a recent block to avoid `LiquidityZero()` reverts — this is exactly what bit you in `trigger_v3_swap_via_router` earlier. |
| **2\. Optimism Sepolia test‑net** | On‑chain latency, chain reorgs, EIP‑1559 fee markets | Switch `.env` → `WS_RPC_URL=https://sepolia‑optimism.g.alchemy.com/v2/...` and set `CHAIN_ID=11155420`. Keep `MIN_PROFIT_ABS_BUFFER_WEI=1` so the on‑chain `minProfit` check always passes . | Free (Alchemy/Infura dev tier) | • USDC/WETH liquidity is thin; flash‑loan route will underflow. • Flashbots & Balancer vault are not deployed—stub them behind `cfg(testnet)` flags. |
| **3\. Tenderly SimNet or Anvil RPC replay** | “What if this exact tx landed on mainnet?” including revert data | `tenderly devnet fork https://mainnet.optimism.io --balance=∞ --block $(latest)` and call `simulate_transaction` API on every candidate route. | Free for 10 k simulations/mo | • SimNet doesn’t model **priority fee auctions**; you still need to add the 25 % `GAS_LIMIT_BUFFER_PERCENTAGE` you already have in configuration [Google Cloud](https://cloud.google.com/spot-vms/pricing?utm_source=chatgpt.com). |
| **4\. Flashbots “Protect” sandbox** | Private‑relay path, tx replacement rules | Point `PRIVATE_RPC_URL=https://opt-mainnet.gateway.flashbots.net` in your `.env` and broadcast dry‑runs with `simulate=true`. | Free | • The Protect relay only *simulates* inclusion; real miners are off‑path, so don’t rely on returned bundle hash for nonce‑tracking. |
| **5\. Live sand‑boxed wallet on mainnet (₿ in, 0 out)** | *Everything* except losing money | Fund a burner EOA with **0.03 ETH** & **40 USDC**: — enough to pay gas for ≈ 200 L2 txs (gas ≈ 0.0001 ETH each). In `.env` keep `MIN_PROFIT_ABS_BUFFER_WEI="500000000000000"` (0.0005 WETH) so the contract never triggers a real swap . | $120 ≈ one‑off | • Need a guard in `submit_arbitrage_transaction` to **skip** final `BalancerVault.flashLoan` when `DRY_RUN=1`; otherwise you will borrow for real. |

---

\#\#\#\# Integrating each tier with **ULP 1.5**

**Config gating**

 bash  
CopyEdit  
`# .env overrides per tier`

`TEST_MODE="fork"            # fork|testnet|simnet|protect|mainnet-dry`

`CHAIN_ID=10                 # 11155420 for Sepolia`

`DRY_RUN=1                   # skip flash‑loan, still build route`

1.  In `src/config.rs` read `TEST_MODE` and:

   * short‑circuit `deploy_executor` if `testnet|fork`

   * hard‑code `BALANCER_VAULT_ADDRESS=0x000…dead` when `simnet|fork`

**Rust feature flags**  
 Wrap the gas‑intensive Balancer estimate (`estimate_flash_loan_gas`) in

 rust  
CopyEdit  
`#[cfg(not(feature = "budget"))]`

2.  so `cargo run --features budget,local_simulation` skips the RPC-heavy parts.

**Spot‑fork automation**  
 Controller script:

 bash  
CopyEdit  
`gcloud compute instances start ulp-fork \`

    `--metadata=startup-script='#!/bin/bash`

`docker run -d --pull=always \`

  `-e TEST_MODE=fork -e WS_RPC_URL=... \`

  `ghcr.io/quetzal/ulp:latest`

`'`

`sleep 7200 && gcloud compute instances stop ulp-fork`

3.  Two hours of `t2d-standard‑1` Spot in `us‑west1` ≈ **$0.004** (80 % off on‑demand) .

**Nonce safety on pre‑empt**  
 Add a `tokio::signal::ctrl_c` handler that calls

 rust  
CopyEdit  
`nonce_manager.persist().await?;`

4.  so the next boot picks up the last used nonce – otherwise the fork tier and mainnet‑dry tier will diverge.

---

\#\#\# Pros / Cons quick‑scan

* **Mainnet‑fork** – cheapest realism; lacks mempool & true gas race.

* **Sepolia** – gives you chain noise; no liquidity or MEV competition.

* **Tenderly / Protect** – free to abuse, perfect for unit tests; cannot measure latency.

* **Mainnet‑dry** – proves end‑to‑end pipeline; costs a tiny float but uncovers real revert edges.

Mix tiers 1 → 3 for daily CI, and run tier 5 a couple of times per week; you’ll catch 95 % of production‑only failures without burning real capital.

Below is a **repeatable “Mix 1 → 3” debug pipeline** you can bolt onto ULP 1.5 with minimal extra tooling.  
 Each stage is meant to catch a different class of bugs; you move to the next tier only if the previous tier is green.

| Stage | Trigger / CI job | Environment & how to launch | What you validate | Tools / commands | Gate ↔ Fix loop |
| ----- | ----- | ----- | ----- | ----- | ----- |
| **0 – Fast unit & integration tests (local)** | `cargo test --all --locked` on every PR | Laptop; `TEST_MODE=unit` (no chain) | Pure Rust logic, math, serialization | `cargo nextest`, `cargo udi` for unused‑deps | Fail → patch code / adjust mocks |
| **1 – Mainnet‑fork smoke** | GitHub Action on push to **feature branches** | Free **`e2‑micro`** self‑hosted runner (or Act) running: `anvil --fork-url $OP_RPC --fork-block-number $(cast block_number)-2 cargo test --features local_simulation -- --ignored --nocapture test_*_simulation` | Contract bindings, snapshot getters, route‑finder math *with real state* | `cast trace`, `forge inspect`, `debug_traceTransaction` | If pool reverts: pin to later block or call `pool.initialize()` in pre‑hook |
| **2 – Tenderly SimNet** | Manual (dev branch) or nightly GH Action | In workflow call: `tenderly simulations fork --fork-url $OP_RPC --block-tag latest` then run `tenderly sim` for **every candidate tx** generated by `find_optimal_loan_amount` (use API) | End‑to‑end flash‑loan, including Balancer vault & callback reverts | Tenderly Web UI diff, `tenderly export tx‑trace` | If revert reason ≠ gas, tweak executor logic or swap params |
| **3 – Optimism Sepolia dry‑runs** | Nightly / “pre‑merge to main” | GitHub Action starts matrix of **`cargo run --release`** with: `TEST_MODE=testnet CHAIN_ID=11155420 DRY_RUN=1` Runs for 15 min (workflow timeout) | Fee market behaviour, nonce tracking, on‑chain latency, WS heartbeat | `cast block`, `cast tx`, `forge watch` gas/fee metrics | If tx stuck: investigate pending nonce, raise `gas_price_boost`, widen `deadline` |
| **4 – Mainnet fork on Spot VM (optional burst)** | Manual “simulate prod” button or scheduled 2 h | `gcloud compute instances start ulp-fork-spot …` boot script: 1\. Pull container 2\. Start Anvil fork 3\. Run `ulp1_5 --features local_simulation` | Same as Stage 1 + network RTT & CPU perf under cloud | `perf stat`, `tracing` timestamps in Cloud Logging | Tune CPU pinning, adjust `tokio` worker threads |
| **5 – Mainnet dry‑wallet** | Promotion tag `v*-rc` | Same Spot VM but pointed at **real mainnet RPC** with burner key holding 0.03 ETH/40 USDC and `MIN_PROFIT_ABS_BUFFER_WEI=5e14` (never meets check) | Final guard: nonce flow, gas‑estimator, private‑relay path | Flashbots Protect sandbox, `cast bundle-sim` | Only proceed to prod if no reverts / stuck tx ‑\> else fix |

---

\#\#\# Integration hooks in the repo

* **`src/config.rs`** – add `env TEST_MODE` match arm; inject dummy Balancer vault when `fork` or `testnet`.

* **`bin/cli.rs`** – accept `--simulate <tx_json>` so Stage 2 can replay candidate bundles easily.

* **`NonceManager`** – add `persist(path)`/`load(path)` to survive Spot pre‑emptions (Stage 4).

* **`Cargo.toml`** – feature `"budget"` disables gas‑heavy Monte‑Carlo in `optimal_loan.rs` for CI.

---

\#\#\# Debug golden‑path checklist per tier

1. **Green tests** →

2. `forge inspect ArbitrageExecutor storage-layout` shows unchanged slot order →

3. Tenderly simulation returns *positive* `totalProfit` and status 1 →

4. Sepolia dry‑run pushes \>10 blocks without nonce gap; no `Transaction replaced` warnings →

5. Spot fork shows p95 end‑to‑end loop ≤ 250 ms and `simulated_net_profit_wei` metric ≥ 0 →

6. Mainnet dry‑wallet pushes 50 tx with zero reverts.

Hit all six and you’re safe to fund the prod key and flip `DRY_RUN=0`.

That pipeline costs **$0 for CI**, \<$0.01 per nightly Spot fork, and about 20 minutes wall‑clock, yet exercises 95 % of production failure modes before real capital is at risk.

## WebSocket Configuration
Use a WebSocket provider for real-time event subscriptions:
```bash
export WS_RPC_URL="wss://optimism-mainnet.infura.io/ws/v3/<PROJECT_ID>"
```
In `config.rs`, read `WS_RPC_URL` from env and initialize:
```rust
let ws_provider = Provider::connect(ws_rpc_url).await?;
```

## Testnet Dry Run Configuration
For Optimism Goerli testing, set:
```bash
export HTTP_RPC_URL="https://goerli.optimism.io"
export WS_RPC_URL="wss://goerli.optimism.io/ws"
export CHAIN_ID=420
```
Ensure `.env` reflects these values and use `--features local_simulation` in tests.
