// bot/src/main.rs

// --- Imports ---
use ethers::{
    prelude::*,
    providers::{Provider, StreamExt, Ws},
    types::{
        Address, BlockId, BlockNumber, Eip1559TransactionRequest, Filter, Log, H256, I256, U256,
        U64, TxHash, // Added TxHash
    },
    utils::{format_units, keccak256, parse_units},
};
use eyre::{Result, WrapErr};
use std::{collections::HashSet, sync::Arc, time::Duration as StdDuration};
use tokio::sync::Mutex; // Needed for NonceManager
use tokio::time::{interval, timeout, Duration};
use chrono::Utc;
use dashmap::DashMap; // Still used internally by state.rs
use futures_util::{future::join_all, stream::select_all, StreamExt as FuturesStreamExt};
use lazy_static::lazy_static;
use tracing::{debug, error, info, instrument, warn, Level, trace}; // Added trace
use tracing_subscriber::{fmt, EnvFilter};

// --- Module Declarations ---
mod bindings;
mod config;
mod deploy;
mod encoding;
mod event_handler;
mod gas;
mod local_simulator; // <-- Added module declaration
mod path_optimizer;
mod simulation;
mod state;
mod transaction;
mod utils;

// --- Use Statements ---
use crate::bindings::{
    ArbitrageExecutor, BalancerVault, IERC20, IUniswapV3Factory, IVelodromeFactory, QuoterV2,
    UniswapV3Pool, VelodromeRouter, VelodromeV2Pool,
};
use crate::config::load_config;
use crate::deploy::deploy_contract_from_bytecode;
use crate::encoding::encode_user_data;
use crate::event_handler::{handle_log_event, handle_new_block};
use crate::gas::estimate_flash_loan_gas;
use crate::path_optimizer::{find_top_routes, RouteCandidate};
use crate::simulation::find_optimal_loan_amount;
use crate::state::{self, AppState, DexType, PoolState}; // Import state module itself for helpers
use crate::transaction::{NonceManager, submit_arbitrage_transaction};
use crate::utils::{f64_to_wei, v2_price_from_reserves, v3_price_from_sqrt, ToF64Lossy};

// --- Constants ---
const INITIAL_STATE_FETCH_TIMEOUT_SECS: u64 = 120;
const EVENT_STREAM_HEALTH_CHECK_INTERVAL_SECS: u64 = 60;

// --- Event Signatures ---
lazy_static! {
    pub static ref UNI_V3_SWAP_TOPIC: H256 = H256::from_slice(&keccak256(b"Swap(address,address,int256,int256,uint160,uint128,int24)"));
    pub static ref VELO_V2_SWAP_TOPIC: H256 = H256::from_slice(&keccak256(b"Swap(address,uint256,uint256,uint256,uint256,address)"));
    pub static ref UNI_V3_POOL_CREATED_TOPIC: H256 = H256::from_slice(&keccak256(b"PoolCreated(address,address,uint24,int24,address)"));
    pub static ref VELO_V2_POOL_CREATED_TOPIC: H256 = H256::from_slice(&keccak256(b"PoolCreated(address,address,bool,address,uint256)"));
}

// --- Main Execution ---
#[tokio::main]
async fn main() -> Result<()> {
    // Initialize Logging
    fmt().with_env_filter(EnvFilter::from_default_env().add_directive(Level::INFO.into()))
        .with_target(true).with_line_number(true).init();

    info!("🚀 Starting Arbitrage Bot ULP 1.5 (Scalable Core)...");

    // Load Configuration
    let config = load_config().wrap_err("Failed to load configuration")?;
    debug!(config = ?config, "Configuration loaded");

    // Setup Providers & Client
    info!(url = %config.ws_rpc_url, "Connecting WebSocket Provider...");
    let provider_ws = Provider::<Ws>::connect(&config.ws_rpc_url).await?;
    let provider: Arc<Provider<Ws>> = Arc::new(provider_ws);
    info!("✅ WebSocket Provider connected.");
    info!("Setting up Signer Client (HTTP)...");
    let http_provider = Provider::<Http>::try_from(config.http_rpc_url.clone())?;
    let chain_id = http_provider.get_chainid().await?;
    info!(id = %chain_id, "Signer Chain ID obtained.");
    let wallet = config.local_private_key.parse::<LocalWallet>()?.with_chain_id(chain_id.as_u64());
    let wallet_address = wallet.address();
    let client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>> = Arc::new(SignerMiddleware::new(http_provider.clone(), wallet));
    info!(address = ?wallet_address, "✅ Signer Client setup complete.");

    // Confirm Balancer Fee Assumption
    info!(vault = %config.balancer_vault_address, "ASSUMPTION: Using Balancer V2 Vault - Flash Loan fee is 0%.");

    // Deploy Executor Contract
    let arb_executor_address = if config.deploy_executor {
        info!("Auto-deploy enabled for ArbitrageExecutor...");
        deploy_contract_from_bytecode(client.clone(), &config.executor_bytecode_path).await?
    } else {
        info!("Using existing executor address from config...");
        config.arb_executor_address.ok_or_else(|| eyre!("Executor address required"))?
    };
    info!(address = ?arb_executor_address, "Using Executor contract.");

    // Initialize Shared State
    let app_state = Arc::new(AppState::new(config.clone()));
    info!("🧠 Shared application state initialized.");
    let target_pair_filter = app_state.target_pair();
    info!(pair = ?target_pair_filter, "Target pair filter set.");

    // Initialize Nonce Manager
    let nonce_manager = Arc::new(NonceManager::new(wallet_address));
    info!("🔑 Nonce Manager initialized.");

    // Initialize Contract Instances
    let velo_router_instance = Arc::new(VelodromeRouter::new(config.velo_router_addr, client.clone()));
    let uni_quoter_instance = Arc::new(QuoterV2::new(config.quoter_v2_address, client.clone()));
    info!("✅ Velo Router & Uni Quoter instances initialized.");

    // Load Initial Pool States
    info!("🔍 Fetching initial states for target pair pools...");
    let mut initial_fetch_tasks = Vec::new();
    let mut initial_monitored_pools = HashSet::new();
    let fetch_timeout = Duration::from_secs(config.fetch_timeout_secs.unwrap_or(15));

    // Fetch UniV3 Pools
    if let Some((token_a, token_b)) = target_pair_filter {
        let uni_factory = IUniswapV3Factory::new(config.uniswap_v3_factory_addr, client.clone());
        let fees = [100, 500, 3000, 10000];
        let (query_t0, query_t1) = if token_a < token_b { (token_a, token_b) } else { (token_b, token_a) };
        for fee in fees {
            match timeout(fetch_timeout, uni_factory.get_pool(query_t0, query_t1, fee).call()).await {
                Ok(Ok(pool_addr)) if pool_addr != Address::zero() => {
                    if initial_monitored_pools.insert(pool_addr) {
                        initial_fetch_tasks.push(tokio::spawn(state::fetch_and_cache_pool_state(
                            pool_addr, DexType::UniswapV3, client.clone(), app_state.clone(),
                        )));
                    }
                } // ... handle other cases ...
                 Ok(Ok(_)) => { trace!(fee=fee, "No UniV3 pool found for fee tier."); }
                 Ok(Err(e)) => warn!(error = ?e, fee = fee, "Failed to query UniV3 getPool"),
                 Err(_) => warn!(fee = fee, timeout_secs = fetch_timeout.as_secs(), "Timeout querying UniV3 getPool"),
            }
        }
    }

    // Fetch Velodrome V2 Pools
    let velo_factory = IVelodromeFactory::new(config.velodrome_v2_factory_addr, client.clone());
    match timeout(fetch_timeout * 2, velo_factory.all_pools_length().call()).await {
        Ok(Ok(pool_len_u256)) => {
            let pool_len = pool_len_u256.as_usize();
            info!(count = pool_len, "Checking {} Velodrome pools...", pool_len);
            for i in 0..pool_len {
                match timeout(fetch_timeout, velo_factory.all_pools(U256::from(i)).call()).await {
                     Ok(Ok(pool_addr)) if pool_addr != Address::zero() => {
                         match timeout(fetch_timeout, VelodromeV2Pool::new(pool_addr, client.clone()).tokens().call()).await {
                             Ok(Ok((t0, t1))) => {
                                 if state::is_target_pair_option(t0, t1, target_pair_filter) {
                                     if initial_monitored_pools.insert(pool_addr) {
                                         initial_fetch_tasks.push(tokio::spawn(state::fetch_and_cache_pool_state(
                                             pool_addr, DexType::VelodromeV2, client.clone(), app_state.clone(),
                                         )));
                                     }
                                 }
                             } // ... handle token fetch errors ...
                              Ok(Err(e)) => warn!(pool = %pool_addr, index = i, error = ?e, "Failed to get tokens for Velo pool"),
                              Err(_) => warn!(pool = %pool_addr, index = i, timeout_secs = fetch_timeout.as_secs(), "Timeout fetching tokens for Velo pool"),
                         }
                     } // ... handle pool address fetch errors ...
                      Ok(Ok(_)) => {} // Zero address pool
                      Ok(Err(e)) => warn!(index = i, error = ?e, "Failed to get Velo pool address"),
                      Err(_) => warn!(index = i, timeout_secs = fetch_timeout.as_secs(), "Timeout getting Velo pool address"),
                }
            }
        } // ... handle pool length fetch errors ...
         Ok(Err(e)) => error!(error = ?e, "Failed to get Velodrome pool length"),
         Err(_) => error!(timeout_secs = (fetch_timeout * 2).as_secs(), "Timeout getting Velodrome pool length"),
    }

    // Wait for initial fetches
    info!("Waiting for initial pool state fetches ({} tasks)...", initial_fetch_tasks.len());
    let fetch_timeout_total = Duration::from_secs(INITIAL_STATE_FETCH_TIMEOUT_SECS);
    let fetch_results = match timeout(fetch_timeout_total, join_all(initial_fetch_tasks)).await { /* ... */ Ok(res) => res, Err(_) => { error!("Timeout!"); Vec::new() } };
    // ... log fetch results ...
    info!( "✅ Initial fetch complete. Monitored pools in state: {}.", app_state.pool_states.len() );

    // Define Event Filters
    let monitored_addresses: Vec<Address> = app_state.pool_states.iter().map(|e| *e.key()).collect();
    // ... filter setup ...
    let swap_filter = Filter::new().address(monitored_addresses.clone()).topic0(vec![*UNI_V3_SWAP_TOPIC, *VELO_V2_SWAP_TOPIC]);
    let factory_filter = Filter::new().address(vec![config.uniswap_v3_factory_addr, config.velodrome_v2_factory_addr]).topic0(vec![*UNI_V3_POOL_CREATED_TOPIC, *VELO_V2_POOL_CREATED_TOPIC]);


    // Subscribe to Events
    info!("Subscribing to event streams...");
    let mut block_stream = provider.subscribe_blocks().await?;
    info!("✅ Subscribed to Blocks");
    let swap_log_stream_option = if !monitored_addresses.is_empty() { Some(provider.subscribe_logs(&swap_filter).await?.boxed()) } else { warn!("No pools monitored, skipping swap log subscription."); None };
    let factory_log_stream = provider.subscribe_logs(&factory_filter).await?.boxed();
    info!("✅ Subscribed to Logs");
    let mut log_streams = vec![factory_log_stream];
    if let Some(swap_stream) = swap_log_stream_option { log_streams.push(swap_stream); }
    let mut all_log_stream = select_all(log_streams);
    info!("✅ Log Streams Merged.");


    // Main Event Loop
    info!("🚦 Starting main event processing loop...");
    let mut health_check_interval = interval(Duration::from_secs(EVENT_STREAM_HEALTH_CHECK_INTERVAL_SECS));
    let mut last_block_time = Utc::now(); let mut last_log_time = Utc::now();
    loop {
        tokio::select! {
            biased;
            // Log Event Handling
            maybe_log_result = all_log_stream.next() => {
                if let Some(Ok(log)) = maybe_log_result {
                    last_log_time = Utc::now();
                    let state_clone = app_state.clone();
                    let client_clone = client.clone();
                    // Clone necessary Arcs for the handler task
                    let nonce_manager_clone = nonce_manager.clone();
                    let velo_router_clone = velo_router_instance.clone();
                    let uni_quoter_clone = uni_quoter_instance.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_log_event(
                            log, state_clone, client_clone,
                            nonce_manager_clone, velo_router_clone, uni_quoter_clone // Pass instances
                        ).await {
                            error!(error = ?e, "Error processing log event");
                        }
                    });
                } // ... handle stream errors / stream ending ...
                 else if let Some(Err(e)) = maybe_log_result { error!(error = ?e, "Log stream error."); tokio::time::sleep(Duration::from_secs(10)).await; }
                 else { warn!("Log stream ended."); break; }
            }
            // Block Event Handling
            maybe_block_result = block_stream.next() => {
                 if let Some(Ok(block_header)) = maybe_block_result {
                     last_block_time = Utc::now();
                     if let Some(num) = block_header.number {
                         let state_clone = app_state.clone();
                         tokio::spawn(async move { handle_new_block(num, state_clone).await; }); // Error handled inside
                     }
                 } // ... handle stream errors / stream ending ...
                  else if let Some(Err(e)) = maybe_block_result { error!(error = ?e, "Block stream error."); tokio::time::sleep(Duration::from_secs(10)).await; }
                  else { warn!("Block stream ended."); break; }
            }
            // Health Check
            _ = health_check_interval.tick() => { /* ... health check logic ... */ }
            // Graceful Shutdown
            _ = tokio::signal::ctrl_c() => { info!("🔌 CTRL-C received..."); break; }
            else => { warn!("Main loop 'else'."); tokio::time::sleep(Duration::from_millis(100)).await; }
        }
    }

    info!("🛑 Bot shutdown complete.");
    Ok(())
}


// END OF FILE: bot/src/main.rs