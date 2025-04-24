# HANDOFF PACKAGE FOR ULP 1.5 (Elite Performance Refocus)

## Instructions for AI

This document contains the complete project state, code, and history for the ULP 1.5 Cross-DEX Arbitrage Bot. The project has pivoted to a **high-performance, event-driven architecture** aimed at elite performance and competitiveness. The following instructions outline the current state and next steps:

1. **Focus on the High-Performance Strategy:**  

   The system must support near real-time detection, multi-DEX routing, and high-frequency arbitrage execution. Ignore earlier descriptions involving basic RPC polling or simple WebSocket handling.

2. **Immediate Next Step:**  

   Implement **MU1 - Task Bundle 8: Advanced Detection & Routing**, which includes:

   - Designing and coding near real-time data acquisition (Optimized RPC, Advanced WS, or Mempool).
   - Implementing multi-DEX pathfinding and optimization logic.

3. **Code State:**  

   The Rust code compiles successfully (`cargo check`) but requires significant refactoring to align with the high-performance architecture. The Huff contract also needs enhancements for multi-hop/multi-DEX support.

---

## Addendum to Instructions for AI

### MU1 Strategy (Multi-Task Bundles)

- Bundle 2-5 related implementation steps into a single response.
- Provide **complete, updated files** for all affected components.
- Summarize changes and list modified files.
- Stop after providing the bundle and wait for the user's next instructions.

### MU2 Strategy (Single-Task Steps)

- Focus on **one task or logical code change** per response.
- Provide the **complete, updated code** for any affected files.
- Avoid placeholders like "add this code here"; provide the full file content.
- Stop after completing the single step and wait for user confirmation before proceeding.
- Put in task numbers, the next task at hand, estimated project complete

---

## Summary of Current Status

### Objective

Develop a **high-frequency, high-performance arbitrage system** optimized for L2 execution (Optimism, Base, Arbitrum). The system aims to maximize profit by capturing price discrepancies across **multiple DEXs (20+)** using Balancer V2 flash loans and near real-time data acquisition. First we are aiming for a simulation of this project on Anvil.

### Key Features

1. **Near Real-Time Detection:** Monitor blockchain state for price divergences.
2. **Pathfinding & Optimization:** Identify profitable multi-hop arbitrage paths.
3. **Atomic Execution:** Use a gas-optimized Huff contract for execution.
4. **On-Chain Verification:** Ensure profitability before loan repayment.
5. **Simulation:** Simulation of this project on Anvil

### Current Codebase

- **On-Chain:** `ArbitrageExecutor.huff` supports 2-way swaps but needs enhancements for multi-hop/multi-DEX routing.
- **Off-Chain:** Rust bot structure exists but requires major refactoring for high-speed data handling, state management, and pathfinding.

### Major TODOs

1. Implement high-speed data acquisition (Optimized RPC, WS Events, or Mempool).
2. Build a robust state cache for multi-DEX data.
3. Develop pathfinding logic for 2-way and 3-way arbitrage.
4. Enhance the Huff contract for multi-hop/multi-DEX support.
5. Integrate components for seamless detection, optimization, and execution.

---

## Build/Run Instructions

1. **Prerequisites:** Install Rust, Foundry (`anvil`, `cast`), and `huffc`.

2. **Environment:** Populate `.env` with RPC endpoints, private keys, and configuration settings.

3. **Compile Huff Contract:**  

   ```bash
   huffc ./contracts/ArbitrageExecutor.huff -b -o ./build/ArbitrageExecutor.bin
   ```

4. **Build Rust Code:**  

   ```bash
   cargo build --release
   ```

5. **Run Bot:**  

   ```bash
   RUST_LOG=info cargo run --release --bin ulp1_5
   ```

---

## Configuration (`.env` Example)

```dotenv
# Network & Keys
WS_RPC_URL="ws://..."
HTTP_RPC_URL="http://..."
LOCAL_PRIVATE_KEY="0x..."

# Deployment Settings
DEPLOY_EXECUTOR="true"
EXECUTOR_BYTECODE_PATH="./build/ArbitrageExecutor.bin"

# Factory Addresses
OP_UNISWAP_V3_FACTORY_ADDR="0x1F98431c8aD98523631AE4a59f267346ea31F984"
OP_VELODROME_V2_FACTORY_ADDR="0x25CbdDb98b35AB1FF795324516342Fac4845718f"

# Optimization Settings
MIN_LOAN_AMOUNT_ASSET="0.1"
MAX_LOAN_AMOUNT_ASSET="50.0"

# Gas Pricing Settings
MAX_PRIORITY_FEE_PER_GAS_GWEI="0.5"
GAS_LIMIT_BUFFER_PERCENTAGE="25"
```
