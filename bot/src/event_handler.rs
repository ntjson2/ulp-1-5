// bot/src/event_handler.rs

use std::sync::Arc;
use std::time::Duration; // Keep Duration if used by timeout
// Removed unused bindings import block
use crate::{
    config::Config, // Keep if used by other functions in this module not shown
    path_optimizer::{self, PathOptimizer, RouteCandidate},
    simulation, 
    state::{AppState, DexType, PoolSnapshot}, // PoolInfo removed as it's unresolved
    transaction,
    utils,
};
use ethers::{
    abi::RawLog, // Corrected path
    core::types::{Address, Filter, Log, H160, H256, U256, U64 as EthersU64}, // Added EthersU64
    providers::{Middleware, StreamExt, Provider, Http, Ws}, // Added Provider, Http, Ws
    middleware::SignerMiddleware, // Added
    signers::LocalWallet, // Added
};
use eyre::{eyre, Result, Report};
use tokio::time::timeout; // Keep timeout
use tracing::{debug, error, info, instrument, trace, warn};

pub const UNI_V3_POOL_CREATED_TOPIC: H256 = H256([
    0x96, 0x7c, 0x7e, 0x3f, 0x8b, 0x1f, 0x4e, 0x8c,
    0x9e, 0x77, 0x6c, 0x3f, 0x8b, 0x1f, 0x4e, 0x8c,
    0x9e, 0x77, 0x6c, 0x3f, 0x8b, 0x1f, 0x4e, 0x8c,
    0x9e, 0x77, 0x6c, 0x3f, 0x8b, 0x1f, 0x4e, 0x8c,
]);
pub const UNI_V3_SWAP_TOPIC: H256 = H256([
    0xd9, 0x4f, 0x3e, 0x2b, 0x8b, 0x1f, 0x4e, 0x8c,
    0x9e, 0x77, 0x6c, 0x3f, 0x8b, 0x1f, 0x4e, 0x8c,
    0x9e, 0x77, 0x6c, 0x3f, 0x8b, 0x1f, 0x4e, 0x8c,
    0x9e, 0x77, 0x6c, 0x3f, 0x8b, 0x1f, 0x4e, 0x8c,
]);
pub const VELO_AERO_POOL_CREATED_TOPIC: H256 = H256([
    0x96, 0x7c, 0x7e, 0x3f, 0x8b, 0x1f, 0x4e, 0x8c,
    0x9e, 0x77, 0x6c, 0x3f, 0x8b, 0x1f, 0x4e, 0x8c,
    0x9e, 0x77, 0x6c, 0x3f, 0x8b, 0x1f, 0x4e, 0x8c,
    0x9e, 0x77, 0x6c, 0x3f, 0x8b, 0x1f, 0x4e, 0x8c,
]);
pub const VELO_AERO_SWAP_TOPIC: H256 = H256([
    0xd9, 0x4f, 0x3e, 0x2b, 0x8b, 0x1f, 0x4e, 0x8c,
    0x9e, 0x77, 0x6c, 0x3f, 0x8b, 0x1f, 0x4e, 0x8c,
    0x9e, 0x77, 0x6c, 0x3f, 0x8b, 0x1f, 0x4e, 0x8c,
    0x9e, 0x77, 0x6c, 0x3f, 0x8b, 0x1f, 0x4e, 0x8c,
]);

// --- Event Handlers ---

pub async fn handle_new_block(block_number: EthersU64, _state: Arc<AppState>) -> Result<()> {
    info!("New block received: {}", block_number);
    // Potential logic: Trigger periodic checks, update non-event-driven state, etc.
    Ok(())
}

pub async fn handle_log_event(
    log: Log,
    app_state: Arc<AppState>,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, // Corrected client type
    path_optimizer: Arc<PathOptimizer>,
) -> Result<()> {
    let topic0 = log.topics.get(0).cloned();
    // let pool_address = log.address; // Already available as log.address

    if topic0 == UNI_V3_POOL_CREATED_TOPIC {
        match crate::bindings::i_uniswap_v3_factory::IUniswapV3FactoryPoolCreatedFilter::decode_log(&RawLog::from(log.clone())) {
            Ok(event) => {
                info!(pool = %event.pool, token0 = %event.token_0, token1 = %event.token_1, fee = %event.fee, "UniswapV3 PoolCreated Event");
                let s = app_state.clone();
                let c = client.clone();
                tokio::spawn(async move {
                    let fetch_result = crate::state::fetch_and_cache_pool_state(event.pool, DexType::UniswapV3, log.address, c, s).await;
                    if let Err(e) = fetch_result {
                        error!("Error fetching/caching UniV3 pool state: {:?}", e);
                    }
                });
            }
            Err(e) => warn!("Failed to decode UniV3 PoolCreated Log: {:?}", e),
        }
    } else if topic0 == VELO_AERO_POOL_CREATED_TOPIC {
        match crate::bindings::i_velodrome_factory::IVelodromeFactoryPoolCreatedFilter::decode_log(&RawLog::from(log.clone())) { 
            Ok(event) => {
                let factory_addr = log.address;
                let dex_type = if Some(factory_addr) == app_state.config.velodrome_v2_factory_addr {
                    DexType::VelodromeV2
                } else if Some(factory_addr) == app_state.config.aerodrome_factory_addr {
                    DexType::Aerodrome
                } else {
                    warn!("PoolCreated event from unknown factory: {}", factory_addr);
                    return Ok(());
                };
                // Access event fields correctly (e.g., token_0, token_1, stable if those are the generated names)
                info!(pool = %event.pool, token0 = %event.token_0, token1 = %event.token_1, stable = %event.stable, "{:?} PoolCreated Event", dex_type);
                let s=app_state.clone();
                let c=client.clone();
                tokio::spawn(async move {
                    let fetch_result = crate::state::fetch_and_cache_pool_state(event.pool, dex_type, log.address, c, s).await;
                     if let Err(e) = fetch_result {
                        error!("Error fetching/caching {:?} pool state: {:?}", dex_type, e);
                    }
                });
            }
            Err(e) => warn!("Failed to decode Velo/Aero PoolCreated Log: {:?}", e),
        }
    } else if topic0 == UNI_V3_SWAP_TOPIC {
        match crate::bindings::i_uniswap_v3_pool::IUniswapV3PoolSwapFilter::decode_log(&RawLog::from(log.clone())) {
            Ok(event) => {
                trace!(pool = %log.address, sender = %event.sender, recipient = %event.recipient, "UniV3 Swap Event");
                // Update snapshot (simplified, ideally get block number with event)
                let s = app_state.clone();
                let c = client.clone();
                tokio::spawn(async move {
                    if let Err(e) = crate::state::update_pool_snapshot(log.address, DexType::UniswapV3, c.clone(), s.clone()).await {
                        warn!("Failed to update snapshot for UniV3 pool {}: {:?}", log.address, e);
                    }
                    if let Err(e) = check_for_arbitrage(log.address, s, c, log.address).await {
                        error!(pool=%log.address, error=?e, "Check arbitrage task failed after UniV3 swap");
                    }
                });
            }
            Err(e) => warn!("Failed to decode UniV3 Swap Log: {:?}", e),
        }
    } else if topic0 == VELO_AERO_SWAP_TOPIC {
        let pool_address = log.address;
        if let Some(pool_snapshot_ref) = app_state.pool_snapshots.get(&log.address) {
            let pool_snapshot = pool_snapshot_ref.value().clone(); // Clone the snapshot for use
            // ... rest of the logic using pool_snapshot ...
            if app_state.known_pools.contains_key(&log.address) { // Check if pool is known
                let check_task = check_for_arbitrage(
                    app_state.clone(),
                    client.clone(), // HTTP client for simulation
                    path_optimizer.clone(),
                    log.address, // pool_address that had the event
                    pool_snapshot, // Pass the cloned snapshot
                );
                tokio::spawn(check_task);
            }
        }
    } else if topic0 == VELO_V2_SWAP_TOPIC || topic0 == AERO_SWAP_TOPIC {
        let dex_type = if topic0 == VELO_V2_SWAP_TOPIC { DexType::VelodromeV2 } else { DexType::Aerodrome };
        let pool_address = log.address;

        let decoded_event_result: Result<String> = match dex_type {
            DexType::VelodromeV2 => {
                match crate::bindings::velodrome_v2_pool::VelodromeV2PoolSwapFilter::decode_log(&RawLog::from(log.clone())) {
                    Ok(event) => {
                        trace!(pool = %log.address, sender = %event.sender, recipient = %event.recipient, "Velodrome V2 Swap Event");
                        // Update snapshot (simplified, ideally get block number with event)
                        let s = app_state.clone();
                        let c = client.clone();
                        tokio::spawn(async move {
                            if let Err(e) = crate::state::update_pool_snapshot(log.address, DexType::VelodromeV2, c.clone(), s.clone()).await {
                                warn!("Failed to update snapshot for Velodrome V2 pool {}: {:?}", log.address, e);
                            }
                            if let Err(e) = check_for_arbitrage(log.address, s, c, log.address).await {
                                error!(pool=%log.address, error=?e, "Check arbitrage task failed after Velodrome V2 swap");
                            }
                        });
                        Ok("Velodrome V2 event processed".into())
                    }
                    Err(e) => {
                        warn!("Failed to decode Velodrome V2 Swap Log: {:?}", e);
                        Err(eyre!("Failed to decode Velodrome V2 Swap Log"))
                    }
                }
            }
            DexType::Aerodrome => {
                 match crate::bindings::aerodrome_pool::AerodromePoolSwapFilter::decode_log(&RawLog::from(log.clone())) {
                    Ok(event) => {
                        trace!(pool = %log.address, sender = %event.sender, recipient = %event.recipient, "Aerodrome Swap Event");
                        // Update snapshot (simplified, ideally get block number with event)
                        let s = app_state.clone();
                        let c = client.clone();
                        tokio::spawn(async move {
                            if let Err(e) = crate::state::update_pool_snapshot(log.address, DexType::Aerodrome, c.clone(), s.clone()).await {
                                warn!("Failed to update snapshot for Aerodrome pool {}: {:?}", log.address, e);
                            }
                            if let Err(e) = check_for_arbitrage(log.address, s, c, log.address).await {
                                error!(pool=%log.address, error=?e, "Check arbitrage task failed after Aerodrome swap");
                            }
                        });
                        Ok("Aerodrome event processed".into())
                    }
                    Err(e) => {
                        warn!("Failed to decode Aerodrome Swap Log: {:?}", e);
                        Err(eyre!("Failed to decode Aerodrome Swap Log"))
                    }
                }
            }
        } {
            Ok(_) => (),
            Err(_) => (),
        }
    } else {
        trace!("Log with unhandled topic0: {:?}", topic0);
    }

    Ok(())
}


#[instrument(skip_all, fields(block_number = %block_number.as_u64()))]
async fn process_block_for_arbitrage(
    block_number: EthersU64, 
    state: Arc<AppState>,    
    client_http: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, 
) -> Result<()> {
    let s = state.clone(); // s is AppState
    let config_ref = s.config.clone();

    info!(
        block = block_number.as_u64(),
        "Processing block for arbitrage opportunities."
    );
    
    for entry in s.pool_states.iter() { // Iterate correctly over DashMap
        let pool_address = *entry.key();
        let pool_state_val = entry.value();
        let dex_type = pool_state_val.dex_type;
        let fetch_client_http_clone = client_http.clone(); // Use the passed client_http
        
        let timeout_duration = Duration::from_secs(s.config.fetch_timeout_secs.unwrap_or(10)); // s.config is correct
        
        match tokio::time::timeout(timeout_duration, crate::state::update_pool_snapshot(pool_address, dex_type, fetch_client_http_clone, s.clone())).await {
            Ok(Ok(_)) => {
                trace!("Successfully updated snapshot for pool {}", pool_address);
            }
            Ok(Err(e)) => {
                warn!("Failed to update snapshot for pool {}: {:?}", pool_address, e);
            }
            Err(_) => {
                warn!("Timeout updating snapshot for pool {}", pool_address);
            }
        }
    }

    let weth_decimals = config_ref.weth_decimals.expect("WETH decimals not configured for find_top_routes");
    let usdc_decimals = config_ref.usdc_decimals.expect("USDC decimals not configured for find_top_routes");

    let representative_snapshot_ref_option = s.pool_snapshots.iter().next().map(|entry| entry.value().clone());
    
    if let Some(representative_snapshot_ref) = representative_snapshot_ref_option {
        let top_routes: Vec<RouteCandidate> = path_optimizer::find_top_routes(
            &representative_snapshot_ref,
            &s.pool_states,
            &s.pool_snapshots,
            &config_ref,
            config_ref.weth_address,
            config_ref.usdc_address,
            weth_decimals,  
            usdc_decimals,  
        );
        if top_routes.is_empty() {
            info!(block_number = block_number.as_u64(), "No arbitrage routes found in this block.");
        } else {
            info!(block_number = block_number.as_u64(), "Found {} potential arbitrage routes.", top_routes.len()); // Log block_number.as_u64()
            for route_candidate in top_routes {
                debug!(route_id = %route_candidate.id(), "Potential route found");
                // Further processing: detailed simulation, profit calculation, and transaction submission
                // This would involve calling functions from simulation.rs and transaction.rs
                // Example:
                // match crate::simulation::find_optimal_loan_amount(s.clone(), client_http.clone(), &route_candidate, config_ref.clone()).await {
                //     Ok((optimal_loan, estimated_profit)) => {
                //         if estimated_profit > I256::zero() { // Or some threshold
                //             info!(route_id = %route_candidate.id(), loan = %optimal_loan, profit = %estimated_profit, "Profitable trade identified, proceeding to submission.");
                //             // Construct and submit transaction
                //             // crate::transaction::submit_arbitrage_transaction(...).await;
                //         } else {
                //             debug!(route_id = %route_candidate.id(), "Route not profitable after simulation.");
                //         }
                //     }
                //     Err(e) => {
                //         warn!(route_id = %route_candidate.id(), "Error during simulation: {:?}", e);
                //     }
                // }
            }
        }
    } else {
        warn!("No pool snapshots available to provide to find_top_routes. Skipping arbitrage search for block {}.", block_number.as_u64()); // Log block_number.as_u64()
    }

    Ok(())
}


// Correct skip parameters to match actual function parameter names if they start with underscore
#[instrument(skip(state, _http_client, _nonce_manager_ref), fields(updated_pool=%updated_pool_address), level = "debug")]
pub async fn check_for_arbitrage(
    updated_pool_address: Address,
    state: Arc<AppState>, 
    _http_client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, 
    _nonce_manager_ref: Arc<NonceManager>, 
) -> Result<()> {
    info!(pool = %updated_pool_address, "Checking for arbitrage opportunities involving this pool...");

    let app_state_ref = state.as_ref(); // app_state_ref is &AppState
    let config_ref = &app_state_ref.config;

    let updated_pool_snapshot = match app_state_ref.pool_snapshots.get(&updated_pool_address) {
        Some(snapshot_ref) => snapshot_ref.value().clone(),
        None => {
            warn!(pool = %updated_pool_address, "Snapshot not found for updated pool in check_for_arbitrage. Skipping.");
            return Ok(());
        }
    };

    let weth_decimals = config_ref.weth_decimals.expect("WETH decimals not configured in check_for_arbitrage");
    let usdc_decimals = config_ref.usdc_decimals.expect("USDC decimals not configured in check_for_arbitrage");

    let top_routes: Vec<RouteCandidate> = path_optimizer::find_top_routes(
        &updated_pool_snapshot,    
        &state.pool_states,        
        &state.pool_snapshots,     
        config_ref,                
        config_ref.weth_address,   
        config_ref.usdc_address,   
        weth_decimals,  // Pass u8
        usdc_decimals,  // Pass u8
    );

    if top_routes.is_empty() {
        trace!("No promising routes found from pool update: {}", updated_pool_address);
        return Ok(());
    }
    info!("Found {} promising routes from pool update: {}", top_routes.len(), updated_pool_address);

    #[cfg(feature = "local_simulation")]
    {
        let mut triggered_guard = state.test_arb_check_triggered.lock().expect("Failed to lock test_arb_check_triggered");
        if !*triggered_guard {
            if !top_routes.is_empty() { 
                info!("LOCAL_SIMULATION: Arbitrage routes found, setting test_arb_check_triggered to true.");
                *triggered_guard = true; 
            } else {
                info!("LOCAL_SIMULATION: No arbitrage routes found for updated pool {}.", updated_pool_address);
            }
        } else {
            info!("LOCAL_SIMULATION: test_arb_check_triggered was already true.");
        }
    }
    Ok(())
}

// Removed duplicate handle_swap_event function
// The logic is now consolidated in the main handle_log_event function.

pub async fn listen_for_events(
    app_state: Arc<AppState>,
    ws_client: Arc<SignerMiddleware<Provider<Ws>, LocalWallet>>,
    http_client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, // Corrected type for _http_client
    path_optimizer: Arc<PathOptimizer>,
) -> Result<()> {
    // ...existing code...
}