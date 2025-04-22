// bot/src/event_handler.rs

use crate::bindings::{UniswapV3Pool, VelodromeV2Pool}; // Import bindings for decoding
use crate::utils::{v2_price_from_reserves, v3_price_from_sqrt}; // For price calculation after event
use dashmap::DashMap; // Concurrent HashMap
use ethers::{
    prelude::*,
    types::{Address, Log, H256, I256, U256, U64}, // Add I256 if profit is calculated here
};
use eyre::Result;
use std::sync::Arc;
use tracing::{debug, error, info, warn}; // Use tracing

// --- State Definitions ---

// Represents the latest known state of a single pool
#[derive(Debug, Clone)]
pub struct PoolState {
    pub pool_address: Address,
    pub dex_type: DexType, // UniV3 or VeloV2
    // UniV3 specific
    pub sqrt_price_x96: Option<U256>,
    pub tick: Option<i32>, // Use i32 for tick
    // VeloV2 specific
    pub reserve0: Option<U256>,
    pub reserve1: Option<U256>,
    // Common / Derived
    pub token0: Address, // Store token addresses for context
    pub token1: Address,
    pub last_update_block: Option<U64>,
    // Add other static info if needed (fee, stable)
    pub uni_fee: Option<u32>,
    pub velo_stable: Option<bool>,
    pub t0_is_weth: Option<bool>, // To know price direction WETH/USDC
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DexType {
    UniswapV3,
    VelodromeV2,
    Unknown,
}

// Shared application state, accessible across tasks/threads
#[derive(Debug, Clone)]
pub struct AppState {
    // Map pool address to its latest known state
    pub pool_states: Arc<DashMap<Address, PoolState>>,
    // Add other shared info if needed (e.g., token decimals, config reference)
    pub weth_address: Address,
    pub usdc_address: Address,
    pub weth_decimals: u8,
    pub usdc_decimals: u8,
}

// --- Event Handlers ---

pub async fn handle_new_block(block_number: U64, _provider: Arc<Provider<Ws>>) -> Result<()> {
    info!("🧱 New Block Received: #{}", block_number);
    // Could trigger periodic state refresh/validation here
    Ok(())
}

/// Decodes Swap events and updates the AppState cache.
pub async fn handle_log_event(log: Log, state: AppState, provider: Arc<Provider<Ws>>) -> Result<()> {
    debug!(address = ?log.address, topics = ?log.topics, block = ?log.block_number, "Log Received");

    // --- Event Signature Hashes (Keccak256) ---
    // TODO: Define these properly, perhaps load from bindings or calculate once
    // Example: keccak256("Swap(address,address,int256,int256,uint160,uint128,int24)") for UniV3
    let uni_v3_swap_topic = H256::from_slice(&hex::decode("c42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67").unwrap_or_default());
    // Example: keccak256("Swap(address,uint256,uint256,uint256,uint256,address)") for VeloV2
    let velo_v2_swap_topic = H256::from_slice(&hex::decode("d78ad95fa46c994b6551d0680f3b11d8ff68cb1f753454c66d2f1312cd7e52f6").unwrap_or_default());

    let event_sig = log.topics.get(0).cloned().unwrap_or_default();
    let pool_address = log.address;

    // --- Identify Event and Update State ---
    if event_sig == uni_v3_swap_topic {
        debug!(pool = ?pool_address, "Handling Uniswap V3 Swap event");
        // Decode UniV3 Swap event
        // event Swap(address indexed sender, address indexed recipient, int256 amount0, int256 amount1, uint160 sqrtPriceX96, uint128 liquidity, int24 tick)
        if let Ok(swap) = UniswapV3Pool::SwapFilter::decode_log(&log.into()) {
            // Update state cache for this pool
            if let Some(mut pool_entry) = state.pool_states.get_mut(&pool_address) {
                pool_entry.sqrt_price_x96 = Some(swap.sqrt_price_x96);
                pool_entry.tick = Some(swap.tick);
                pool_entry.last_update_block = Some(swap.meta.block_number); // Use meta if available
                debug!(pool = ?pool_address, sqrtP = %swap.sqrt_price_x96, tick = %swap.tick, "UniV3 State Updated");

                // Trigger arbitrage check after state update
                check_for_arbitrage(pool_address, state.clone()).await?; // Clone Arc<DashMap>
            } else {
                warn!(pool = ?pool_address, "Received UniV3 Swap for untracked pool");
                // TODO: Add logic to fetch initial state for newly seen pools
            }
        } else {
             error!(pool=?pool_address, topics=?log.topics, data=?log.data, "Failed to decode UniV3 Swap log");
        }

    } else if event_sig == velo_v2_swap_topic {
         debug!(pool = ?pool_address, "Handling Velodrome V2 Swap event");
         // Decode VeloV2 Swap event
         // event Swap(address indexed sender, uint amount0In, uint amount1In, uint amount0Out, uint amount1Out, address indexed to);
         if let Ok(swap) = VelodromeV2Pool::SwapFilter::decode_log(&log.into()) {
             // Velo Swap doesn't directly give reserves. We *must* re-query getReserves.
             warn!(pool = ?pool_address, "VeloV2 Swap detected, re-querying reserves...");
             let velo_pool = VelodromeV2Pool::new(pool_address, provider.clone()); // Need provider
             match velo_pool.get_reserves().call().await {
                 Ok(reserves) => {
                     if let Some(mut pool_entry) = state.pool_states.get_mut(&pool_address) {
                         pool_entry.reserve0 = Some(reserves.0.into()); // Assuming reserves are U112/U112/U32 originally
                         pool_entry.reserve1 = Some(reserves.1.into());
                         pool_entry.last_update_block = Some(swap.meta.block_number);
                         debug!(pool = ?pool_address, r0 = %reserves.0, r1 = %reserves.1, "VeloV2 State Updated via getReserves");

                         // Trigger arbitrage check after state update
                         check_for_arbitrage(pool_address, state.clone()).await?;
                     } else {
                         warn!(pool = ?pool_address, "Received VeloV2 Swap for untracked pool");
                         // TODO: Add logic to fetch initial state
                     }
                 },
                 Err(e) => {
                     error!(pool = ?pool_address, error = ?e, "Failed to query VeloV2 reserves after Swap event");
                 }
             }
         } else {
             error!(pool=?pool_address, topics=?log.topics, data=?log.data, "Failed to decode VeloV2 Swap log");
         }
    } else {
        // Log other events if filter includes them unexpectedly
        debug!(pool=?pool_address, topic0=?event_sig, "Ignoring irrelevant log event");
    }

    Ok(())
}


/// Placeholder function to check for arbitrage opportunities after a state update.
async fn check_for_arbitrage(updated_pool_address: Address, state: AppState) -> Result<()> {
    debug!(pool = ?updated_pool_address, "Checking for arbitrage opportunities...");

    // 1. Get updated pool state from cache
    let updated_pool_state = match state.pool_states.get(&updated_pool_address) {
        Some(entry) => entry.value().clone(), // Clone needed data to avoid holding lock
        None => {
            warn!(pool = ?updated_pool_address, "State not found in cache during arbitrage check");
            return Ok(()); // Cannot check if state isn't cached
        }
    };

    // 2. Identify the counterparty token (WETH or USDC)
    let token0 = updated_pool_state.token0;
    let token1 = updated_pool_state.token1;
    let counterparty_token = if token0 == state.weth_address || token0 == state.usdc_address {
        if token1 == state.weth_address || token1 == state.usdc_address {
            // This IS one of our target pairs (WETH/USDC)
            token1 // If token0 is WETH/USDC, token1 is the other
        } else {
             debug!("Updated pool is not a target WETH/USDC pair (token1 mismatch)");
             return Ok(()); // Not a target pair
        }
    } else {
         debug!("Updated pool is not a target WETH/USDC pair (token0 mismatch)");
         return Ok(()); // Not a target pair
    };

    // 3. Find potential matching pools in the cache
    // Iterate through all cached pools to find pools with the same token pair
    let mut potential_arbs = Vec::new();
    for entry in state.pool_states.iter() {
        let other_pool_addr = *entry.key();
        let other_pool_state = entry.value();

        // Skip if it's the same pool that just updated
        if other_pool_addr == updated_pool_address { continue; }

        // Check if the other pool has the same token pair (order doesn't matter)
        if (other_pool_state.token0 == token0 && other_pool_state.token1 == token1) ||
           (other_pool_state.token0 == token1 && other_pool_state.token1 == token0)
        {
             // Potential arbitrage candidate found
             debug!("Found potential arbitrage candidate: {:?} vs {:?}", updated_pool_address, other_pool_addr);
             potential_arbs.push(other_pool_state.clone()); // Clone state for processing
        }
    }

    // 4. Calculate Prices and Check Spreads
    if potential_arbs.is_empty() {
        debug!("No other pools found for pair {:?}/{:?}", token0, token1);
        return Ok(());
    }

    // Calculate price for the updated pool
    let updated_price_weth_usdc = match calculate_pool_price_weth_per_usdc(&updated_pool_state, &state) {
         Ok(p) => p,
         Err(e) => { warn!("Could not calculate price for updated pool {:?}: {}", updated_pool_address, e); return Ok(()); }
    };
    if updated_price_weth_usdc <= 0.0 { return Ok(()); } // Skip if price is invalid

    // Check against each potential counterparty pool
    for other_pool_state in potential_arbs {
        let other_pool_addr = other_pool_state.pool_address;
        let other_price_weth_usdc = match calculate_pool_price_weth_per_usdc(&other_pool_state, &state) {
            Ok(p) => p,
            Err(e) => { warn!("Could not calculate price for other pool {:?}: {}", other_pool_addr, e); continue; } // Skip this candidate pool
        };
         if other_price_weth_usdc <= 0.0 { continue; } // Skip if price is invalid

        // Calculate spread (ensure consistent price direction, e.g., WETH per USDC)
        let price_diff = (updated_price_weth_usdc - other_price_weth_usdc).abs();
        let base_price = updated_price_weth_usdc.min(other_price_weth_usdc);
        let spread_percentage = if base_price > 1e-18 { (price_diff / base_price) * 100.0 } else { 0.0 };

        info!(pool1 = ?updated_pool_address, price1 = %updated_price_weth_usdc, pool2 = ?other_pool_addr, price2 = %other_price_weth_usdc, spread = %format!("{:.4}%", spread_percentage), "Arbitrage Check");

        // TODO: If spread > threshold, trigger the full optimization and execution flow
        // This would involve calling find_optimal_loan_amount, then potentially sending the tx
        // Need to pass client, config, etc. - maybe structure this differently (e.g., send details to a separate task queue?)
        if spread_percentage > ARBITRAGE_THRESHOLD_PERCENTAGE { // Use constant defined in main? Pass config?
             warn!(pool1 = ?updated_pool_address, pool2 = ?other_pool_addr, spread = %spread_percentage, ">>> POTENTIAL ARBITRAGE OPPORTUNITY FOUND (Execution TODO) <<<");
             // Call find_optimal_loan_amount(...) -> send_tx(...)
        }
    }


    Ok(())
}

/// Helper to calculate WETH per USDC price from cached pool state
fn calculate_pool_price_weth_per_usdc(pool_state: &PoolState, app_state: &AppState) -> Result<f64> {
     let weth_decimals = app_state.weth_decimals;
     let usdc_decimals = app_state.usdc_decimals;
     let t0_is_weth = pool_state.t0_is_weth.ok_or_else(|| eyre::eyre!("Missing t0_is_weth flag"))?;

     match pool_state.dex_type {
          DexType::UniswapV3 => {
               let sqrt_price = pool_state.sqrt_price_x96.ok_or_else(|| eyre::eyre!("Missing sqrtPriceX96"))?;
               // v3 price is T1/T0
               let price_t1_t0 = v3_price_from_sqrt(sqrt_price, weth_decimals, usdc_decimals)?; // Assume T0=WETH, T1=USDC for calculation
               if t0_is_weth { // If T0 is WETH, price is USDC/WETH, need 1/price for WETH/USDC
                    if price_t1_t0.abs() < f64::EPSILON { Ok(0.0) } else { Ok(1.0 / price_t1_t0) }
               } else { // T0 is USDC, price is WETH/USDC
                    Ok(price_t1_t0)
               }
          },
          DexType::VelodromeV2 => {
               let r0 = pool_state.reserve0.ok_or_else(|| eyre::eyre!("Missing reserve0"))?;
               let r1 = pool_state.reserve1.ok_or_else(|| eyre::eyre!("Missing reserve1"))?;
                // v2 price is T1/T0
               let (dec0, dec1) = if t0_is_weth { (weth_decimals, usdc_decimals) } else { (usdc_decimals, weth_decimals) };
               let price_t1_t0 = v2_price_from_reserves(r0, r1, dec0, dec1)?;
               if t0_is_weth { // If T0 is WETH, price is USDC/WETH, need 1/price for WETH/USDC
                    if price_t1_t0.abs() < f64::EPSILON { Ok(0.0) } else { Ok(1.0 / price_t1_t0) }
               } else { // T0 is USDC, price is WETH/USDC
                    Ok(price_t1_t0)
               }
          },
          DexType::Unknown => Err(eyre::eyre!("Cannot calculate price for unknown DEX type")),
     }
}


// END OF FILE: bot/src/event_handler.rs