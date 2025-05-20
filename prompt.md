# ULP 1.5 Arbitrage Bot - Project Context Transfer

## 1. Purpose of this Prompt

This prompt provides the necessary context and instructions to continue development on the ULP 1.5 Arbitrage Bot project. You are taking over from a previous session. Your primary goal is to understand the project's current state, goals, code structure, and established development workflows (`z1.1` and `j9` protocols) based on the information herein and the accompanying `all_files_combined.txt` file.

**Prioritize the instructions and definitions within *this* `prompt.md` file.** The `all_files_combined.txt` file contains the project's code history, previous notes, and older protocol definitions, which should be used as reference and context, but superseded by this document where definitions conflict (especially for `z1.1`).

## 2. Project Overview

*   **Goal:** Develop a high-performance arbitrage bot (ULP 1.5) capable of executing flash loan-based arbitrage opportunities across Layer 2 DEXs (initially focusing on Uniswap V3 and Velodrome V2 / Aerodrome style pools on Optimism/Base).
*   **Core Strategy:** Utilize Balancer V2 flash loans as the capital source and a custom, gas-optimized Huff contract (`ArbitrageExecutor.huff`) for atomic execution of the arbitrage (swap A -> swap B -> repay loan + profit check).
*   **Key Technologies:** Rust, Tokio, ethers-rs, Huff, Balancer V2, Uniswap V3, Velodrome V2 / Aerodrome.

## 3. Input Files Provided

You will receive the following *after* this prompt:

1.  `all_files_combined.txt`: Contains the complete source code for all relevant project files (`.rs`, `.huff`, `Cargo.toml`, `.env`, `README.md`, etc.), concatenated together. It also contains historical notes, previous prompt definitions, and status updates. **Use this primarily for code reference and historical context.**
2.  `prompt.md` (This file): Contains the **current** instructions, protocol definitions, status, and goals. **This file takes precedence over definitions found in `all_files_combined.txt`.**

**Action:** Process both this `prompt.md` and the contents of `all_files_combined.txt` to build your understanding of the project.

## 4. Development Workflow Protocols

All development **must** follow one of the two protocols defined below:

---

### 4.1. `z1.1` Protocol (Revised: Multi-File Task Output)

*   **Purpose:** Complete **one full task** at a time. A task might involve creating a new feature, refactoring a module, updating documentation, etc.
*   **Behavior:**
    *   Focuses on completing **one discrete development task** per interaction cycle.
    *   If the user prompt contains multiple independent tasks, address only **one task** before stopping.
    *   A single task **may require modifications to multiple files**.
*   **Output Requirements:**
    *   For each completed task, output the **complete and functional contents** of **every file modified** for that task.
    *   **Maximum 10 files** per response. If a task modifies more than 10 files, list the first 10 and note that others were modified.
    *   **No partials or code snippets.** Do not use `...imports...`, `...code...`, etc.
    *   `// TODO` comments are allowed only if non-critical and planned for a future step.
    *   Reflect awareness of breaking changes, recent updates, and project direction.
*   **After Each Task (Outputting 1 to 10 Files):**
    *   Stop and provide:
        *   A **brief summary** of the overall task completed.
        *   An **estimated percent complete** for the entire project.
    *   Wait for the next user instruction.
*   **Trigger:** User types **`z1.1 go`**.

---

### 4.2. `j9` Protocol (Error Correction)

*   **Purpose:** Fix compilation errors reported by `cargo check`, `cargo build`, or `cargo test`.
*   **Trigger:** User provides the complete compiler error output and the command **`j9 go`**.
*   **Action:** Analyze errors, determine fixes for **all** reported errors in the batch.
*   **Output:** Return the **complete contents** of **every file** modified (up to 10 files per response) to fix the errors.
*   **Per-File Summary:** *After* the code block for *each* modified file, stop and provide:
    *   `File Updated: [path/to/filename.ext]`    
    *   `Task Summary: [Brief description of fixes applied *within that specific file* for the current error batch.]`
*   **Overall Batch Summary:** After all modified files and their summaries, provide:
    *   `Estimated Percent Complete (Current Batch): [Percentage for fixing the current set of errors.]`
*   **Wait:** Wait for the user to run the check/build/test again or provide further instructions.

---

### 4.3. post `j9/z1.1` Protocol (logging)

*   **Purpose:** Append to a log for AI and human review history
*   **Trigger:** User provides the complete compiler error output and the command **`j9 go`** or **`z1.1 go`**.
*   **Action:** Analyze errors or tasks and append them for AI and humans to track project history. Keep the appended items concentrated (1-2 lines), add a date and time along with a unique log entry id. Make an exception for this activity, just give me the most recent output for the log entry and I will copy it into the LOG.md file.

---

## 5. Current Project Status (as of transfer)

*   **Core Logic:** Implemented for state management (`state.rs`), event handling (`event_handler.rs`), pathfinding (`path_optimizer.rs`), simulation (`simulation.rs`), and transaction submission/monitoring (`transaction.rs`). Supports UniV3 and Velo/Aero style pools.
*   **Configuration:** Handled via `.env` and `config.rs`. Includes network, keys, addresses, gas settings, profitability buffers, and health checks.
*   **Huff Executor:** `ArbitrageExecutor.huff` implemented with core swap logic, profit check, and salt nonce guard. Successfully compiled recently after fixing opcode/label issues.
*   **Testing:**
    *   `README.md` updated with detailed Anvil testing procedures.
    *   Local simulation framework (`local_simulator.rs`) exists.
    *   Integration test file (`tests/integration_test.rs`) created with tests for setup, basic swap triggers, and a *sequential* simulation of the full arbitrage cycle. Placeholder tests for direct Huff contract verification exist.
*   **Compilation:** `cargo check` passes cleanly (ignoring an allowed `unexpected_cfgs` warning). `huffc` compiles the Huff contract successfully.
*   **Recent Activity:** Primarily focused on fixing Huff compilation errors, implementing the sequential integration test, and refining configuration/health checks. Next WebSocket‐based event testing?
*   **Estimated Overall Completion:** ~89% (Core implementation complete, basic sequential testing structure in place, needs full test execution, hardening, and deployment prep).

## 6. Key Files Overview

*   `bot/src/main.rs`: Main entry point, event loop, provider setup.
*   `bot/src/config.rs`: Loads configuration from `.env`.
*   `bot/src/state.rs`: Defines `AppState`, `PoolState`, `PoolSnapshot`, `DexType`. Handles state fetching/caching.
*   `bot/src/event_handler.rs`: Processes incoming block/log events, updates state, triggers arbitrage checks.
*   `bot/src/path_optimizer.rs`: Finds potential arbitrage routes based on cached snapshots.
*   `bot/src/simulation.rs`: Simulates route profitability off-chain, finds optimal loan amount.
*   `bot/src/transaction.rs`: Constructs, signs, submits, and monitors arbitrage transactions. Includes `NonceManager`.
*   `bot/src/local_simulator.rs`: Framework for interacting with local Anvil fork (used by tests).
*   `bot/tests/integration_test.rs`: Integration tests using Anvil.
*   `contracts/ArbitrageExecutor.huff`: Huff implementation of the flash loan executor.
*   `abis/`: Directory containing JSON ABI files for bindings.
*   `Cargo.toml`: Project dependencies and features.
*   `.env`: User-specific configuration (keys, RPC URLs, addresses).
*   `README.md`: Project documentation, setup, testing instructions.

## 7. Next Steps / Goals

*   **Immediate:** Execute the integration tests defined in `tests/integration_test.rs` against Anvil to verify runtime behavior. Debug any failures. (User action required).
*   **Near-Term:**
    *   Refine error handling and add more specific logging across modules.
    *   Implement more robust nonce error recovery/resynchronization if needed.
    *   Consider adding basic external alerting integration (e.g., simple webhook on success/failure/panic).
*   **Lower Priority / Future:**
    *   Implement accurate UniV3 dynamic loan sizing based on tick liquidity.
    *   Add support for more DEXs (e.g., Curve, SushiSwap, other L2 DEXs).
    *   Optimize gas usage further in Rust and Huff code.
    *   Develop deployment scripts/configuration (e.g., Docker, systemd).
    *   Implement advanced monitoring and alerting dashboard.

## 8. Initial Instruction

1.  Acknowledge that you have processed this `prompt.md` and the `all_files_combined.txt`.
2.  Confirm your understanding of the project goal, current status, and the `z1.1` / `j9` protocols (especially the revised multi-file `z1.1` output).
3.  **Do not start coding yet.** Wait for the user to provide the first `z1.1 go` or `j9 go` command.