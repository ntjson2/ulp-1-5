# HANDOFF PACKAGE FOR ULP 1.5 (Elite Performance Refocus)

## Instructions for AI

This document contains the complete project state, code, and history for the ULP 1.5 Cross-DEX Arbitrage Bot. The project has pivoted to a **high-performance, event-driven architecture** aimed at elite performance and competitiveness. The following instructions outline the current state and next steps:

1.  **Focus on the High-Performance Strategy:**

    The system must support near real-time detection, multi-DEX routing, and high-frequency arbitrage execution. Ignore earlier descriptions involving basic RPC polling or simple WebSocket handling.

2.  **Immediate Next Step:**

    Implement **MU1 - Task Bundle 8: Advanced Detection & Routing**, which includes:

    *   Designing and coding near real-time data acquisition (Optimized RPC, Advanced WS, or Mempool).
    *   Implementing multi-DEX pathfinding and optimization logic.

3.  **Code State:**

    The Rust code compiles successfully (`cargo check`) but requires significant refactoring to align with the high-performance architecture. The Huff contract also needs enhancements for multi-hop/multi-DEX support.

---

## Addendum to Instructions for AI

### MU1 Strategy (Multi-Task Bundles) - *Deprecated*

*   Bundle 2-5 related implementation steps into a single response.
*   Provide **complete, updated files** for all affected components.
*   Summarize changes and list modified files.
*   Stop after providing the bundle and wait for the user's next instructions.

### MU2 Strategy (Single-Task Steps) - *Active*

*   Focus on **one task or logical code change** per response.
*   Provide the **complete, updated code** for **only one affected file** at a time.
*   If the file is a Rust (`.rs`) file, present it in a copyable code block.
*   Avoid placeholders like "add this code here"; provide the full file content.
*   **Stop after providing the single file update** and wait for user confirmation/guidance before proceeding to the next task or file.
*   Include the task number, the next specific action (e.g., "Update file X"), and the estimated project completion percentage.

---

## Summary of Current Status

### Objective

Develop a **high-frequency, high-performance arbitrage system** optimized for L2 execution (Optimism, Base, Arbitrum). The system aims to maximize profit by capturing price discrepancies across **multiple DEXs (20+)** using Balancer V2 flash loans and near real-time data acquisition. First we are aiming for a simulation of this project on Anvil.

### Key Features

1.  **Near Real-Time Detection:** Monitor blockchain state for price divergences.
2.  **Pathfinding & Optimization:** Identify profitable multi-hop arbitrage paths.
3.  **Atomic Execution:** Use a gas-optimized Huff contract for execution.
4.  **On-Chain Verification:** Ensure profitability before loan repayment.
5.  **Simulation:** Simulation of this project on Anvil

### Current Codebase

*   **On-Chain:** `ArbitrageExecutor.huff` supports 2-way swaps but needs enhancements for multi-hop/multi-DEX routing.
*   **Off-Chain:** Rust bot structure exists but requires major refactoring for high-speed data handling, state management, and pathfinding.

### Major TODOs

1.  Implement high-speed data acquisition (Optimized RPC, WS Events, or Mempool).
2.  Build a robust state cache for multi-DEX data.
3.  Develop pathfinding logic for 2-way and 3-way arbitrage.
4.  Enhance the Huff contract for multi-hop/multi-DEX support.
5.  Integrate components for seamless detection, optimization, and execution.

---

## Build/Run Instructions

1.  **Prerequisites:** Install Rust, Foundry (`anvil`, `cast`), and `huffc`.

2.  **Environment:** Populate `.env` with RPC endpoints, private keys, and configuration settings. Ensure WETH/USDC addresses and decimals for the target chain (e.g., Optimism) are included.

3.  **Compile Huff Contract:**

    ```bash
    huffc ./contracts/ArbitrageExecutor.huff -b -o ./build/ArbitrageExecutor.bin
    ```

4.  **Build Rust Code:**

    ```bash
    cargo build --release
    ```

5.  **Run Bot:**

    ```bash
    # Example: Set log level via environment variable
    # RUST_LOG=info,ulp1_5=debug
    cargo run --release --bin ulp1_5
    ```

---

## Configuration (`.env` Example - Optimism)

```dotenv
# Network & Keys (Use Anvil for local simulation)
WS_RPC_URL="ws://127.0.0.1:8545"
HTTP_RPC_URL="http://127.0.0.1:8545"
LOCAL_PRIVATE_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80" # Anvil default key 0

# Contract Addresses (Optimism Mainnet Addresses)
# Replace ARBITRAGE_EXECUTOR_ADDRESS if DEPLOY_EXECUTOR=false
ARBITRAGE_EXECUTOR_ADDRESS="" # Fill in if using existing deployment
UNISWAP_V3_FACTORY_ADDR="0x1F98431c8aD98523631AE4a59f267346ea31F984"
VELODROME_V2_FACTORY_ADDR="0x25CbdDb98b35AB1FF795324516342Fac4845718f"
WETH_ADDRESS="0x4200000000000000000000000000000000000006"
USDC_ADDRESS="0x7F5c764cBc14f9669B88837ca1490cCa17c31607" # Note: USDC Bridged (USDC.e) might be 0x7F5c7... or Native USDC 0x0b2C6... Check pools!
WETH_DECIMALS=18
USDC_DECIMALS=6 # Check decimals for the specific USDC used in pools
VELO_V2_ROUTER_ADDR="0x9c12939390052919aF3155f41Bf41543Ca30607P" # CHECK ACTUAL ADDRESS - Placeholder invalid
BALANCER_VAULT_ADDRESS="0xBA12222222228d8Ba445958a75a0704d566BF2C9" # Same on most chains
QUOTER_V2_ADDRESS="0xbC52C688c34A4F6180437B40593F1F9638C2571d" # CHECK ACTUAL ADDRESS - Placeholder likely invalid

# Deployment Settings
DEPLOY_EXECUTOR="true" # Set to false if ARBITRAGE_EXECUTOR_ADDRESS is filled
EXECUTOR_BYTECODE_PATH="./build/ArbitrageExecutor.bin"

# Optimization Settings
MIN_LOAN_AMOUNT_WETH="0.1"
MAX_LOAN_AMOUNT_WETH="50.0"
OPTIMAL_LOAN_SEARCH_ITERATIONS=10

# Gas Pricing Settings (Adjust for Optimism L2)
MAX_PRIORITY_FEE_PER_GAS_GWEI="0.01" # L2 priority fees are usually very low
GAS_LIMIT_BUFFER_PERCENTAGE="25"
MIN_FLASHLOAN_GAS_LIMIT=400000 # Adjust based on L2 execution costs