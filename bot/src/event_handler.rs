// bot/src/event_handler.rs

use crate::bindings::{
    AerodromePool,
    VelodromeV2Pool,
    uniswap_v3_pool::SwapFilter as UniV3SwapFilter, // Alias for clarity
    velodrome_v2_pool::SwapFilter as VeloSwapFilter, // Alias for clarity
    i_uniswap_v3_factory::PoolCreatedFilter as UniV3PoolCreatedFilter, // Alias
    i_velodrome_factory::PoolCreatedFilter as VeloPoolCreatedFilter, // Alias
};
use crate::state::{self, AppState, DexType};
use crate::path_optimizer::{find_top_routes, RouteCandidate};
use crate::simulation::find_optimal_loan_amount;
use crate::{
    UNI_V3_POOL_CREATED_TOPIC, UNI_V3_SWAP_TOPIC, VELO_AERO_POOL_CREATED_TOPIC,
    VELO_AERO_SWAP_TOPIC,
};
use crate::transaction::{submit_arbitrage_transaction, NonceManager};
use crate::utils::ToF64Lossy;

use ethers::{
    abi::RawLog,
    contract::{EthLogDecode, ContractCall}, // Added ContractCall
    prelude::*,
    types::{Log, U64, I256, U256, Address},
};
use eyre::{Result};
use std::{sync::Arc, time::Duration};
use tokio::time::timeout;
use tracing::{debug, error, info, instrument, trace, warn};


// --- Event Handlers ---

pub async fn handle_new_block(block_number: U64, _state: Arc<AppState>) -> Result<()> {
    info!("🧱 New Block Received: #{}", block_number);
    // TODO: Potentially trigger periodic checks or updates based on block number
    Ok(())
}

/// Processes individual log events. Updates hot-cache, triggers checks.
#[instrument(skip_all, fields(tx_hash = ?log.transaction_hash, block = ?log.block_number, address = ?log.address))]
pub async fn handle_log_event(
    log: Log,
    state: Arc<AppState>,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    nonce_manager: Arc<NonceManager>,
) -> Result<()> {
    // Ensure topic0 exists
    let event_sig = match log.topics.get(0) {
        Some(t) => *t,
        None => {
            warn!("Log missing topic0, cannot identify event.");
            return Ok(());
        }
    };
    let contract_address = log.address;

    // Use static references for comparison
    let velo_aero_pool_created_topic = *VELO_AERO_POOL_CREATED_TOPIC;
    let velo_aero_swap_topic = *VELO_AERO_SWAP_TOPIC;
    let uni_v3_pool_created_topic = *UNI_V3_POOL_CREATED_TOPIC;
    let uni_v3_swap_topic = *UNI_V3_SWAP_TOPIC;


    // --- Pool Creation Events ---
    if event_sig == uni_v3_pool_created_topic {
        // Check if the event is from the configured Uniswap V3 factory
        if contract_address != state.config.uniswap_v3_factory_addr {
            trace!("Ignoring UniV3 PoolCreated log from non-factory address: {}", contract_address);
            return Ok(());
        }
        let raw_log: RawLog = log.clone().into();
        match <UniV3PoolCreatedFilter as EthLogDecode>::decode_log(&raw_log) {
            Ok(event) => {
                 // Check if the created pool involves the target pair (WETH/USDC)
                if state::is_target_pair_option(event.token_0, event.token_1, state.target_pair()) {
                    info!(pool=%event.pool, fee=%event.fee, "✨ Target UniV3 pool created! Fetching state...");
                    let s = state.clone();
                    let c = client.clone();
                    // Spawn task to fetch state for the newly created pool
                    tokio::spawn(async move {
                         let fetch_result = state::fetch_and_cache_pool_state(event.pool, DexType::UniswapV3, contract_address, c, s).await;
                         if let Err(e) = fetch_result {
                              error!(pool=%event.pool, factory=%contract_address, error=?e, "Fetch state failed for new UniV3 pool");
                          }
                    });
                } else {
                    trace!(pool=%event.pool, "Ignoring non-target pair UniV3 pool creation.");
                }
            }
            Err(e) => error!(address=%contract_address, error=?e, "Failed to decode UniV3 PoolCreated event"),
        }
    } else if event_sig == velo_aero_pool_created_topic {
         // Determine which DEX factory emitted the event
        let dex_type = if contract_address == state.config.velodrome_v2_factory_addr {
            DexType::VelodromeV2
        } else if Some(contract_address) == state.config.aerodrome_factory_addr {
            DexType::Aerodrome
        } else {
            trace!("Ignoring Velo/Aero PoolCreated log from non-factory address: {}", contract_address);
            return Ok(());
        };

        let raw_log: RawLog = log.clone().into();
        match <VeloPoolCreatedFilter as EthLogDecode>::decode_log(&raw_log) {
             Ok(event) => {
                 // Check if the created pool involves the target pair
                 if state::is_target_pair_option(event.token_0, event.token_1, state.target_pair()) {
                    info!(pool=%event.pool, dex=?dex_type, stable=%event.stable, "✨ Target {:?} pool created! Fetching state...", dex_type);
                     let s=state.clone();
                     let c=client.clone();
                     // Spawn task to fetch state
                     tokio::spawn(async move {
                         let fetch_result = state::fetch_and_cache_pool_state(event.pool, dex_type, contract_address, c, s).await;
                         if let Err(e) = fetch_result {
                              error!(pool=%event.pool, factory=%contract_address, dex=?dex_type, error=?e, "Fetch state failed for new Velo/Aero pool");
                          }
                     });
                 } else {
                    trace!(pool=%event.pool, "Ignoring non-target pair Velo/Aero pool creation.");
                 }
             }
             Err(e) => error!(address=%contract_address, error=?e, "Failed to decode Velo/Aero PoolCreated event"),
         }

    // --- Swap Events ---
    } else if event_sig == uni_v3_swap_topic {
        // Only process swaps for pools we are actively tracking (in snapshots map)
        if let Some(mut snapshot_entry) = state.pool_snapshots.get_mut(&contract_address) {
            trace!(pool=%contract_address, "Handling UniV3 Swap");
            let raw_log: RawLog = log.clone().into();
            match <UniV3SwapFilter as EthLogDecode>::decode_log(&raw_log) {
                Ok(swap) => {
                    let block_number = log.block_number; // Get block number from the log metadata
                    // Update the snapshot cache with new price/tick info from the event
                    snapshot_entry.sqrt_price_x96 = Some(U256::from(swap.sqrt_price_x96)); // Convert u160
                    snapshot_entry.tick = Some(swap.tick);
                    snapshot_entry.last_update_block = block_number;
                    debug!(pool=%contract_address, tick=%swap.tick, "UniV3 Snapshot Updated from Swap event");

                    // Clone necessary Arcs for the spawned task
                    let s = state.clone();
                    let c = client.clone();
                    let nm = nonce_manager.clone();
                    // Spawn task to check for arbitrage opportunities involving this pool
                    tokio::spawn(async move {
                        if let Err(e) = check_for_arbitrage(contract_address, s, c, nm).await {
                            error!(pool=%contract_address, error=?e, "Check arbitrage task failed after UniV3 swap");
                        }
                    });
                }
                Err(e) => error!(pool=%contract_address, error=?e, "Failed to decode UniV3 Swap event"),
            }
        } // Ignore swaps from untracked pools

    } else if event_sig == velo_aero_swap_topic {
        // Check if we are tracking this Velo/Aero pool
        if let Some(snapshot_entry) = state.pool_snapshots.get(&contract_address) {
             // Get details needed before dropping the reference
             let dex_type = snapshot_entry.dex_type;
             let pool_address = *snapshot_entry.key();
             drop(snapshot_entry); // Release the read lock

             trace!(pool=%pool_address, dex=?dex_type, "Handling {:?} Swap", dex_type);
             // Decode the swap event (though we don't use its data directly, ensures it's valid)
             let raw_log: RawLog = log.clone().into();
             match <VeloSwapFilter as EthLogDecode>::decode_log(&raw_log) {
                 Ok(_swap_data) => { // We don't need swap_data, reserves are fetched below
                     let block_number = log.block_number;
                     // Clone necessary Arcs
                     let s = state.clone();
                     let c = client.clone();
                     let nm = nonce_manager.clone();
                     // Spawn task to fetch updated reserves and check for arbitrage
                     tokio::spawn(async move {
                         debug!(pool=%pool_address, dex=?dex_type, "Fetching reserves after swap...");
                         let timeout_duration = Duration::from_secs(s.config.fetch_timeout_secs.unwrap_or(10));

                         // FIX E0716: Create the ContractCall binding first
                         // Define the type explicitly for clarity if needed
                         type ReservesCall = ContractCall<SignerMiddleware<Provider<Http>, LocalWallet>, (U256, U256, U256)>;
                         let pool_call_binding: ReservesCall = if dex_type == DexType::VelodromeV2 {
                             let pool = VelodromeV2Pool::new(pool_address, c.clone());
                             pool.get_reserves() // Bind the call object
                         } else { // Assumes Aerodrome uses the same get_reserves signature
                             let pool = AerodromePool::new(pool_address, c.clone());
                             pool.get_reserves() // Bind the call object
                         };

                         // Now await the future obtained from call() outside the if/else
                         let pool_call_future = pool_call_binding.call();

                         // Fetch reserves with timeout
                         match timeout(timeout_duration, pool_call_future).await {
                            Ok(Ok(reserves)) => {
                                let (reserve0, reserve1, _ts): (U256, U256, U256) = reserves;
                                // Get a mutable reference to update the snapshot
                                if let Some(mut snapshot) = s.pool_snapshots.get_mut(&pool_address) {
                                    snapshot.reserve0 = Some(reserve0);
                                    snapshot.reserve1 = Some(reserve1);
                                    snapshot.last_update_block = block_number;
                                    debug!(pool=%pool_address, dex=?dex_type, r0=%reserve0, r1=%reserve1, "Velo/Aero Snapshot Updated after Swap");

                                    // Now check for arbitrage
                                    if let Err(e) = check_for_arbitrage(pool_address, s.clone(), c.clone(), nm.clone()).await {
                                        error!(pool=%pool_address, error=?e, "Check arbitrage task failed after Velo/Aero swap");
                                    }
                                } else {
                                     warn!(pool = %pool_address, "Snapshot disappeared before update after Velo/Aero swap");
                                }
                            },
                            Ok(Err(e)) => { error!(pool=%pool_address, dex=?dex_type, error=?e, "Fetch reserves RPC failed after swap"); },
                            Err(_) => { error!(pool=%pool_address, dex=?dex_type, timeout_secs = timeout_duration.as_secs(), "Timeout fetching reserves after swap"); }
                        }
                    }); // End spawned task
                }
                Err(e) => error!(pool=%contract_address, error=?e, "Failed to decode Velo/Aero Swap event"),
            }
        } // Ignore swaps from untracked pools
    }
    Ok(())
}


/// Checks for arbitrage opportunities involving the pool that was just updated.
#[instrument(skip(state, client, nonce_manager), fields(updated_pool=%updated_pool_address), level = "debug")]
async fn check_for_arbitrage(
    updated_pool_address: Address,
    state: Arc<AppState>,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    nonce_manager: Arc<NonceManager>,
) -> Result<()> {
    debug!("Checking for arbitrage opportunities triggered by pool update...");

    // 1. Get Snapshot of the updated pool
    let updated_pool_snapshot = match state.pool_snapshots.get(&updated_pool_address) {
        Some(entry) => entry.value().clone(), // Clone the snapshot data
        None => {
            warn!("Snapshot missing for updated pool {} during arbitrage check.", updated_pool_address);
            return Ok(()); // Cannot proceed without snapshot
        }
    };

    // 2. Sanity check: Ensure the updated pool involves the target pair
    if !state::is_target_pair_option(
        updated_pool_snapshot.token0,
        updated_pool_snapshot.token1,
        state.target_pair(),
    ) {
        trace!("Updated pool {} is not the target pair. Skipping arbitrage check.", updated_pool_address);
        return Ok(());
    }

    // 3. Find Potential Routes using the Path Optimizer
    debug!("Finding potential routes involving pool {}...", updated_pool_address);
    // find_top_routes operates on the hot cache (snapshots) and pool states for context
    let top_routes: Vec<RouteCandidate> = find_top_routes(
        &updated_pool_snapshot,
        &state.pool_states,      // Pass reference to detailed states map
        &state.pool_snapshots,   // Pass reference to snapshot map (hot cache)
        &state.config,           // Pass reference to config
        state.weth_address,      // Pass WETH address
        state.usdc_address,      // Pass USDC address
        state.weth_decimals,     // Pass WETH decimals
        state.usdc_decimals,     // Pass USDC decimals
    );

    if top_routes.is_empty() {
        trace!("No potential arbitrage routes found involving pool {}.", updated_pool_address);
        return Ok(());
    }

    info!(pool=%updated_pool_address, count=top_routes.len(), "Found potential routes!");

     // 4. Evaluate Top Route Candidates
     // Consider evaluating only the top N routes or based on estimated profit threshold
     for route_candidate in top_routes.into_iter().take(1) { // Example: Only evaluate the single most promising route
        info!(
            buy_pool = ?route_candidate.buy_pool_addr, buy_dex = ?route_candidate.buy_dex_type,
            sell_pool = ?route_candidate.sell_pool_addr, sell_dex = ?route_candidate.sell_dex_type,
            est_profit_pct = route_candidate.estimated_profit_usd, // Using placeholder field name
            "Evaluating Route Candidate..."
        );

        // Clone Arcs for the simulation task
        let sim_state = state.clone();
        let sim_client = client.clone();
        let sim_nonce_manager = nonce_manager.clone();
        let route = route_candidate.clone(); // Clone route for the spawn

        // Spawn a separate task for simulation and potential execution
        tokio::spawn(async move {
            // FIX E0382: Capture necessary fields before the move happens in submit_arbitrage_transaction
            let route_buy_addr = route.buy_pool_addr;
            let route_sell_addr = route.sell_pool_addr;

            debug!(buy_pool =?route_buy_addr, sell_pool =?route_sell_addr, "Spawning simulation task for route");

            // Get snapshots required for dynamic loan sizing
            let buy_snapshot_option = sim_state.pool_snapshots.get(&route_buy_addr).map(|r| r.value().clone());
            let sell_snapshot_option = sim_state.pool_snapshots.get(&route_sell_addr).map(|r| r.value().clone());

            // Fetch current gas price before simulation
            let gas_info = match crate::transaction::fetch_gas_price(sim_client.clone(), &sim_state.config).await {
                 Ok(g) => g,
                 Err(e) => {
                     // Use captured fields for logging
                     error!(buy_pool = ?route_buy_addr, sell_pool = ?route_sell_addr, error=?e, "Gas price fetch failed in simulation task. Aborting route evaluation.");
                     return; // Cannot simulate without gas price
                 }
            };
            let current_gas_price_gwei = gas_info.max_priority_fee_per_gas.to_f64_lossy() / 1e9;
            debug!(gas_price_gwei = current_gas_price_gwei, "Fetched gas price for simulation.");

            // Find the optimal loan amount and corresponding profit
             let optimal_loan_result = find_optimal_loan_amount(
                 sim_client.clone(),
                 sim_state.clone(),
                 &route, // Pass reference to the cloned route
                 buy_snapshot_option.as_ref(),
                 sell_snapshot_option.as_ref(),
                 current_gas_price_gwei,
             ).await;

            match optimal_loan_result {
                Ok(Some((optimal_loan_amount_wei, max_net_profit_wei))) => {
                    // Check if the maximum possible profit is positive
                    if max_net_profit_wei > I256::zero() {
                        info!(
                            // Use captured fields/cloned route for logging
                            buy_pool = ?route_buy_addr, sell_pool = ?route_sell_addr,
                            optimal_loan = %optimal_loan_amount_wei,
                            max_profit = %max_net_profit_wei,
                            "🎉 PROFITABLE OPPORTUNITY IDENTIFIED! Attempting execution."
                        );
                        // Attempt to submit the transaction
                         let execute_result = submit_arbitrage_transaction(
                             sim_client,
                             sim_state,
                             route, // Pass the owned route
                             optimal_loan_amount_wei,
                             max_net_profit_wei,
                             sim_nonce_manager,
                        ).await;

                         if let Err(e) = execute_result {
                              // Use captured fields for logging as route is now moved
                              error!(buy_pool = ?route_buy_addr, sell_pool = ?route_sell_addr, error = ?e,
                                     "Arbitrage execution attempt failed");
                         }
                         // If successful, submit_arbitrage_transaction logs success internally
                    } else {
                        // Use cloned route for logging
                        debug!(route = ?route, max_profit = %max_net_profit_wei, "Route evaluated, but max profit is not positive.");
                    }
                 }
                 Ok(None) => {
                    // Use cloned route for logging
                    debug!(route = ?route, "No profitable loan amount found for this route during optimization.");
                 }
                 Err(e) => {
                    // Use cloned route for logging
                    error!(route = ?route, error = ?e, "Optimal loan search failed for route");
                 }
            }
        }); // End simulation task spawn
    } // End loop through routes

    Ok(())
}