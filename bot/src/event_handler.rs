// bot/src/event_handler.rs

use crate::bindings::{
    AerodromePool,
    VelodromeV2Pool,
    uniswap_v3_pool::SwapFilter as UniV3SwapFilter,
    i_uniswap_v3_factory::PoolCreatedFilter as UniV3PoolCreatedFilter,
    i_velodrome_factory::PoolCreatedFilter as VeloPoolCreatedFilter,
};
use crate::state::{self, AppState, DexType}; // Removed unused PoolSnapshot import
use crate::path_optimizer::{self, RouteCandidate};
use crate::{
    UNI_V3_POOL_CREATED_TOPIC, UNI_V3_SWAP_TOPIC, VELO_AERO_POOL_CREATED_TOPIC,
    VELO_AERO_SWAP_TOPIC,
};
use crate::transaction::NonceManager;

use ethers::{
    abi::RawLog,
    contract::{EthLogDecode, ContractCall},
    core::types::{Address, Log, U256, U64},
    middleware::SignerMiddleware,
    providers::{Http, Provider}, // Removed unused Middleware import
    signers::LocalWallet,
};
use eyre::Result;
use std::sync::Arc;
use tokio::time::{timeout, Duration};
use tracing::{debug, error, info, instrument, trace, warn};

// --- Event Handlers ---

pub async fn handle_new_block(block_number: U64, _state: Arc<AppState>) -> Result<()> {
    info!("🧱 New Block Received: #{}", block_number);
    Ok(())
}

/// Processes individual log events. Updates hot-cache, triggers checks.
#[instrument(skip_all, fields(tx_hash = ?log.transaction_hash, block = ?log.block_number))]
pub async fn handle_log_event(
    log: Log,
    state: Arc<AppState>,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    nonce_manager: Arc<NonceManager>,
) -> Result<()> {
    let topics = &log.topics;
    if topics.is_empty() {
        warn!("Log received with no topics: {:?}", log);
        return Ok(());
    }

    let topic0 = topics[0];

    if topic0 == UNI_V3_POOL_CREATED_TOPIC {
        if log.address != state.config.uniswap_v3_factory_addr {
            trace!("Ignoring UniV3 PoolCreated log from non-factory address: {}", log.address);
            return Ok(());
        }
        let raw_log: RawLog = log.clone().into();
        match <UniV3PoolCreatedFilter as EthLogDecode>::decode_log(&raw_log) {
            Ok(event) => {
                if state::is_target_pair_option(event.token_0, event.token_1, state.target_pair()) {
                    info!(pool=%event.pool, fee=%event.fee, "✨ Target UniV3 pool created! Fetching state...");
                    let s = state.clone();
                    let c = client.clone();
                    tokio::spawn(async move {
                         let fetch_result = state::fetch_and_cache_pool_state(event.pool, DexType::UniswapV3, log.address, c, s).await;
                         if let Err(e) = fetch_result {
                              error!(pool=%event.pool, factory=%log.address, error=?e, "Fetch state failed for new UniV3 pool");
                          }
                    });
                } else {
                    trace!(pool=%event.pool, "Ignoring non-target pair UniV3 pool creation.");
                }
            }
            Err(e) => error!(address=%log.address, error=?e, "Failed to decode UniV3 PoolCreated event"),
        }
    } else if topic0 == VELO_AERO_POOL_CREATED_TOPIC {
        let dex_type = if log.address == state.config.velodrome_v2_factory_addr {
            DexType::VelodromeV2
        } else if Some(log.address) == state.config.aerodrome_factory_addr {
            DexType::Aerodrome
        } else {
            trace!("Ignoring Velo/Aero PoolCreated log from non-factory address: {}", log.address);
            return Ok(());
        };

        let raw_log: RawLog = log.clone().into();
        match <VeloPoolCreatedFilter as EthLogDecode>::decode_log(&raw_log) {
             Ok(event) => {
                 if state::is_target_pair_option(event.token_0, event.token_1, state.target_pair()) {
                    info!(pool=%event.pool, dex=?dex_type, stable=%event.stable, "✨ Target {:?} pool created! Fetching state...", dex_type);
                     let s=state.clone();
                     let c=client.clone();
                     tokio::spawn(async move {
                         let fetch_result = state::fetch_and_cache_pool_state(event.pool, dex_type, log.address, c, s).await;
                         if let Err(e) = fetch_result {
                              error!(pool=%event.pool, factory=%log.address, dex=?dex_type, error=?e, "Fetch state failed for new Velo/Aero pool");
                          }
                     });
                 } else {
                    trace!(pool=%event.pool, "Ignoring non-target pair Velo/Aero pool creation.");
                 }
             }
             Err(e) => error!(address=%log.address, error=?e, "Failed to decode Velo/Aero PoolCreated event"),
         }

    } else if topic0 == UNI_V3_SWAP_TOPIC {
        if let Some(mut snapshot_entry) = state.pool_snapshots.get_mut(&log.address) {
            trace!(pool=%log.address, "Handling UniV3 Swap");
            let raw_log: RawLog = log.clone().into();
            match <UniV3SwapFilter as EthLogDecode>::decode_log(&raw_log) {
                Ok(swap) => {
                    let block_number_u64_opt = log.block_number;
                    snapshot_entry.sqrt_price_x96 = Some(swap.sqrt_price_x96);
                    snapshot_entry.tick = Some(swap.tick);
                    snapshot_entry.last_update_block = block_number_u64_opt.map(|val| U256::from(val.as_u64()));
                    debug!(pool=%log.address, tick=%swap.tick, "UniV3 Snapshot Updated from Swap event");

                    let s = state.clone();
                    let c = client.clone();
                    let nm = nonce_manager.clone();
                    tokio::spawn(async move {
                        if let Err(e) = check_for_arbitrage(log.address, s, c, nm).await {
                            error!(pool=%log.address, error=?e, "Check arbitrage task failed after UniV3 swap");
                        }
                    });
                }
                Err(e) => error!(pool=%log.address, error=?e, "Failed to decode UniV3 Swap event"),
            }
        }

    } else if topic0 == VELO_AERO_SWAP_TOPIC {
        if let Some(snapshot_entry) = state.pool_snapshots.get(&log.address) {
             let dex_type = snapshot_entry.dex_type;
             let pool_address = *snapshot_entry.key();
             drop(snapshot_entry);

             trace!(pool=%pool_address, dex=?dex_type, "Handling {:?} Swap", dex_type);
             let block_number_u64_opt = log.block_number;
             let s = state.clone();
             let c = client.clone();
             let nm = nonce_manager.clone();
             tokio::spawn(async move {
                 debug!(pool=%pool_address, dex=?dex_type, "Fetching reserves after swap...");
                 let timeout_duration = Duration::from_secs(s.config.fetch_timeout_secs.unwrap_or(10));

                 // The error indicates the contract returns (U256, U256, U256)
                 // Adjust ReservesType to match the actual return type of get_reserves.
                 // If the third U256 is indeed blockTimestampLast, it's fine.
                 // If it's something else, the variable name _ts might be misleading.
                 type ReservesType = (U256, U256, U256); // Changed u32 to U256 based on E0308
                 let pool_call_binding: ContractCall<SignerMiddleware<Provider<Http>, LocalWallet>, ReservesType> = if dex_type == DexType::VelodromeV2 {
                     let pool = VelodromeV2Pool::new(pool_address, c.clone());
                     pool.get_reserves()
                 } else {
                     let pool = AerodromePool::new(pool_address, c.clone());
                     pool.get_reserves()
                 };

                 let pool_call_future = pool_call_binding.call();

                 match timeout(timeout_duration, pool_call_future).await {
                    Ok(Ok(reserves)) => {
                        let (reserve0, reserve1, _ts): ReservesType = reserves;
                        if let Some(mut snapshot) = s.pool_snapshots.get_mut(&pool_address) {
                            snapshot.reserve0 = Some(reserve0);
                            snapshot.reserve1 = Some(reserve1);
                            snapshot.last_update_block = block_number_u64_opt.map(|val| U256::from(val.as_u64()));
                            debug!(pool=%pool_address, dex=?dex_type, r0=%reserve0, r1=%reserve1, "Velo/Aero Snapshot Updated after Swap");

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
            });
        }
    } else {
        trace!("Log with unhandled topic0: {:?}", topic0);
    }

    Ok(())
}


async fn handle_pool_created_event(
    log: Log,
    state: Arc<AppState>,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, // Ensure this is 'client', not '_client'
    dex_type: DexType,
    factory_addr: Address,
) -> Result<()> {
    let raw_log: RawLog = log.clone().into();
    match dex_type {
        DexType::UniswapV3 => {
            match <UniV3PoolCreatedFilter as EthLogDecode>::decode_log(&raw_log) {
                Ok(event) => {
                    if state::is_target_pair_option(event.token_0, event.token_1, state.target_pair()) {
                        info!(pool=%event.pool, fee=%event.fee, "✨ Target UniV3 pool created! Fetching state...");
                        let s = state.clone();
                        let c = client.clone(); // Uses the 'client' parameter
                        tokio::spawn(async move {
                             let fetch_result = state::fetch_and_cache_pool_state(event.pool, DexType::UniswapV3, factory_addr, c, s).await;
                             if let Err(e) = fetch_result {
                                  error!(pool=%event.pool, factory=%factory_addr, error=?e, "Fetch state failed for new UniV3 pool");
                              }
                        });
                    } else {
                        trace!(pool=%event.pool, "Ignoring non-target pair UniV3 pool creation.");
                    }
                }
                Err(e) => error!(address=%factory_addr, error=?e, "Failed to decode UniV3 PoolCreated event"),
            };
            Ok(())
        },
        DexType::VelodromeV2 | DexType::Aerodrome => {
            match <VeloPoolCreatedFilter as EthLogDecode>::decode_log(&raw_log) {
                 Ok(event) => {
                     if state::is_target_pair_option(event.token_0, event.token_1, state.target_pair()) {
                        info!(pool=%event.pool, dex=?dex_type, stable=%event.stable, "✨ Target {:?} pool created! Fetching state...", dex_type);
                         let s=state.clone();
                         let c=client.clone(); // Uses the 'client' parameter
                         tokio::spawn(async move {
                             let fetch_result = state::fetch_and_cache_pool_state(event.pool, dex_type, factory_addr, c, s).await;
                             if let Err(e) = fetch_result {
                                  error!(pool=%event.pool, factory=%factory_addr, dex=?dex_type, error=?e, "Fetch state failed for new Velo/Aero pool");
                              }
                         });
                     } else {
                        trace!(pool=%event.pool, "Ignoring non-target pair Velo/Aero pool creation.");
                     }
                 }
                 Err(e) => error!(address=%factory_addr, error=?e, "Failed to decode Velo/Aero PoolCreated event"),
             };
            Ok(())
        },
        DexType::Unknown => {
            warn!("handle_pool_created_event called with Unknown DexType for factory: {}", factory_addr);
            Ok(())
        }
    }
}

/// Checks for arbitrage opportunities involving the pool that was just updated.
// The skip list should match the parameter names exactly.
// If parameters are _client and _nonce_manager, then skip(_client, _nonce_manager) is correct.
// The error message might be showing a canonicalized name. Assuming previous fix was correct.
#[instrument(skip(state, _client, _nonce_manager), fields(updated_pool=%updated_pool_address), level = "debug")]
pub async fn check_for_arbitrage(
    updated_pool_address: Address,
    state: Arc<AppState>,
    _client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, 
    _nonce_manager: Arc<NonceManager>, 
) -> Result<()> {
    info!(pool = %updated_pool_address, "Checking for arbitrage opportunities involving this pool...");

    let app_state_ref = state.as_ref();
    let config_ref = &app_state_ref.config;

    let updated_pool_snapshot = match app_state_ref.pool_snapshots.get(&updated_pool_address) {
        Some(snapshot_ref) => snapshot_ref.value().clone(),
        None => {
            warn!(pool = %updated_pool_address, "Snapshot not found for updated pool in check_for_arbitrage. Skipping.");
            return Ok(());
        }
    };

    let top_routes: Vec<RouteCandidate> = path_optimizer::find_top_routes(
        &updated_pool_snapshot,    // Arg 1: &PoolSnapshot
        &state.pool_states,        // Arg 2: &Arc<DashMap<Address, PoolState>>
        &state.pool_snapshots,     // Arg 3: &Arc<DashMap<Address, PoolSnapshot>>
        config_ref,                // Arg 4: &Config
        config_ref.weth_address,   // Arg 5: Address
        config_ref.usdc_address,   // Arg 6: Address
        config_ref.weth_decimals,  // Arg 7: u8
        config_ref.usdc_decimals,  // Arg 8: u8
    );

    if top_routes.is_empty() {
        trace!("No promising routes found from pool update: {}", updated_pool_address);
        return Ok(());
    }
    info!("Found {} promising routes from pool update: {}", top_routes.len(), updated_pool_address);

    #[cfg(feature = "local_simulation")]
    {
        if state.test_arb_check_triggered {
            info!("LOCAL_SIMULATION: test_arb_check_triggered was already true, not modifying.");
        } else {
            info!("LOCAL_SIMULATION: check_for_arbitrage was called. If routes were found and profitable, a test might set test_arb_check_triggered.");
        }
    }
    Ok(())
}

// Removed duplicate handle_swap_event function
// The logic is now consolidated in the main handle_log_event function.