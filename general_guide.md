# General Guide
Okay, refocusing on the local testing setup for the **Cross-DEX 2-Way Arbitrage via Multi-Pair Scanner** strategy.

We left off after successfully setting up the basic Rust bot connection (Step 4). The next logical step is **implementing data polling to get pool states**. Since we're starting with direct contract calls, we need to:

1.  Identify the specific function(s) on the DEX contracts that provide the necessary price/liquidity data.
2.  Get the addresses of a few sample pools on your forked L2 (Optimism) for testing.
3.  Use `ethers-rs` to call these functions in a loop.

**Step 6 (Resumed): Data Polling - Direct Contract Calls**

**Step 6.1: Identify Target DEXs and Pool Data Functions**

*   **DEX 1: Uniswap V3:**
    *   **Function:** `slot0()` - Returns `sqrtPriceX96`, `tick`, and other state info. The `sqrtPriceX96` is key for calculating the current price.
    *   **ABI:** We'll need a minimal ABI containing just `slot0`.
*   **DEX 2 (Example L2 Native): Velodrome V2 (on Optimism)**
    *   **Function:** `getReserves()` - Returns `reserve0`, `reserve1`, and `blockTimestampLast`. These reserves are used in the CPMM formula (like UniV2) to get the price. (Note: Velodrome V2 pools are typically UniV2 forks).
    *   **ABI:** We'll need a minimal ABI containing `getReserves`, `token0`, `token1`.

**Step 6.2: Find Sample Pool Addresses (Optimism Fork)**

We need addresses for the *same pair* on both UniV3 and Velodrome V2 that exist on Optimism mainnet (and thus on your fork). Let's use **WETH / USDC**:

1.  **Find Optimism Uniswap V3 WETH/USDC Pool:**
    *   Go to the Uniswap Info site (info.uniswap.org).
    *   Select the Optimism network.
    *   Search for the WETH/USDC pair. There will likely be multiple pools with different fee tiers (0.05%, 0.3%, etc.).
    *   Pick one with decent liquidity, e.g., the **0.05% fee tier pool**. Click on it.
    *   Find its contract address on the page. As of late 2023/early 2024, a common one was `0x85149247691df622eac1a890620f5c43775697b2`. **Verify this address on a block explorer like Optimistic Etherscan.** Let's assume this is `UNI_V3_POOL_ADDR`.
2.  **Find Optimism Velodrome V2 WETH/USDC Pool:**
    *   Go to the Velodrome Finance app (app.velodrome.finance).
    *   Connect your wallet (read-only is fine) and ensure it's set to Optimism.
    *   Go to the "Pools" or "Liquidity" section.
    *   Find the WETH/USDC pool (likely the volatile `vAMM-WETH/USDC`).
    *   Click on it or find details to get its pool address. A common one was `0x79c912fef520be002c2b6e57ec4324e260f38e3a`. **Verify this on Optimistic Etherscan.** Let's assume this is `VELO_V2_POOL_ADDR`.

**Step 6.3: Create ABIs (Minimal JSON)**

Create two simple JSON ABI files in your `bot/` directory (or a new `bot/abis/` subdirectory):

*   `UniswapV3Pool.json`:
    ```json
    [
      {
        "inputs": [],
        "name": "slot0",
        "outputs": [
          { "internalType": "uint160", "name": "sqrtPriceX96", "type": "uint160" },
          { "internalType": "int24", "name": "tick", "type": "int24" },
          { "internalType": "uint16", "name": "observationIndex", "type": "uint16" },
          { "internalType": "uint16", "name": "observationCardinality", "type": "uint16" },
          { "internalType": "uint16", "name": "observationCardinalityNext", "type": "uint16" },
          { "internalType": "uint8", "name": "feeProtocol", "type": "uint8" },
          { "internalType": "bool", "name": "unlocked", "type": "bool" }
        ],
        "stateMutability": "view",
        "type": "function"
      }
    ]
    ```
*   `VelodromeV2Pool.json`:
    ```json
    [
      {
        "inputs": [],
        "name": "getReserves",
        "outputs": [
          { "internalType": "uint112", "name": "_reserve0", "type": "uint112" },
          { "internalType": "uint112", "name": "_reserve1", "type": "uint112" },
          { "internalType": "uint32", "name": "_blockTimestampLast", "type": "uint32" }
        ],
        "stateMutability": "view",
        "type": "function"
      },
      {
        "inputs": [],
        "name": "token0",
        "outputs": [{ "internalType": "address", "name": "", "type": "address" }],
        "stateMutability": "view",
        "type": "function"
      },
      {
        "inputs": [],
        "name": "token1",
        "outputs": [{ "internalType": "address", "name": "", "type": "address" }],
        "stateMutability": "view",
        "type": "function"
      }
    ]
    ```

**Step 6.4: Update `.env` with Pool Addresses**

Add the pool addresses you found to your `bot/.env` file:

```dotenv
# ... (keep existing variables)

# Example Pool Addresses (VERIFY THESE ON OPTIMISTIC ETHERSCAN)
UNI_V3_POOL_ADDR="0x85149247691df622eac1a890620f5c43775697b2"
VELO_V2_POOL_ADDR="0x79c912fef520be002c2b6e57ec4324e260f38e3a"
```

**Step 6.5: Implement Polling Logic in `bot/src/main.rs`**

Modify your `main` function to include a loop that calls the contract functions. We'll use `ethers-rs`'s contract abstraction.

```rust
use ethers::prelude::*;
use eyre::Result;
use std::{env, sync::Arc, time::Duration}; // Added time::Duration
use dotenv::dotenv;
use ethers::utils::format_units; // For displaying balance

// Define contract bindings using ABIs
abigen!(
    UniswapV3Pool, // Name we give the contract type
    "./abis/UniswapV3Pool.json", // Path to the ABI file
    event_derives(serde::Deserialize, serde::Serialize)
);

abigen!(
    VelodromeV2Pool,
    "./abis/VelodromeV2Pool.json",
    event_derives(serde::Deserialize, serde::Serialize)
);


#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok(); // Load .env file

    // Load environment variables
    let rpc_url = env::var("LOCAL_RPC_URL").expect("LOCAL_RPC_URL must be set");
    let private_key = env::var("LOCAL_PRIVATE_KEY").expect("LOCAL_PRIVATE_KEY must be set");
    let _contract_addr_str = env::var("ARBITRAGE_EXECUTOR_ADDRESS") // Keep for later, mark unused
        .expect("ARBITRAGE_EXECUTOR_ADDRESS must be set");
    let uni_v3_pool_addr_str = env::var("UNI_V3_POOL_ADDR").expect("UNI_V3_POOL_ADDR must be set");
    let velo_v2_pool_addr_str = env::var("VELO_V2_POOL_ADDR").expect("VELO_V2_POOL_ADDR must be set");

    println!("Connecting to Anvil node at: {}", rpc_url);

    // Setup provider (Arc allows sharing across tasks/threads)
    let provider = Provider::<Http>::try_from(rpc_url)?;
    let provider = Arc::new(provider);

    let chain_id = provider.get_chainid().await?.as_u64();
    println!("Connected to chain ID: {}", chain_id);

    // Setup wallet (we don't need the full client middleware for read-only calls)
    let wallet = private_key
        .parse::<LocalWallet>()?
        .with_chain_id(chain_id);
    let signer_address = wallet.address();
    println!("Using signer address: {:?}", signer_address);

    // Parse contract addresses
    let uni_v3_pool_address = uni_v3_pool_addr_str.parse::<Address>()?;
    let velo_v2_pool_address = velo_v2_pool_addr_str.parse::<Address>()?;
    let _arb_executor_address = _contract_addr_str.parse::<Address>()?; // Keep for later

    println!("Watching UniV3 Pool: {:?}", uni_v3_pool_address);
    println!("Watching VeloV2 Pool: {:?}", velo_v2_pool_address);

    // Create contract instances using the provider (read-only)
    let uni_v3_pool = UniswapV3Pool::new(uni_v3_pool_address, provider.clone());
    let velo_v2_pool = VelodromeV2Pool::new(velo_v2_pool_address, provider.clone());

    println!("\n--- Starting Polling Loop (Ctrl+C to stop) ---");

    // Polling loop
    loop {
        println!("--- Fetching Pool States ---");

        // Fetch UniV3 state
        match uni_v3_pool.slot_0().call().await {
            Ok(slot0_data) => {
                // slot0_data contains sqrtPriceX96, tick, etc.
                println!("UniV3 WETH/USDC:");
                println!("  SqrtPriceX96: {}", slot0_data.sqrt_price_x96);
                println!("  Tick: {}", slot0_data.tick);
                // TODO: Calculate actual price from sqrtPriceX96
            }
            Err(e) => {
                eprintln!("Error fetching UniV3 slot0: {}", e);
            }
        }

        // Fetch Velodrome state
        match velo_v2_pool.get_reserves().call().await {
            Ok(reserves) => {
                // reserves contains _reserve0, _reserve1, _blockTimestampLast
                 println!("Velo V2 WETH/USDC:");
                 println!("  Reserve0: {}", reserves.0); // Assuming token0 is WETH here - VERIFY
                 println!("  Reserve1: {}", reserves.1); // Assuming token1 is USDC here - VERIFY
                 // TODO: Calculate actual price from reserves (reserve1 / reserve0) - Need decimals!
            }
            Err(e) => {
                 eprintln!("Error fetching Velo V2 reserves: {}", e);
            }
        }

        // Wait before next poll
        tokio::time::sleep(Duration::from_secs(10)).await; // Poll every 10 seconds
    }

    // Note: Code below loop is unreachable in this example
    // Ok(())
}
```

**Step 6.6: Run the Bot**

1.  **Make sure Anvil fork is running.**
2.  **In the `ulp-1.5` root directory terminal, run:**
    ```bash
    cargo run --bin ulp1_5
    ```

**Expected Output:**

After compiling (which should be fast now), the bot will print the connection info and then enter a loop:

```
Connecting to Anvil node at: http://127.0.0.1:8545
Connected to chain ID: 10
Using signer address: 0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266
Watching UniV3 Pool: 0x85149247691df622eac1a890620f5c43775697b2
Watching VeloV2 Pool: 0x79c912fef520be002c2b6e57ec4324e260f38e3a

--- Starting Polling Loop (Ctrl+C to stop) ---
--- Fetching Pool States ---
UniV3 WETH/USDC:
  SqrtPriceX96: <large_number>
  Tick: <tick_number>
Velo V2 WETH/USDC:
  Reserve0: <reserve_number_token0>
  Reserve1: <reserve_number_token1>
--- Fetching Pool States ---
UniV3 WETH/USDC:
  SqrtPriceX96: <large_number>
  Tick: <tick_number>
Velo V2 WETH/USDC:
  Reserve0: <reserve_number_token0>
  Reserve1: <reserve_number_token1>
... (repeats every 10 seconds) ...
```

This confirms your bot can now poll the state of the relevant pools on the Anvil fork.

**Next Steps within the Bot:**

Good questions! Let's clarify those points:

1.  **Static Block on Anvil:**
    *   **The reason the prices, spread, and block number** (`134457754`) are not changing in your bot's output is because Anvil has created a *local copy* (a fork) of the Optimism blockchain *as it existed* at that specific block height. Time is essentially frozen at that block on your local Anvil instance unless *you* send new transactions *to Anvil* (e.g., deploying a contract, making a swap via your bot later). The bot is querying your local Anvil node, which only knows about the state at block `134457754`.

2.  **Spread Threshold (0.1%): Why Higher is Better (Initially):**
    *   You're right that in the *final execution*, a smaller spread might seem efficient. However, the **threshold (0.1%)** is the **minimum *gross* spread** the bot looks for *before* even considering an arbitrage attempt.
    *   **Why we need a minimum gross spread:** We need the initial price difference (gross spread) to be large enough to cover all the costs involved in executing the arbitrage:
        *   Swap Fee on DEX A (e.g., 0.05% on UniV3)
        *   Swap Fee on DEX B (e.g., 0.20% on Velo)
        *   Balancer Flash Loan Fee (e.g., 0.01%)
        *   Slippage Cost (price impact from your trade, e.g., 0.05%)
        *   Gas Cost (small, but non-zero)
    *   **Calculation:** In the example above, the costs add up to roughly 0.05% + 0.20% + 0.01% + 0.05% = 0.31% (excluding gas).
    *   **Conclusion:** A gross spread of only 0.0539% (as seen in your logs) is *much smaller* than the ~0.31% needed just to break even. Therefore, the bot correctly ignores it. We need the initial difference between the DEX prices (`spread_percentage`) to be *greater* than our `ARBITRAGE_THRESHOLD_PERCENTAGE` (which itself should be set high enough to likely cover all costs plus a desired profit margin) before we bother simulating and potentially executing. A higher initial spread means a higher chance of *net* profit after all costs.

3.  **Velodrome/Uniswap Routers and Anvil:**
    *   **You are correct, the *real* Velodrome and Uniswap router contracts deployed on Optimism mainnet are *not* looking at your local Anvil instance.** They don't even know it exists.
    *   **What the bot queries:** When your Rust bot calls `.slot0()` or `.getReserves()` on the *pool* contract addresses (`UNI_V3_POOL_ADDR`, `VELO_V2_POOL_ADDR`) via your Anvil RPC (`http://127.0.0.1:8545`), Anvil intercepts these calls. It uses its *forked state* (the copy of Optimism at block `134457754`) to determine what the return value *would have been* if you had called those functions on the real Optimism network at that specific block.
    *   **Routers in `.env`:** The `VELO_V2_ROUTER_ADDR` we added to `.env` is *not* being polled right now. It will be used *later* by the Rust bot when it needs to:
        *   **Simulate** a Velodrome swap (often helper libraries need the router address).
        *   **Prepare the actual transaction:** When sending the flash loan transaction, the `userData` encoded for the Huff contract will need to include the correct Velo router address so the Huff contract knows which contract to `call` for the Velodrome swap leg.
    *   **In essence:** We poll the *pools* for prices via Anvil's forked state. We use the *router* addresses later when constructing swap simulations or transactions, assuming those routers exist at those addresses in the forked state.


Okay, let's start with **Step 1: Run Live Test (Anvil) with Manual State Manipulation**.

This step requires interaction between the running bot and manual commands sent to the Anvil fork in a separate terminal. The goal is to artificially create a large price spread while the bot is running and see if it triggers the full execution path.

---

**Task 11: Prepare for State Manipulation Test**

**Objective:** Identify the target pool, the specific storage slot to modify, get its current value, and calculate a significantly different value to create an artificial price spread. We'll target the Uniswap V3 pool's `slot0`.

**Steps:**

1.  **Ensure Anvil is Running:**
    ```bash
    anvil --fork-url $OPTIMISM_RPC_URL --chain-id 10
    ```
2.  **Identify UniV3 Pool Address:** From your `.env`, this is `0x85149247691df622eaf1a8bd0cafd40bc45154a9`.
3.  **Target Storage Slot:** For Uniswap V3 pools, the core price information (`sqrtPriceX96`) is stored in **storage slot 0**.
4.  **Get Current `slot0` Value:** Use `cast` to read the current value of storage slot 0 for the UniV3 pool on your Anvil fork.
    ```bash
    cast storage $UNI_V3_POOL_ADDR 0 --rpc-url http://127.0.0.1:8545
    # Or using the address directly:
    # cast storage 0x85149247691df622eaf1a8bd0cafd40bc45154a9 0 --rpc-url http://127.0.0.1:8545
    ```
    *   Copy the full 32-byte hex value returned (it will look like `0x...`). Let's call this `CURRENT_SLOT0_VALUE`.
5.  **Interpret Current Price (Optional but helpful):** You can roughly interpret the price from this `sqrtPriceX96`. The first part of the `slot0` value is `sqrtPriceX96`. Take `CURRENT_SLOT0_VALUE`, remove the `0x` prefix, and take the first 40 hex characters (representing the 160 bits of `sqrtPriceX96`). Let's call this `CURRENT_SQRT_PRICE_X96`. You could plug this into the `v3_price_from_sqrt` function (perhaps modified slightly to take the hex string directly) to see the current price representation if desired, just to get a baseline.
6.  **Calculate *New* `sqrtPriceX96`:** We need to create a significantly different price. Let's aim for a price ~10% higher or lower.
    *   **Easier Method (Approximate):** Take the `CURRENT_SQRT_PRICE_X96` value (as a large number). Calculate roughly +/- 5% of this value (since sqrtPrice is proportional to the *square root* of the price, a 5% change in sqrtPrice leads to roughly a 10% change in price). Add or subtract this difference to get a `NEW_SQRT_PRICE_X96_APPROX` (as a large number). Convert this new large number back into its 160-bit hexadecimal representation (padded with leading zeros if needed to 40 hex characters).
    *   **More Precise Method:** Use the `v3_price_from_sqrt` logic *in reverse*. Take the current price (from step 5 or from the bot's output), increase/decrease it by 10%, then calculate the `sqrtPriceX96` corresponding to that *new* price using the formula `new_sqrtPriceX96 = sqrt(new_price / price_adjustment) * 2^96`. This requires using appropriate big number math libraries. The approximate method is likely sufficient for this test.
7.  **Construct *New* `slot0` Value:** The `slot0` storage combines `sqrtPriceX96` (160 bits), `tick` (24 bits), and other data. To minimize disruption, we'll *only* change the `sqrtPriceX96` part.
    *   Take the `CURRENT_SLOT0_VALUE`.
    *   Replace the first 40 hex characters (after `0x`) with your calculated `NEW_SQRT_PRICE_X96` hex value (ensure it's exactly 40 hex chars).
    *   Keep the remaining 24 hex characters of the original `slot0` value unchanged.
    *   This combined 64-character hex string (prefixed with `0x`) is your `NEW_SLOT0_VALUE`.
8.  **Prepare `cast rpc` Command:** Construct the command to write the new value. It uses the `anvil_setStorageAt` RPC method.
    ```bash
    # Replace NEW_SLOT0_VALUE with the hex value you constructed
    cast rpc anvil_setStorageAt --rpc-url http://127.0.0.1:8545 \
      "0x85149247691df622eaf1a8bd0cafd40bc45154a9" \
      "0x0" \
      "NEW_SLOT0_VALUE"
    ```
    *   Arguments: `target_address`, `storage_slot_index`, `new_value`.

**Expected Outcome:** You have identified the UniV3 pool address, confirmed slot 0 is the target, retrieved the current `slot0` value from Anvil, calculated a `NEW_SLOT0_VALUE` representing a significantly different price, and prepared the `cast rpc anvil_setStorageAt ...` command, ready to be executed in the next step.

---

