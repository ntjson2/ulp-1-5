// bot/src/event_handler.rs

use crate::bindings::{ // Keep event filters and needed Pool contract types
    i_uniswap_v3_factory::PoolCreatedFilter as UniV3PoolCreatedFilter,
    i_velodrome_factory::PoolCreatedFilter as VeloAeroPoolCreatedFilter, // Renamed for clarity
    uniswap_v3_pool::SwapFilter as UniV3SwapFilter,
    velodrome_v2_pool::SwapFilter as VeloAeroSwapFilter, // Renamed for clarity
    AerodromePool, // Keep specific pool types if needed for direct calls (reserve fetch)
    VelodromeV2Pool,
    // REMOVED: VelodromeRouter, QuoterV2 (will be initialized in simulation.rs)
};
use crate::state::{self, AppState, DexType, PoolSnapshot}; // Use state module
use crate::path_optimizer::find_top_routes;
use crate::simulation::find_optimal_loan_amount;
use crate::{ // Use updated consolidated topic names from main.rs
    UNI_V3_POOL_CREATED_TOPIC, UNI_V3_SWAP_TOPIC, VELO_AERO_POOL_CREATED_TOPIC,
    VELO_AERO_SWAP_TOPIC,
};
use crate::transaction::{submit_arbitrage_transaction, NonceManager}; // Keep NonceManager

use ethers::{
    abi::RawLog,
    contract::EthLogDecode,
    prelude::*,
    types::{Log, U64, I256, U256},
};
use eyre::Result;
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tokio::time::timeout;
use tracing::{debug, error, info, instrument, trace, warn};

// --- Event Handlers ---

pub async fn handle_new_block(block_number: U64, _state: Arc<AppState>) -> Result<()> {
    info!("🧱 New Block Received: #{}", block_number);
    Ok(())
}

/// Processes individual log events. Updates hot-cache, triggers checks.
/// Removes direct passing of Router/Quoter instances.
#[instrument(skip_all, fields(tx_hash = ?log.transaction_hash, block = ?log.block_number, address = ?log.address))]
pub async fn handle_log_event(
    log: Log,
    state: Arc<AppState>,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    nonce_manager: Arc<NonceManager>, // Keep NonceManager
    // REMOVED router/quoter instances from signature
) -> Result<()> {
    let event_sig = match log.topics.get(0) { Some(t)=>*t, None=>{warn!("Log missing topic0"); return Ok(());} };
    let contract_address = log.address;

    // --- Pool Creation Events ---
    if event_sig == *UNI_V3_POOL_CREATED_TOPIC {
        if contract_address != state.config.uniswap_v3_factory_addr { trace!("Ignore UniV3 PoolCreated from non-factory"); return Ok(()); }
        let raw_log: RawLog = log.clone().into();
        match UniV3PoolCreatedFilter::decode_log(&raw_log) {
            Ok(event) => {
                if state::is_target_pair_option(event.token_0, event.token_1, state.target_pair()) {
                    info!(pool=%event.pool, "✨ Target UniV3 pool found! Fetching state...");
                    let s = state.clone(); let c = client.clone();
                    tokio::spawn(async move { let _ = state::fetch_and_cache_pool_state(event.pool, DexType::UniswapV3, c, s).await.map_err(|e| error!(pool=%event.pool, error=?e,"Fetch state failed")); });
                }
            }
            Err(e) => error!(address=%contract_address, error=?e, "Decode UniV3 PoolCreated failed"),
        }
    } else if event_sig == *VELO_AERO_POOL_CREATED_TOPIC {
        let dex_type = if Some(contract_address) == state.config.velodrome_v2_factory_addr { DexType::VelodromeV2 }
                       else if Some(contract_address) == state.config.aerodrome_factory_addr { DexType::Aerodrome }
                       else { trace!("Ignore Velo/Aero PoolCreated from non-factory"); return Ok(()); };
        let raw_log: RawLog = log.clone().into();
        match VeloAeroPoolCreatedFilter::decode_log(&raw_log) {
             Ok(event) => {
                 if state::is_target_pair_option(event.token_0, event.token_1, state.target_pair()) {
                    info!(pool=%event.pool, dex=?dex_type, "✨ Target {:?} pool found! Fetching state...", dex_type);
                     let s=state.clone(); let c=client.clone();
                     tokio::spawn(async move { let _ = state::fetch_and_cache_pool_state( event.pool, dex_type, c, s ).await.map_err(|e| error!(pool=%event.pool, error=?e,"Fetch state failed")); });
                 }
             }
             Err(e) => error!(address=%contract_address, error=?e, "Decode Velo/Aero PoolCreated failed"),
         }

    // --- Swap Events ---
    } else if event_sig == *UNI_V3_SWAP_TOPIC {
        if state.pool_snapshots.contains_key(&contract_address) {
            trace!(pool=%contract_address, "Handling UniV3 Swap");
            let raw_log: RawLog = log.clone().into();
            match UniV3SwapFilter::decode_log(&raw_log) {
                Ok(swap) => {
                    let block_number = log.block_number;
                    let update_result = state.pool_snapshots.entry(contract_address)
                        .and_modify(|snapshot| {
                            snapshot.sqrt_price_x96 = Some(U256::from(swap.sqrt_price_x96));
                            snapshot.tick = Some(swap.tick); snapshot.last_update_block = block_number;
                        });
                    if update_result.exists() {
                        debug!(pool=%update_result.key(), "UniV3 Snapshot Updated");
                        // Pass only needed Arcs to check task
                        let s = state.clone(); let c = client.clone(); let nm = nonce_manager.clone();
                        tokio::spawn(async move {
                            if let Err(e) = check_for_arbitrage(contract_address, s, c, nm).await {
                                error!(pool=%contract_address, error=?e, "Check arbitrage failed");
                            }
                        });
                    }
                }
                Err(e) => error!(pool=%contract_address, error=?e, "Decode UniV3 Swap failed"),
            }
        } // ignore untracked

    } else if event_sig == *VELO_AERO_SWAP_TOPIC {
        if let Some(mut snapshot_entry) = state.pool_snapshots.get_mut(&contract_address) {
             let dex_type = snapshot_entry.dex_type.clone();
             trace!(pool=%contract_address, dex=?dex_type, "Handling {:?} Swap", dex_type);
             let raw_log: RawLog = log.clone().into();
             match VeloAeroSwapFilter::decode_log(&raw_log) {
                 Ok(_swap_data) => {
                     let block_number = log.block_number;
                     let s = state.clone(); let c = client.clone(); let nm = nonce_manager.clone();
                     tokio::spawn(async move {
                         debug!(pool=%contract_address, dex=?dex_type, "Fetching reserves after swap...");
                         let pool_result: Result<_, _> = if dex_type == DexType::VelodromeV2 {
                              VelodromeV2Pool::new(contract_address, c.clone()).get_reserves().call().await
                         } else { // Aero
                              AerodromePool::new(contract_address, c.clone()).get_reserves().call().await
                         };
                         let timeout_duration = Duration::from_secs(s.config.fetch_timeout_secs.unwrap_or(10));
                         match timeout(timeout_duration, pool_result).await {
                            Ok(Ok(reserves)) => {
                                let (reserve0, reserve1, _) = reserves;
                                let update_result = s.pool_snapshots.entry(contract_address)
                                    .and_modify(|snapshot| {
                                        snapshot.reserve0 = Some(reserve0); snapshot.reserve1 = Some(reserve1);
                                        snapshot.last_update_block = block_number;
                                    });
                                if update_result.exists() {
                                    debug!(pool=%update_result.key(), dex=?dex_type, "Snapshot Updated");
                                    // Trigger check after update
                                    if let Err(e) = check_for_arbitrage(contract_address, s, c, nm).await {
                                        error!(pool=%contract_address, error=?e, "Check arbitrage failed");
                                    }
                                }
                            },
                            Ok(Err(e)) => { error!(pool=%contract_address, dex=?dex_type, error=?e, "Fetch reserves RPC failed"); },
                            Err(_) => { error!(pool=%contract_address, dex=?dex_type, timeout_secs = timeout_duration.as_secs(), "Timeout fetching reserves"); }
                        }
                    }); // End spawned task
                }
                Err(e) => error!(pool=%contract_address, error=?e, "Decode Velo/Aero Swap failed"),
            }
        } // ignore untracked
    }
    Ok(())
}


/// Checks for arbitrage opportunities involving the pool that was just updated.
/// Removed Router/Quoter instance parameters.
#[instrument(skip(state, client, nonce_manager), fields(updated_pool=%updated_pool_address), level = "debug")]
async fn check_for_arbitrage(
    updated_pool_address: Address,
    state: Arc<AppState>,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    nonce_manager: Arc<NonceManager>, // Keep NonceManager
    // REMOVED router/quoter instances
) -> Result<()> {
    debug!("Checking for arbitrage opportunities...");
    let updated_pool_snapshot = match state.pool_snapshots.get(&updated_pool_address) { Some(e)=>e.value().clone(), None=>{warn!("Snapshot missing");return Ok(());}};
    let updated_pool_state_details = match state.pool_states.get(&updated_pool_address) { Some(e)=>e.value().clone(), None=>{warn!("State missing"); return Ok(());}};
    if !state::is_target_pair_option( updated_pool_snapshot.token0, updated_pool_snapshot.token1, state.target_pair(), ) { return Ok(()); }

    debug!("Finding potential routes...");
    let top_routes = find_top_routes( &updated_pool_snapshot, &state.pool_states, &state.pool_snapshots, &state.config, state.weth_address, state.usdc_address, state.weth_decimals, state.usdc_decimals );
    if top_routes.is_empty() { return Ok(()); }
    info!(pool=%updated_pool_address, count=top_routes.len(), "Found potential routes!");

     for route in top_routes {
        info!( buy_pool =?route.buy_pool_addr, sell_pool =?route.sell_pool_addr, "Evaluating Route Candidate" );
        let sim_state = state.clone(); let sim_client = client.clone(); let sim_nonce_manager = nonce_manager.clone();
        // REMOVED cloning of router/quoter instances

        tokio::spawn(async move {
            debug!(buy_pool =?route.buy_pool_addr, sell_pool =?route.sell_pool_addr, "Spawning simulation task");
            let buy_snapshot_option = sim_state.pool_snapshots.get(&route.buy_pool_addr).map(|r| r.value().clone());
            let sell_snapshot_option = sim_state.pool_snapshots.get(&route.sell_pool_addr).map(|r| r.value().clone());
            if buy_snapshot_option.is_none() || sell_snapshot_option.is_none() { warn!("Snapshot missing"); return; }
            let buy_snapshot_ref = buy_snapshot_option.as_ref(); let sell_snapshot_ref = sell_snapshot_option.as_ref();

            let gas_info = match crate::transaction::fetch_gas_price(sim_client.clone(), &sim_state.config).await { Ok(g) => g, Err(_) => { return; } };
            let current_gas_price_gwei = gas_info.max_priority_fee_per_gas.to_f64_lossy() / 1e9;

            // Call find_optimal_loan_amount with simplified signature
             let optimal_loan_result = find_optimal_loan_amount(
                 sim_client.clone(),
                 sim_state.clone(), // Pass AppState Arc
                 &route,            // Pass RouteCandidate ref
                 buy_snapshot_ref,  // Pass snapshot Option refs
                 sell_snapshot_ref,
                 current_gas_price_gwei,
                 // REMOVED router/quoter args
             ).await;

            match optimal_loan_result {
                Ok(Some((loan_amount, net_profit))) => {
                    if net_profit > I256::zero() {
                        info!(?route, %loan_amount, %net_profit, "🎉 PROFITABLE OPPORTUNITY FOUND!");
                         let execute_result = submit_arbitrage_transaction( sim_client, sim_state, route, loan_amount, net_profit, sim_nonce_manager ).await; // Pass NonceManager Arc
                         if let Err(e) = execute_result { error!(error = ?e, "Arbitrage execution failed"); }
                    }
                 }
                 Ok(None) => { debug!(?route, "No profitable loan found."); }
                 Err(e) => { error!(?route, error = ?e, "Optimal loan search failed"); }
            }
        }); // End simulation task spawn
    } // End loop through routes
    Ok(())
}

// END OF FILE: bot/src/event_handler.rs