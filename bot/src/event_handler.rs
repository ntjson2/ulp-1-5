// bot/src/event_handler.rs

// FIX E0432: Import necessary items correctly
use crate::bindings::{ // Import all needed types from bindings
    i_uniswap_v3_factory::PoolCreatedFilter as UniV3PoolCreatedFilter,
    i_velodrome_factory::PoolCreatedFilter as VeloPoolCreatedFilter,
    uniswap_v3_pool::SwapFilter as UniV3SwapFilter,
    velodrome_v2_pool::SwapFilter as VeloV2SwapFilter,
    VelodromeV2Pool, // Keep pool types
    UniswapV3Pool, // Keep pool types
};
use crate::utils::{v2_price_from_reserves, v3_price_from_sqrt}; // Keep needed utils
use crate::config::Config; // Import Config
use dashmap::DashMap;
use ethers::{
    abi::RawLog, // Need RawLog
    contract::EthLogDecode, // Need trait for decoding
    prelude::*,
    types::{Address, Log, H256, I256, U256, U64},
};
use eyre::{Result, WrapErr}; // Keep Result and WrapErr
use std::sync::Arc;
use tracing::{debug, error, info, instrument, warn};
// Import helpers defined in main.rs
use crate::{is_target_pair_option, fetch_and_cache_pool_state};
// Import constants defined in main.rs
use crate::{UNI_V3_SWAP_TOPIC, VELO_V2_SWAP_TOPIC, UNI_V3_POOL_CREATED_TOPIC, VELO_V2_POOL_CREATED_TOPIC};
// Import path optimizer (placeholder for now)
use crate::path_optimizer::find_top_routes; // Placeholder


// --- State Definitions ---
#[derive(Debug, Clone)]
pub struct PoolState {
    pub pool_address: Address,
    pub dex_type: DexType,
    pub sqrt_price_x96: Option<U256>, // UniV3
    pub tick: Option<i32>,            // UniV3
    pub reserve0: Option<U256>,       // VeloV2
    pub reserve1: Option<U256>,       // VeloV2
    pub token0: Address,
    pub token1: Address,
    pub last_update_block: Option<U64>,
    pub uni_fee: Option<u32>,       // UniV3 fee tier
    pub velo_stable: Option<bool>,  // VeloV2 stability flag
    pub t0_is_weth: Option<bool>,   // True if token0 is WETH (for price normalization)
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DexType { UniswapV3, VelodromeV2, Unknown }

#[derive(Debug, Clone)]
pub struct AppState {
    pub pool_states: Arc<DashMap<Address, PoolState>>,
    // Include necessary config directly or derived values
    pub weth_address: Address,
    pub usdc_address: Address,
    pub weth_decimals: u8,
    pub usdc_decimals: u8,
    pub config: Config, // Store the whole config or necessary parts
}

// --- Event Handlers ---

pub async fn handle_new_block(block_number: U64, _state: Arc<AppState>) -> Result<()> {
    info!("🧱 New Block Received: #{}", block_number);
    // Potentially trigger periodic checks here if needed
    Ok(())
}

#[instrument(skip_all, fields(tx_hash = ?log.transaction_hash, block = ?log.block_number, address = ?log.address))]
pub async fn handle_log_event(
    log: Log,
    state: Arc<AppState>,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, // Using HTTP client for potential state reads
) -> Result<()> {
    let event_sig = match log.topics.get(0) {
        Some(topic) => *topic,
        None => {
            warn!("Log missing topic 0");
            return Ok(()); // Ignore logs without topics
        }
    };
    let contract_address = log.address;

    // Avoid excessive logging for common events unless debug level is enabled
    // trace!(topic0 = ?event_sig, "Log Received");

    // --- Pool Creation Events ---
    if event_sig == *UNI_V3_POOL_CREATED_TOPIC {
        info!(factory = ?contract_address, "Handling Uniswap V3 PoolCreated event");
        // FIX E0277/E0034: Use EthEvent trait method decode_log which requires RawLog
        let raw_log: RawLog = log.clone().into(); // Clone and convert
        match UniV3PoolCreatedFilter::decode_log(&raw_log) {
            Ok(event) => {
                info!(pool = ?event.pool, token0 = ?event.token_0, token1 = ?event.token_1, fee = %event.fee, "New UniV3 Pool Detected");
                // Check if this pool involves the target pair (WETH/USDC)
                if is_target_pair_option(event.token_0, event.token_1, state.target_pair()) {
                    info!("✨ Target pair UniV3 pool found! Fetching state...");
                    let app_state_clone = state.clone();
                    let client_clone = client.clone();
                    tokio::spawn(async move {
                        if let Err(e) = fetch_and_cache_pool_state(event.pool, DexType::UniswapV3, client_clone, app_state_clone).await {
                            error!(pool=?event.pool, error=?e, "Failed to fetch state for new UniV3 pool");
                        } else {
                            info!(pool=?event.pool, "State fetched for new UniV3 pool.");
                            // Ideally, dynamically update the main log filter here, but that's complex.
                            // For now, it will be picked up on the next restart or if already included.
                            // Consider adding logic to update the filter dynamically if necessary.
                        }
                    });
                } else {
                    debug!("Ignoring new UniV3 pool - not target pair.");
                }
            }
            Err(e) => error!(address = ?contract_address, error = ?e, "Failed to decode UniV3 PoolCreated log"),
        }

    } else if event_sig == *VELO_V2_POOL_CREATED_TOPIC {
        info!(factory = ?contract_address, "Handling Velodrome V2 PoolCreated event");
        let raw_log: RawLog = log.clone().into();
        match VeloPoolCreatedFilter::decode_log(&raw_log) {
            Ok(event) => {
                info!(pool = ?event.pool, token0 = ?event.token_0, token1 = ?event.token_1, stable = %event.stable, "New VeloV2 Pool Detected");
                if is_target_pair_option(event.token_0, event.token_1, state.target_pair()) {
                    info!("✨ Target pair VeloV2 pool found! Fetching state...");
                    let app_state_clone = state.clone();
                    let client_clone = client.clone();
                    tokio::spawn(async move {
                        if let Err(e) = fetch_and_cache_pool_state(event.pool, DexType::VelodromeV2, client_clone, app_state_clone).await {
                            error!(pool=?event.pool, error=?e, "Failed to fetch state for new VeloV2 pool");
                        } else {
                            info!(pool=?event.pool, "State fetched for new VeloV2 pool.");
                        }
                    });
                } else {
                    debug!("Ignoring new VeloV2 pool - not target pair.");
                }
            }
            Err(e) => error!(address = ?contract_address, error = ?e, "Failed to decode VeloV2 PoolCreated log"),
        }

    // --- Swap Events ---
    } else if event_sig == *UNI_V3_SWAP_TOPIC {
        // Check if the pool emitting the swap is one we are tracking
        if state.pool_states.contains_key(&contract_address) {
             debug!(pool = ?contract_address, "Handling Uniswap V3 Swap event");
             let raw_log: RawLog = log.clone().into();
             match UniV3SwapFilter::decode_log(&raw_log) {
                 Ok(swap) => {
                     let block_number = log.block_number; // Get block number from original log

                     // Update pool state in the DashMap
                     let update_result = state.pool_states.entry(contract_address)
                         .and_modify(|ps| {
                             debug!(pool = ?contract_address, old_sqrtP = ?ps.sqrt_price_x96, new_sqrtP = ?swap.sqrt_price_x96, old_tick = ?ps.tick, new_tick = ?swap.tick, "Updating UniV3 state");
                             ps.sqrt_price_x96 = Some(swap.sqrt_price_x96);
                             ps.tick = Some(swap.tick);
                             ps.last_update_block = block_number;
                         });
                        // Check if modification happened (entry existed)
                         if update_result.exists() {
                            debug!(pool = ?update_result.key(), "UniV3 State Updated");

                            // Check for arbitrage opportunities involving this updated pool
                            // Spawn check in a separate task to avoid blocking log processing
                            let state_clone = state.clone();
                            let client_clone = client.clone();
                            tokio::spawn(async move {
                                if let Err(e) = check_for_arbitrage(contract_address, state_clone, client_clone).await {
                                    error!(pool = ?contract_address, error = ?e, "Error checking arbitrage after UniV3 swap");
                                }
                            });
                        } else {
                            warn!(pool = ?contract_address, "Attempted to modify non-existent pool state for UniV3 swap");
                        }

                 }
                 Err(e) => error!(pool=?contract_address, error = ?e, "Failed to decode UniV3 Swap log"),
             }
         } else {
             // This pool is not in our tracked list, ignore swap.
             // trace!(pool = ?contract_address, "Ignoring UniV3 Swap log for untracked pool.");
         }

    } else if event_sig == *VELO_V2_SWAP_TOPIC {
        if state.pool_states.contains_key(&contract_address) {
            debug!(pool = ?contract_address, "Handling Velodrome V2 Swap event");
            let raw_log: RawLog = log.clone().into();
            // Decode the log just to confirm it's a valid swap, even if we don't use its data directly yet
            // FIX E0034/E0609: VeloV2SwapFilter is correct type, use it; no meta field needed
            match VeloV2SwapFilter::decode_log(&raw_log) {
                 Ok(_swap_data) => {
                     let block_number = log.block_number; // Get block number from original log
                     let state_clone = state.clone();
                     let client_clone = client.clone();

                     // Spawn a task to fetch new reserves asynchronously
                     tokio::spawn(async move {
                         debug!(pool = ?contract_address, "Fetching VeloV2 reserves after swap...");
                         let velo_pool = VelodromeV2Pool::new(contract_address, client_clone.clone());
                         match velo_pool.get_reserves().call().await {
                             Ok(reserves) => {
                                  let (reserve0, reserve1, _timestamp) = reserves;
                                  // Update state
                                  let update_result = state_clone.pool_states.entry(contract_address).and_modify(|ps| {
                                      debug!(pool = ?contract_address, old_r0 = ?ps.reserve0, new_r0 = ?reserve0, old_r1 = ?ps.reserve1, new_r1 = ?reserve1, "Updating VeloV2 state");
                                      ps.reserve0 = Some(reserve0);
                                      ps.reserve1 = Some(reserve1);
                                      ps.last_update_block = block_number;
                                  });

                                 if update_result.exists() {
                                     debug!(pool = ?update_result.key(), "VeloV2 State Updated");
                                     // Check for arbitrage after state update
                                     if let Err(e) = check_for_arbitrage(contract_address, state_clone, client_clone).await {
                                         error!(pool=?contract_address, error=?e, "Error checking arbitrage after VeloV2 update");
                                     }
                                 } else {
                                     warn!(pool = ?contract_address, "Attempted to modify non-existent pool state for VeloV2 swap");
                                 }
                             },
                             Err(e) => {
                                 error!(pool=?contract_address, error=?e, "Failed to fetch VeloV2 reserves after swap");
                             }
                         }
                     });
                 }
                 Err(e) => error!(pool=?contract_address, error = ?e, "Failed to decode VeloV2 Swap log"),
             }
         } else {
             // trace!(pool = ?contract_address, "Ignoring VeloV2 Swap log for untracked pool.");
         }
    } else {
        // Ignore other irrelevant logs
        // trace!(pool=?contract_address, topic0=?event_sig, "Ignoring irrelevant log");
    }
    Ok(())
 }


// FIX E0583: Remove skip for client as it's needed for potential simulations/executions later
// Instrument arbitrage check, skip state and client for cleaner logs, show updated pool
#[instrument(skip(state, client), fields(updated_pool=%updated_pool_address), level = "debug")]
async fn check_for_arbitrage(
    updated_pool_address: Address,
    state: Arc<AppState>,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
) -> Result<()> {
    debug!("Checking for arbitrage opportunities...");

    // 1. Get the state of the pool that was just updated
    // FIX E0382: Clone the specific PoolState entry we need
    let updated_pool_state = match state.pool_states.get(&updated_pool_address) {
        Some(entry) => entry.value().clone(), // Clone the PoolState from the DashMap entry
        None => {
            warn!(pool = ?updated_pool_address, "Attempted arbitrage check on untracked pool after update");
            return Ok(()); // Should not happen if logic is correct, but handle defensively
        }
    };

    // 2. Check if the updated pool is part of the target pair (WETH/USDC)
    // Use the cloned state here
    if !is_target_pair_option(updated_pool_state.token0, updated_pool_state.token1, state.target_pair()) {
         debug!("Updated pool is not the target pair, skipping arbitrage check.");
         return Ok(());
    }

    // 3. Calculate the price for the updated pool (WETH per USDC)
    // Price calculation might fail, handle it gracefully.
    let updated_price_weth_per_usdc = match calculate_pool_price_weth_per_usdc(&updated_pool_state, &state) {
        Ok(p) if p.is_finite() && p > 0.0 => p,
        Ok(p) => {
            debug!(pool = ?updated_pool_address, price = p, "Calculated non-positive or non-finite price for updated pool, skipping check.");
            return Ok(());
        }
        Err(e) => {
            warn!(pool = ?updated_pool_address, error = ?e, "Could not calculate price for updated pool, skipping check.");
            return Ok(()); // Can't proceed without a valid price
        }
    };
    debug!(pool = ?updated_pool_address, price = updated_price_weth_per_usdc, "Updated pool price calculated");


    // --- Pathfinding & Simulation Placeholder ---
    // This is where the path optimizer will be called
    debug!("Finding potential routes involving updated pool...");
    // Pass necessary parts of state to the optimizer
    let top_routes = find_top_routes(
        &updated_pool_state,
        &state.pool_states, // Pass the whole map of states
        &state.config,
        state.weth_address,
        state.usdc_address,
        state.weth_decimals,
        state.usdc_decimals
    );

    if top_routes.is_empty() {
        debug!("No promising arbitrage routes found involving the updated pool.");
        return Ok(());
    }

    // If routes are found, log at INFO level
    info!(pool = ?updated_pool_address, count = top_routes.len(), "Found potential arbitrage routes!");

    // TODO MU2 Task 7: Iterate through `top_routes`, call `find_optimal_loan_amount`,
    // and potentially trigger execution if profitable.
    for route in top_routes {
        // Log details of the promising route
        info!(
            buy_pool =?route.buy_pool_addr,
            buy_dex = ?route.buy_dex_type,
            sell_pool =?route.sell_pool_addr,
            sell_dex = ?route.sell_dex_type,
            est_profit_usd = route.estimated_profit_usd, // Assuming path_optimizer provides this
            "Evaluating Route"
        );

        // --- Trigger Simulation Task ---
        // Spawn simulation in a separate task
        let sim_state = state.clone();
        let sim_client = client.clone();
        tokio::spawn(async move {
            info!(buy_pool =?route.buy_pool_addr, sell_pool =?route.sell_pool_addr, "Spawning simulation task for route");

            // TODO MU2 Task 6: Get current gas price estimate
            let current_gas_price_gwei = 0.01; // Placeholder - fetch dynamically

            // TODO MU2 Task 7: Call find_optimal_loan_amount
            // let optimal_loan_result = find_optimal_loan_amount(
            //     sim_client.clone(),
            //     sim_state.config.min_loan_amount_weth,
            //     sim_state.config.max_loan_amount_weth,
            //     sim_state.config.optimal_loan_search_iterations,
            //     sim_state.weth_address, // token_in (loan token)
            //     sim_state.usdc_address, // token_out (intermediate token)
            //     sim_state.weth_decimals,
            //     crate::FLASH_LOAN_FEE_RATE, // Use constant or config value
            //     current_gas_price_gwei,
            //     &route.buy_dex_type.to_string(), // Convert DexType enum to string slice
            //     &route.sell_dex_type.to_string(),
            //     route.buy_pool_addr, route.sell_pool_addr,
            //     route.buy_pool_stable.unwrap_or(false), // Default to false if None
            //     route.sell_pool_stable.unwrap_or(false),
            //     route.buy_pool_fee.unwrap_or(0), // Default to 0 if None
            //     route.sell_pool_fee.unwrap_or(0),
            //     sim_state.velo_router_instance, // Need to pass actual instances
            //     sim_state.uni_quoter_instance,
            //     sim_state.config.arb_executor_address.expect("Executor address needed"),
            //     sim_state.config.balancer_vault_address,
            //     sim_state.config.velo_router_addr,
            //     route.buy_pool_addr, // pool_a_addr
            //     route.sell_pool_addr, // pool_b_addr
            //     route.zero_for_one_a, // Direction of first swap (WETH -> USDC?)
            //     route.buy_dex_type == DexType::VelodromeV2, // is_a_velo
            //     route.sell_dex_type == DexType::VelodromeV2, // is_b_velo
            // ).await;

            // match optimal_loan_result {
            //     Ok(Some((loan_amount, net_profit))) => {
            //         if net_profit > I256::zero() { // Or some positive threshold
            //             info!(?route, loan_amount = ?loan_amount, net_profit_wei = ?net_profit, "🎉 PROFITABLE OPPORTUNITY FOUND!");
            //             // TODO MU2 Task 8: Execute Transaction
            //             // Call execute_arbitrage(...)
            //         } else {
            //             debug!(?route, "Simulation complete, but net profit is not positive.");
            //         }
            //     }
            //     Ok(None) => {
            //          debug!(?route, "Simulation complete, no profitable loan amount found.");
            //     }
            //     Err(e) => {
            //          error!(?route, error = ?e, "Error during optimal loan amount search");
            //     }
            // }
        }); // End simulation task spawn
    } // End loop through routes

    Ok(())
}


/// Helper impl for AppState
impl AppState {
    // FIX E0599: Ensure method definition is correct and returns Option<(Address, Address)>
    pub fn target_pair(&self) -> Option<(Address, Address)> {
        // Assuming WETH/USDC are the primary targets defined in config
        if !self.weth_address.is_zero() && !self.usdc_address.is_zero() {
             // Return consistently ordered pair (e.g., lower address first for uniqueness)
            if self.weth_address < self.usdc_address {
                Some((self.weth_address, self.usdc_address))
            } else {
                Some((self.usdc_address, self.weth_address))
            }
        } else {
            warn!("WETH or USDC address is zero in config, target pair filtering disabled.");
            None // Return None if addresses are not set or zero
        }
    }
}

/// Helper to calculate price of WETH in terms of USDC for a given pool.
/// Returns Ok(price) where price is USDC per WETH, or Error.
/// Handles potential errors during calculation.
#[instrument(level="trace", skip(app_state), fields(pool=%pool_state.pool_address, dex=?pool_state.dex_type))]
fn calculate_pool_price_weth_per_usdc(pool_state: &PoolState, app_state: &AppState) -> Result<f64> {
    // FIX E0308: Ensure all paths return Result<f64> and handle potential None values safely
    let weth_decimals = app_state.weth_decimals;
    let usdc_decimals = app_state.usdc_decimals;
    let weth_address = app_state.weth_address;

    // Determine if token0 is WETH for this specific pool
    // Use the cached flag if available, otherwise determine and log a warning
    let t0_is_weth = match pool_state.t0_is_weth {
        Some(is_weth) => is_weth,
        None => {
            warn!(pool = ?pool_state.pool_address, "t0_is_weth flag not cached, determining now.");
             pool_state.token0 == weth_address
        }
    };

    // Calculate the raw price of token1 in terms of token0
    let price_t1_per_t0 = match pool_state.dex_type {
        DexType::UniswapV3 => {
            let sqrt_price = pool_state.sqrt_price_x96.ok_or_else(|| eyre::eyre!("Missing sqrtPriceX96 for UniV3 pool {}", pool_state.pool_address))?;
            // Decimals depend on which token is t0
            let (dec0, dec1) = if t0_is_weth { (weth_decimals, usdc_decimals) } else { (usdc_decimals, weth_decimals) };
            v3_price_from_sqrt(sqrt_price, dec0, dec1)
                .wrap_err_with(|| format!("Failed V3 price calculation for pool {}", pool_state.pool_address))?
        },
        DexType::VelodromeV2 => {
            let r0 = pool_state.reserve0.ok_or_else(|| eyre::eyre!("Missing reserve0 for VeloV2 pool {}", pool_state.pool_address))?;
            let r1 = pool_state.reserve1.ok_or_else(|| eyre::eyre!("Missing reserve1 for VeloV2 pool {}", pool_state.pool_address))?;
            // Decimals depend on which token is t0
            let (dec0, dec1) = if t0_is_weth { (weth_decimals, usdc_decimals) } else { (usdc_decimals, weth_decimals) };
            v2_price_from_reserves(r0, r1, dec0, dec1)
                 .wrap_err_with(|| format!("Failed V2 price calculation for pool {}", pool_state.pool_address))?
        },
        DexType::Unknown => return Err(eyre::eyre!("Cannot calculate price for pool {} with Unknown DEX type", pool_state.pool_address)),
    };

    // price_t1_per_t0 is the price of token1 in terms of token0.
    // We want the price of WETH in terms of USDC.

    let price_weth_per_usdc = if t0_is_weth {
        // T0 is WETH, T1 is USDC. price_t1_per_t0 = Price(USDC) / Price(WETH)
        // We want Price(WETH) / Price(USDC), which is 1.0 / price_t1_per_t0
        if price_t1_per_t0.abs() < f64::EPSILON {
            // Avoid division by zero if price is effectively zero
            // Returning an error is safer than returning 0.0 or Infinity
            return Err(eyre::eyre!("Calculated intermediate price (t1/t0) is near zero, cannot invert for WETH/USDC price (pool {})", pool_state.pool_address));
        }
        1.0 / price_t1_per_t0
    } else {
        // T0 is USDC, T1 is WETH. price_t1_per_t0 = Price(WETH) / Price(USDC)
        // This is already the format we want.
        price_t1_per_t0
    };

    // Final check for valid finite number
    if !price_weth_per_usdc.is_finite() {
         return Err(eyre::eyre!("Calculated non-finite WETH/USDC price ({}) for pool {}", price_weth_per_usdc, pool_state.pool_address));
    }

    trace!(price = price_weth_per_usdc, "Calculated WETH/USDC price");
    Ok(price_weth_per_usdc)
}

// END OF FILE: bot/src/event_handler.rs