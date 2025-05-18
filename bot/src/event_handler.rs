// bot/src/event_handler.rs

use crate::bindings::{
    AerodromePool,
    VelodromeV2Pool,
    uniswap_v3_pool::SwapFilter as UniV3SwapFilter, 
    velodrome_v2_pool::SwapFilter as VeloSwapFilter, 
    i_uniswap_v3_factory::PoolCreatedFilter as UniV3PoolCreatedFilter, 
    i_velodrome_factory::PoolCreatedFilter as VeloPoolCreatedFilter, 
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
    contract::{EthLogDecode, ContractCall}, 
    prelude::*,
    types::{Log, U64, I256, U256, Address},
    providers::Provider,
};
use eyre::{Result};
use std::{sync::Arc, time::Duration};
use tokio::time::timeout;
use tracing::{debug, error, info, instrument, trace, warn};


// --- Event Handlers ---

pub async fn handle_new_block(block_number: U64, _state: Arc<AppState>) -> Result<()> {
    info!("🧱 New Block Received: #{}", block_number);
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
    let event_sig = match log.topics.get(0) {
        Some(t) => *t,
        None => {
            warn!("Log missing topic0, cannot identify event.");
            return Ok(());
        }
    };
    let contract_address = log.address;

    let velo_aero_pool_created_topic = *VELO_AERO_POOL_CREATED_TOPIC;
    let velo_aero_swap_topic = *VELO_AERO_SWAP_TOPIC;
    let uni_v3_pool_created_topic = *UNI_V3_POOL_CREATED_TOPIC;
    let uni_v3_swap_topic = *UNI_V3_SWAP_TOPIC;


    if event_sig == uni_v3_pool_created_topic {
        if contract_address != state.config.uniswap_v3_factory_addr {
            trace!("Ignoring UniV3 PoolCreated log from non-factory address: {}", contract_address);
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
                 if state::is_target_pair_option(event.token_0, event.token_1, state.target_pair()) {
                    info!(pool=%event.pool, dex=?dex_type, stable=%event.stable, "✨ Target {:?} pool created! Fetching state...", dex_type);
                     let s=state.clone();
                     let c=client.clone();
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

    } else if event_sig == uni_v3_swap_topic {
        if let Some(mut snapshot_entry) = state.pool_snapshots.get_mut(&contract_address) {
            trace!(pool=%contract_address, "Handling UniV3 Swap");
            let raw_log: RawLog = log.clone().into();
            match <UniV3SwapFilter as EthLogDecode>::decode_log(&raw_log) {
                Ok(swap) => {
                    let block_number = log.block_number; 
                    snapshot_entry.sqrt_price_x96 = Some(U256::from(swap.sqrt_price_x96)); 
                    snapshot_entry.tick = Some(swap.tick);
                    snapshot_entry.last_update_block = block_number;
                    debug!(pool=%contract_address, tick=%swap.tick, "UniV3 Snapshot Updated from Swap event");

                    let s = state.clone();
                    let c = client.clone();
                    let nm = nonce_manager.clone();
                    tokio::spawn(async move {
                        if let Err(e) = check_for_arbitrage(contract_address, s, c, nm).await {
                            error!(pool=%contract_address, error=?e, "Check arbitrage task failed after UniV3 swap");
                        }
                    });
                }
                Err(e) => error!(pool=%contract_address, error=?e, "Failed to decode UniV3 Swap event"),
            }
        } 

    } else if event_sig == velo_aero_swap_topic {
        if let Some(snapshot_entry) = state.pool_snapshots.get(&contract_address) {
             let dex_type = snapshot_entry.dex_type;
             let pool_address = *snapshot_entry.key();
             drop(snapshot_entry); 

             trace!(pool=%pool_address, dex=?dex_type, "Handling {:?} Swap", dex_type);
             let raw_log: RawLog = log.clone().into();
             match <VeloSwapFilter as EthLogDecode>::decode_log(&raw_log) {
                 Ok(_swap_data) => { 
                     let block_number = log.block_number;
                     let s = state.clone();
                     let c = client.clone();
                     let nm = nonce_manager.clone();
                     tokio::spawn(async move {
                         debug!(pool=%pool_address, dex=?dex_type, "Fetching reserves after swap...");
                         let timeout_duration = Duration::from_secs(s.config.fetch_timeout_secs.unwrap_or(10));

                         type ReservesCall = ContractCall<SignerMiddleware<Provider<Http>, LocalWallet>, (U256, U256, U256)>;
                         let pool_call_binding: ReservesCall = if dex_type == DexType::VelodromeV2 {
                             let pool = VelodromeV2Pool::new(pool_address, c.clone());
                             pool.get_reserves() 
                         } else { 
                             let pool = AerodromePool::new(pool_address, c.clone());
                             pool.get_reserves() 
                         };

                         let pool_call_future = pool_call_binding.call();

                         match timeout(timeout_duration, pool_call_future).await {
                            Ok(Ok(reserves)) => {
                                let (reserve0, reserve1, _ts): (U256, U256, U256) = reserves;
                                if let Some(mut snapshot) = s.pool_snapshots.get_mut(&pool_address) {
                                    snapshot.reserve0 = Some(reserve0);
                                    snapshot.reserve1 = Some(reserve1);
                                    snapshot.last_update_block = block_number;
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
                Err(e) => error!(pool=%contract_address, error=?e, "Failed to decode Velo/Aero Swap event"),
            }
        } 
    }
    Ok(())
}


/// Checks for arbitrage opportunities involving the pool that was just updated.
#[instrument(skip(state, client, nonce_manager), fields(updated_pool=%updated_pool_address), level = "debug")]
pub async fn check_for_arbitrage( 
    updated_pool_address: Address,
    state: Arc<AppState>,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    nonce_manager: Arc<NonceManager>,
) -> Result<()> {
    debug!("Checking for arbitrage opportunities triggered by pool update...");

    let updated_pool_snapshot = match state.pool_snapshots.get(&updated_pool_address) {
        Some(entry) => entry.value().clone(), 
        None => {
            warn!("Snapshot missing for updated pool {} during arbitrage check.", updated_pool_address);
            return Ok(()); 
        }
    };

    if !state::is_target_pair_option(
        updated_pool_snapshot.token0,
        updated_pool_snapshot.token1,
        state.target_pair(),
    ) {
        trace!("Updated pool {} is not the target pair. Skipping arbitrage check.", updated_pool_address);
        return Ok(());
    }

    debug!("Finding potential routes involving pool {}...", updated_pool_address);
    let top_routes: Vec<RouteCandidate> = find_top_routes(
        &updated_pool_snapshot,
        &state.pool_states,      
        &state.pool_snapshots,   
        &state.config,           
        state.weth_address,      
        state.usdc_address,      
        state.weth_decimals,     
        state.usdc_decimals,     
    );

    #[cfg(feature = "local_simulation")]
    {
        if !top_routes.is_empty() {
            if let Some(flag_arc) = &state.test_arb_check_triggered {
                let mut flag_guard = flag_arc.lock().await;
                *flag_guard = true;
                debug!("TEST HOOK: Set test_arb_check_triggered flag to true because routes were found.");
            } else {
                 warn!("TEST HOOK: Found routes but test_arb_check_triggered flag was None in AppState.");
            }
        } else {
             trace!("TEST_HOOK: No routes found, flag not set.");
        }
    }

    if top_routes.is_empty() {
        trace!("No potential arbitrage routes found involving pool {}.", updated_pool_address);
        return Ok(());
    }

    info!(pool=%updated_pool_address, count=top_routes.len(), "Found potential routes!");

     for route_candidate in top_routes.into_iter().take(1) { 
        info!(
            buy_pool = ?route_candidate.buy_pool_addr, buy_dex = ?route_candidate.buy_dex_type,
            sell_pool = ?route_candidate.sell_pool_addr, sell_dex = ?route_candidate.sell_dex_type,
            est_profit_pct = route_candidate.estimated_profit_usd, 
            "Evaluating Route Candidate..."
        );

        let sim_state = state.clone();
        let sim_client = client.clone();
        let sim_nonce_manager = nonce_manager.clone();
        let route = route_candidate.clone(); 

        tokio::spawn(async move {
            let route_buy_addr = route.buy_pool_addr;
            let route_sell_addr = route.sell_pool_addr;

            debug!(buy_pool =?route_buy_addr, sell_pool =?route_sell_addr, "Spawning simulation task for route");

            let buy_snapshot_option = sim_state.pool_snapshots.get(&route_buy_addr).map(|r| r.value().clone());
            let sell_snapshot_option = sim_state.pool_snapshots.get(&route_sell_addr).map(|r| r.value().clone());

            let gas_info = match crate::transaction::fetch_gas_price(sim_client.clone(), &sim_state.config).await {
                 Ok(g) => g,
                 Err(e) => {
                     error!(buy_pool = ?route_buy_addr, sell_pool = ?route_sell_addr, error=?e, "Gas price fetch failed in simulation task. Aborting route evaluation.");
                     return; 
                 }
            };
            let current_gas_price_gwei = gas_info.max_priority_fee_per_gas.to_f64_lossy() / 1e9;
            debug!(gas_price_gwei = current_gas_price_gwei, "Fetched gas price for simulation.");

             let optimal_loan_result = find_optimal_loan_amount(
                 sim_client.clone(),
                 sim_state.clone(),
                 &route, 
                 buy_snapshot_option.as_ref(),
                 sell_snapshot_option.as_ref(),
                 current_gas_price_gwei,
             ).await;

            match optimal_loan_result {
                Ok(Some((optimal_loan_amount_wei, max_net_profit_wei))) => {
                    if max_net_profit_wei > I256::zero() {
                        info!(
                            buy_pool = ?route_buy_addr, sell_pool = ?route_sell_addr,
                            optimal_loan = %optimal_loan_amount_wei,
                            max_profit = %max_net_profit_wei,
                            "🎉 PROFITABLE OPPORTUNITY IDENTIFIED! Attempting execution."
                        );
                         let execute_result = submit_arbitrage_transaction(
                             sim_client,
                             sim_state,
                             route, 
                             optimal_loan_amount_wei,
                             max_net_profit_wei,
                             sim_nonce_manager,
                        ).await;

                         if let Err(e) = execute_result {
                              error!(buy_pool = ?route_buy_addr, sell_pool = ?route_sell_addr, error = ?e,
                                     "Arbitrage execution attempt failed");
                         }
                    } else {
                        debug!(route = ?route, max_profit = %max_net_profit_wei, "Route evaluated, but max profit is not positive.");
                    }
                 }
                 Ok(None) => {
                    debug!(route = ?route, "No profitable loan amount found for this route during optimization.");
                 }
                 Err(e) => {
                    error!(route = ?route, error = ?e, "Optimal loan search failed for route");
                 }
            }
        }); 
    } 

    Ok(())
}