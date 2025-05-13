# PROJECT_DIRECTION_LOG.MD - ULP 1.5 Arbitrage Bot

**Last Updated:** 2025-05-13

## Current Overarching Goal:
Transition from local Anvil-based testing to production-readiness by validating the bot against live network conditions in a safe, iterative manner. The ultimate aim is a timely deployment to capture arbitrage opportunities.

## Recent Key Decisions & Rationale:

*   **Decision (2025-05-13):** Prioritize testing on live networks (Testnet then Mainnet Dry Run) over further complex local WebSocket emulation.
    *   **Rationale:** Local Anvil testing has provided good component-level validation. However, to de-risk production deployment and understand real-world performance (network latency, RPC reliability, WS stream behavior, gas dynamics), testing against live infrastructure is crucial. This aligns with expert advice in `ULP-1.5-Networking.md` and acknowledges that a month of local testing has built a sufficient foundation to move forward. The value of a complex local WS test is deemed lower than direct live network testing at this stage.

## Current Recommended Action Plan:

**Phase 1: Solidify Core Logic & Final Local Cleanup (Est: Imminent - 1-2 iterations)**

1.  **Task:** Fix all current compilation errors.
    *   **Status:** Pending. Last `cargo test` attempt failed with 3 errors and 1 warning.
    *   **Next Action:** User to provide `j9 go` for AI to attempt fixes. AI to ensure meticulous application of fixes (visibility, ABI names, unused variables).
2.  **Task:** User to verify successful compilation and passing local tests.
    *   **Command:** `cargo test --features local_simulation -- --ignored --nocapture`
    *   **Status:** Pending completion of 1.1.

**Phase 2: Testnet Shakedown (Est: 1-2 days of user effort after Phase 1)**

*   **Objective:** Validate basic connectivity, transaction mechanics (gas, nonce), and event stream processing against a live, public (but consequence-free) network like Optimism Sepolia.
*   **Prerequisites:** Phase 1 complete.
*   **Key Activities:**
    1.  **Configuration:**
        *   Update `.env` for Sepolia (RPCs, Chain ID, funded test wallet key).
        *   Adapt contract addresses (`ArbitrageExecutor`, DEXs, Tokens, Balancer Vault) for Sepolia. This may involve:
            *   Deploying `ArbitrageExecutor.huff` to Sepolia.
            *   Finding Sepolia equivalents for DEX/Token contracts.
            *   Using feature flags (e.g., `#[cfg(not(feature = "sepolia_testing"))]`) or conditional logic to handle missing mainnet-specific contracts, focusing on testing the bot's framework rather than specific arb logic on Sepolia.
    2.  **Execution:** Run the bot binary (`cargo run --bin ulp1_5`) against Sepolia.
    3.  **Monitoring & Validation:**
        *   Successful WS/HTTP connections.
        *   Correct block and event processing (if activity allows).
        *   `NonceManager` and gas fetching behavior.
        *   Overall stability and error handling.

**Phase 3: Mainnet Dry Run - Critical Validation (Est: Ongoing, iterative after Phase 2)**

*   **Objective:** Test the entire system end-to-end against live Optimism mainnet conditions without risking flash loan capital.
*   **Prerequisites:** Successful Testnet Shakedown (Phase 2).
*   **Key Activities:**
    1.  **Implement `DRY_RUN` Mode:**
        *   Add `DRY_RUN=true/false` to `config.rs` (loaded from `.env`).
        *   Modify `transaction.rs::submit_arbitrage_transaction` to:
            *   Perform all steps (gas estimation, encoding, etc.).
            *   If `config.dry_run` is true, log the transaction that *would* have been sent but **DO NOT** actually send it (i.e., skip `client.send_raw_transaction()` and private relay calls).
            *   Ensure `BalancerVault.flashLoan()` is **NOT** called.
        *   Consider using simplified gas estimation in dry run mode if full estimation is too slow/costly for this testing phase.
    2.  **Configuration for Mainnet Dry Run:**
        *   Use production mainnet Optimism RPCs.
        *   Use a **burner mainnet wallet** (small ETH balance for gas of *potential minor transactions* like contract deployment if needed, but not for flash loans).
        *   Use correct mainnet contract addresses for all components (DEXs, Tokens, Balancer Vault).
        *   Deploy `ArbitrageExecutor.huff` to mainnet using the burner wallet and set `ARBITRAGE_EXECUTOR_ADDRESS` in `.env`.
        *   Set `DRY_RUN=true` in `.env`.
        *   Consider an initially high `MIN_PROFIT_ABS_BUFFER_WEI` as an added safety layer for the (bypassed) on-chain profit check.
    3.  **Execution:** Run the bot (`DRY_RUN=true cargo run --bin ulp1_5`) against mainnet.
    4.  **Extensive Monitoring & Analysis (Primary focus):**
        *   WS connection stability and event stream reliability from production nodes.
        *   Accuracy of event decoding and `PoolSnapshot` updates from live mainnet data.
        *   Performance of `find_top_routes` and `find_optimal_loan_amount` (simulation) with real data and RPCs.
        *   Logging for errors, warnings, performance bottlenecks.
        *   Verify no actual flash loan attempts are made.
    5.  **Iteration:** Continuously run, monitor, identify bugs/bottlenecks, refine configurations (gas, profit buffers based on *simulated* outcomes from live data), and improve.

**Integration with `ULP-1.5-Networking.md`:**
The infrastructure and operational guidance in `ULP-1.5-Networking.md` (GCP setup, containerization, secret management, advanced monitoring, etc.) become directly applicable and highly recommended during **Phase 3 (Mainnet Dry Run)** and essential for actual production deployment. The current plan focuses on getting the bot's software ready for such an environment.

## Next Immediate Action:
*   User to issue `j9 go` to address the outstanding compilation errors.