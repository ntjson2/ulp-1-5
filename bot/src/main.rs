// bot/src/main.rs

// --- Imports ---
use ethers::{
    prelude::*,
    providers::{Provider, StreamExt, Ws},
    types::{
        Address, BlockId, BlockNumber, Eip1559TransactionRequest, Filter, Log, H256, I256, U256,
        U64, // Keep needed types
    },
    utils::{format_units, keccak256, parse_units},
};
use eyre::{Result, WrapErr};
use std::{cmp::max, collections::HashSet, sync::Arc, time::Duration as StdDuration};
use tokio::{
    sync::Mutex, // Mutex might not be needed if using DashMap correctly
    time::{interval, timeout, Duration},
};
use chrono::Utc;
use dashmap::DashMap;
use futures_util::{future::join_all, stream::select_all, StreamExt as FuturesStreamExt};
use lazy_static::lazy_static; // Ensure this import is present
use tracing::{debug, error, info, instrument, warn, Level};
use tracing_subscriber::{fmt, EnvFilter};

// FIX E0432: Add module declaration - Already present
mod path_optimizer;

// FIX E0432: Add use statement - Already present
use crate::path_optimizer::{find_top_routes, RouteCandidate};

// --- Module Declarations ---
mod bindings;
mod config;
mod deploy;
mod encoding;
mod event_handler; // Assuming state definitions live here for now
mod gas;
mod simulation;
mod state; // Define state module
mod transaction; // Define transaction module
mod utils;

// --- Use Statements ---
use crate::bindings::{
    ArbitrageExecutor, BalancerVault, IERC20, IUniswapV3Factory, IVelodromeFactory, QuoterV2,
    UniswapV3Pool, VelodromeRouter, VelodromeV2Pool,
};
use crate::config::load_config;
use crate::deploy::deploy_contract_from_bytecode;
use crate::encoding::encode_user_data;
// Use types from event_handler
use crate::event_handler::{handle_log_event, handle_new_block};
use crate::gas::estimate_flash_loan_gas;
use crate::simulation::find_optimal_loan_amount;
// Use types from state module
use crate::state::{AppState, DexType, PoolState}; // Assuming PoolSnapshot might be added here later
// Import specifics from utils
use crate::utils::{f64_to_wei, v2_price_from_reserves, v3_price_from_sqrt, ToF64Lossy};
// Transaction module placeholder
// use crate::transaction::submit_arbitrage_transaction;

// --- Constants ---
// Moved ARBITRAGE_THRESHOLD_PERCENTAGE potentially to path_optimizer logic
// const FLASH_LOAN_FEE_RATE: f64 = 0.0000; // Balancer V2 has 0 fee currently - Removed as it's assumed 0 now
// const POLLING_INTERVAL_SECONDS: u64 = 5; // Not used with event stream
// const MAX_TRADE_SIZE_VS_RESERVE_PERCENT: f64 = 5.0; // Relevant for simulation/pathfinding
const INITIAL_STATE_FETCH_TIMEOUT_SECS: u64 = 120; // Increased timeout
const EVENT_STREAM_HEALTH_CHECK_INTERVAL_SECS: u64 = 60;
const POOL_FETCH_TIMEOUT_SECS: u64 = 15; // Timeout for individual pool data fetches


// --- Event Signatures (Define ONCE here, make pub) ---
// Add pub to make these accessible to event_handler via crate::
lazy_static! {
    pub static ref UNI_V3_SWAP_TOPIC: H256 = H256::from_slice(&keccak256(b"Swap(address,address,int256,int256,uint160,uint128,int24)"));
    pub static ref VELO_V2_SWAP_TOPIC: H256 = H256::from_slice(&keccak256(b"Swap(address,uint256,uint256,uint256,uint256,address)"));
    pub static ref UNI_V3_POOL_CREATED_TOPIC: H256 = H256::from_slice(&keccak256(b"PoolCreated(address,address,uint24,int24,address)"));
    // Make sure the signature matches the actual event in IVelodromeFactory.json ABI
    pub static ref VELO_V2_POOL_CREATED_TOPIC: H256 = H256::from_slice(&keccak256(b"PoolCreated(address,address,bool,address,uint256)"));
}

// --- Main Execution ---
#[tokio::main]
async fn main() -> Result<()> {
    // --- Initialize Logging ---
    fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(Level::INFO.into())) // Default to INFO, override with RUST_LOG
        .with_target(true) // Show module path
        .with_line_number(true) // Show line numbers
        .init();

    info!("🚀 Starting Arbitrage Bot ULP 1.5 (Scalable Core)...");

    // --- Load Configuration ---
    let config = load_config().wrap_err("Failed to load configuration")?;
    debug!(config = ?config, "Configuration loaded");

    // --- Setup Providers & Client ---
    info!(url = %config.ws_rpc_url, "Connecting WebSocket Provider...");
    let provider_ws = Provider::<Ws>::connect(&config.ws_rpc_url)
        .await
        .wrap_err("Failed to connect WebSocket provider")?;
    let provider: Arc<Provider<Ws>> = Arc::new(provider_ws);
    info!("✅ WebSocket Provider connected.");

    info!("Setting up Signer Client (HTTP)...");
    let http_provider = Provider::<Http>::try_from(config.http_rpc_url.clone())
        .wrap_err("Failed to create HTTP provider")?;
    let chain_id = http_provider
        .get_chainid()
        .await
        .wrap_err("Failed to get chain ID")?;
    info!(id = %chain_id, "Signer Chain ID obtained.");
    let wallet = config
        .local_private_key
        .parse::<LocalWallet>()
        .wrap_err("Failed to parse local private key")?
        .with_chain_id(chain_id.as_u64());
    let signer_middleware_instance = SignerMiddleware::new(http_provider.clone(), wallet.clone());
    let client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>> =
        Arc::new(signer_middleware_instance);
    info!(address = ?wallet.address(), "✅ Signer Client setup complete.");

    // --- Confirm Balancer Flash Loan Fee Assumption ---
    // Balancer V2 Vault does not have an easily queryable fee parameter off-chain.
    // Fees are handled internally during settlement. We rely on the known 0% fee.
    info!(vault = %config.balancer_vault_address, "ASSUMPTION: Using Balancer V2 Vault - Flash Loan fee is 0%.");

    // --- Deploy Executor Contract (if configured) ---
    let arb_executor_address: Address = if config.deploy_executor {
        info!("Auto-deploy enabled for ArbitrageExecutor...");
        let deployed_address =
            deploy_contract_from_bytecode(client.clone(), &config.executor_bytecode_path)
                .await
                .wrap_err("Failed to deploy executor contract")?;
        info!(address = ?deployed_address, "✅ Executor contract deployed.");
        deployed_address
    } else {
        info!("Using existing executor address from config...");
        config.arb_executor_address.ok_or_else(|| {
            eyre::eyre!("Executor address is required in config when DEPLOY_EXECUTOR=false")
        })?
    };
    info!(address = ?arb_executor_address, "Using Executor contract.");

    // --- Initialize Shared State ---
    // Create AppState from state module
    let app_state = Arc::new(AppState::new(config.clone())); // Pass config to AppState constructor
    info!("🧠 Shared application state initialized.");
    let target_pair_filter = app_state.target_pair(); // Determine target pair based on config in state
    info!(pair = ?target_pair_filter, "Target pair filter set.");


    // --- Load Initial Pool States ---
    info!("🔍 Fetching initial states for target pair pools...");
    let mut initial_fetch_tasks = Vec::new();
    let mut initial_monitored_pools = HashSet::new(); // Track pools we are adding

    // --- Fetch UniV3 Pools ---
    let uni_factory = IUniswapV3Factory::new(config.uniswap_v3_factory_addr, client.clone());
    let fees = [100, 500, 3000, 10000]; // Common fee tiers
    if let Some((token_a, token_b)) = target_pair_filter {
        let (query_t0, query_t1) = if token_a < token_b { (token_a, token_b) } else { (token_b, token_a) };

        for fee in fees {
            debug!(token0 = ?query_t0, token1 = ?query_t1, fee = fee, "Querying UniV3 getPool...");
            match timeout(
                Duration::from_secs(POOL_FETCH_TIMEOUT_SECS),
                uni_factory.get_pool(query_t0, query_t1, fee).call()
            ).await {
                Ok(Ok(pool_addr)) if pool_addr != Address::zero() => {
                    info!(pool = ?pool_addr, fee = fee, "UniV3 target pool found via getPool.");
                    if initial_monitored_pools.insert(pool_addr) {
                        // Use fetch_and_cache_pool_state from state module now
                        initial_fetch_tasks.push(tokio::spawn(state::fetch_and_cache_pool_state(
                            pool_addr,
                            DexType::UniswapV3,
                            client.clone(),
                            app_state.clone(), // Pass Arc<AppState>
                        )));
                    } else {
                         debug!(pool = ?pool_addr, "Pool already added for monitoring.");
                    }
                }
                Ok(Ok(_)) => { debug!(fee=fee, "No UniV3 pool found for fee tier."); }
                Ok(Err(e)) => warn!(error = ?e, token0 = ?query_t0, token1 = ?query_t1, fee = fee, "Failed to query UniV3 getPool (contract error)"),
                Err(_) => warn!(token0 = ?query_t0, token1 = ?query_t1, fee = fee, "Timeout querying UniV3 getPool"),
            }
        }
    } else {
        warn!("WETH/USDC addresses not configured, cannot fetch initial UniV3 pools by pair.");
    }

    // --- Fetch Velodrome V2 Pools ---
    let velo_factory = IVelodromeFactory::new(config.velodrome_v2_factory_addr, client.clone());
    match timeout(Duration::from_secs(POOL_FETCH_TIMEOUT_SECS*2),
                 velo_factory.all_pools_length().call()).await {
        Ok(Ok(pool_len_u256)) => {
            let pool_len = pool_len_u256.as_usize();
            info!(count = pool_len, "Total Velodrome pools found.");
            let num_to_check = pool_len;
            info!("Checking {} Velodrome pools for target pair...", num_to_check);
            for i in 0..num_to_check {
                match timeout(Duration::from_secs(POOL_FETCH_TIMEOUT_SECS),
                            velo_factory.all_pools(U256::from(i)).call()).await {
                    Ok(Ok(pool_addr)) if pool_addr != Address::zero() => {
                         match timeout(
                             Duration::from_secs(POOL_FETCH_TIMEOUT_SECS),
                             VelodromeV2Pool::new(pool_addr, client.clone()).tokens().call()
                         ).await {
                             Ok(Ok((t0, t1))) => {
                                 if state::is_target_pair_option(t0, t1, target_pair_filter) { // Use helper from state module
                                     info!(index = i, pool = ?pool_addr, token0 = ?t0, token1 = ?t1, "VeloV2 target pool found.");
                                     if initial_monitored_pools.insert(pool_addr) {
                                         initial_fetch_tasks.push(tokio::spawn(state::fetch_and_cache_pool_state(
                                             pool_addr,
                                             DexType::VelodromeV2,
                                             client.clone(),
                                             app_state.clone(),
                                         )));
                                     } else {
                                         debug!(pool = ?pool_addr, "Pool already added for monitoring.");
                                     }
                                 }
                             }
                             Ok(Err(e)) => warn!(pool = ?pool_addr, index = i, error = ?e, "Failed to get tokens for Velo pool (contract error)"),
                             Err(_) => warn!(pool = ?pool_addr, index = i, "Timeout fetching tokens for Velo pool"),
                         }
                     }
                     Ok(Ok(_)) => { /* Pool address is zero, skip */ }
                     Ok(Err(e)) => warn!(index = i, error = ?e, "Failed to get Velo pool address (contract error)"),
                     Err(_) => warn!(index = i, "Timeout getting Velo pool address"),
                 }
            }
        }
        Ok(Err(e)) => error!(error = ?e, "Failed to get Velodrome pool length (contract error)"),
        Err(_) => error!("Timeout getting Velodrome pool length"),
    }

    info!(
        "Waiting for initial pool state fetches ({} tasks)...",
        initial_fetch_tasks.len()
    );
    let fetch_results = match timeout(
        Duration::from_secs(INITIAL_STATE_FETCH_TIMEOUT_SECS),
        join_all(initial_fetch_tasks),
    )
    .await
    {
        Ok(results) => results,
        Err(_) => {
            error!("Timeout waiting for initial pool state fetches!");
            Vec::new()
        }
    };

    // Log results of initial fetches
    let mut successful_fetches = 0;
    let mut failed_fetches = 0;
    for result in fetch_results {
        match result {
            Ok(Ok(_)) => successful_fetches += 1,
            Ok(Err(e)) => {
                error!(error = ?e, "Initial fetch task failed");
                failed_fetches += 1;
            }
            Err(e) => {
                error!(error = ?e, "Initial fetch task panicked or was cancelled");
                failed_fetches += 1;
            }
        }
    }
    // Use app_state.pool_states.len() for final count after potential failures
    info!(
        "✅ Initial fetch round complete ({} successful, {} failed). Total monitored pools in state: {}.",
        successful_fetches,
        failed_fetches,
        app_state.pool_states.len() // Read count from AppState map
    );

    // --- Define Event Filters ---
    // Get addresses of currently monitored pools AFTER initial fetch
    let monitored_addresses: Vec<Address> =
        app_state.pool_states.iter().map(|entry| *entry.key()).collect();
    if monitored_addresses.is_empty() {
        warn!("No pools are being monitored after initial fetch. Check config, RPC connection, and target pair addresses.");
    } else {
        info!(
            "Monitoring {} pools for swap events: {:?}",
            monitored_addresses.len(),
            monitored_addresses
        );
    }

    let swap_filter = Filter::new()
        .address(monitored_addresses.clone()) // Monitor swaps only from known pools
        .topic0(vec![*UNI_V3_SWAP_TOPIC, *VELO_V2_SWAP_TOPIC]);

    let factory_filter = Filter::new()
        .address(vec![
            config.uniswap_v3_factory_addr,
            config.velodrome_v2_factory_addr,
        ])
        .topic0(vec![
            *UNI_V3_POOL_CREATED_TOPIC,
            *VELO_V2_POOL_CREATED_TOPIC,
        ]);

    // --- Subscribe to Events ---
    info!("Subscribing to event streams...");
    let mut block_stream = match provider.subscribe_blocks().await {
        Ok(stream) => { info!("✅ Subscribed to Blocks"); stream },
        Err(e) => { error!(error = ?e, "Failed to subscribe to blocks!"); return Err(e).wrap_err("Block subscription failed"); }
    };
    let swap_log_stream_option = if !monitored_addresses.is_empty() {
        match provider.subscribe_logs(&swap_filter).await {
            Ok(stream) => { info!("✅ Subscribed to Swap Logs for monitored pools"); Some(stream.boxed()) },
            Err(e) => { error!(error = ?e, "Failed to subscribe to swap logs!"); return Err(e).wrap_err("Swap log subscription failed"); }
        }
    } else {
         warn!("Skipping swap log subscription as no pools are monitored.");
         None
    };

    let factory_log_stream = match provider.subscribe_logs(&factory_filter).await {
         Ok(stream) => { info!("✅ Subscribed to Factory Logs"); stream.boxed() },
         Err(e) => { error!(error = ?e, "Failed to subscribe to factory logs!"); return Err(e).wrap_err("Factory log subscription failed"); }
    };

    let mut log_streams = vec![factory_log_stream];
    if let Some(swap_stream) = swap_log_stream_option {
         log_streams.push(swap_stream);
    }
    let mut all_log_stream = select_all(log_streams);
    info!("✅ Log Streams Merged.");

    // --- Main Event Loop ---
    info!("🚦 Starting main event processing loop...");
    let mut health_check_interval =
        interval(Duration::from_secs(EVENT_STREAM_HEALTH_CHECK_INTERVAL_SECS));
    let mut last_block_time = Utc::now();
    let mut last_log_time = Utc::now();

    loop {
        tokio::select! {
            biased; // Process logs slightly before blocks if available simultaneously

            // --- Log Event Handling ---
            maybe_log_result = all_log_stream.next() => {
                match maybe_log_result {
                    Some(Ok(log)) => {
                        last_log_time = Utc::now();
                        let state_clone = app_state.clone();
                        let client_clone = client.clone();
                        tokio::spawn(async move {
                            // Pass Arc<AppState> to handler
                            if let Err(e) = handle_log_event(log, state_clone, client_clone).await {
                                error!(error = ?e, "Error processing log event");
                            }
                        });
                    }
                    Some(Err(e)) => {
                        error!(error = ?e, "Log stream error. Reconnecting may be necessary.");
                        tokio::time::sleep(Duration::from_secs(10)).await;
                    }
                    None => { warn!("Log stream ended unexpectedly. Exiting."); break; }
                }
            }

            // --- Block Event Handling ---
            maybe_block_result = block_stream.next() => {
                match maybe_block_result {
                     Some(Ok(block_header)) => {
                         last_block_time = Utc::now();
                         if let Some(num) = block_header.number {
                             let state_clone = app_state.clone();
                             tokio::spawn(async move {
                                 // Pass Arc<AppState> to handler
                                 if let Err(e) = handle_new_block(num, state_clone).await {
                                      error!(block = num.as_u64(), error = ?e, "Error processing new block");
                                 }
                             });
                         } else { warn!("Received block header without number: {:?}", block_header); }
                    }
                    Some(Err(e)) => { error!(error = ?e, "Block stream error. Reconnecting may be necessary."); tokio::time::sleep(Duration::from_secs(10)).await; }
                    None => { warn!("Block stream ended unexpectedly. Exiting."); break; }
                }
            }

             // --- Health Check ---
            _ = health_check_interval.tick() => {
                let now = Utc::now();
                let time_since_last_block = now.signed_duration_since(last_block_time);
                let time_since_last_log = now.signed_duration_since(last_log_time);
                info!(
                    since_block_secs = time_since_last_block.num_seconds(),
                    since_log_secs = time_since_last_log.num_seconds(),
                    monitored_pools = app_state.pool_states.len(), // Read count from AppState map
                    "🩺 Health Check Tick."
                );
                let block_staleness_threshold = (EVENT_STREAM_HEALTH_CHECK_INTERVAL_SECS * 3) as i64;
                if time_since_last_block.num_seconds() > block_staleness_threshold {
                    error!(threshold = block_staleness_threshold, "Block stream seems stale (last block {}s ago). Potential connection issue.", time_since_last_block.num_seconds());
                }
                if !monitored_addresses.is_empty() {
                     let log_staleness_threshold = (EVENT_STREAM_HEALTH_CHECK_INTERVAL_SECS * 5) as i64;
                     if time_since_last_log.num_seconds() > log_staleness_threshold {
                        warn!(threshold = log_staleness_threshold, "Log stream seems quiet (last log {}s ago). May be normal or connection issue.", time_since_last_log.num_seconds());
                    }
                }
            }

            // --- Graceful Shutdown ---
            _ = tokio::signal::ctrl_c() => {
                info!("🔌 CTRL-C received. Initiating graceful shutdown...");
                break;
            }

            else => { warn!("Main event loop 'else' branch hit. Streams might have ended or there's an issue."); tokio::time::sleep(Duration::from_millis(100)).await; }
        } // End select!
    } // End loop

    info!("🛑 Bot shutdown complete.");
    Ok(())
} // End main


// --- Helper functions previously here (fetch_and_cache_pool_state, is_target_pair_option) are moved to state.rs ---

// END OF FILE: bot/src/main.rs