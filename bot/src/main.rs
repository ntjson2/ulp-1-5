// bot/src/main.rs

// --- Imports ---
use ethers::{
    prelude::*,
    providers::{Provider, Ws, StreamExt},
    types::{Address, Filter, H256, I256, U256, U64, Log, // Keep needed types
            BlockId, BlockNumber, Eip1559TransactionRequest},
    utils::{format_units, keccak256, parse_units},
};
use eyre::{Result, WrapErr};
use std::{sync::Arc, cmp::max, collections::HashSet, time::Duration as StdDuration};
use tokio::{sync::Mutex, time::{interval, Duration, timeout}};
use chrono::Utc;
use dashmap::DashMap;
use tracing::{info, error, warn, debug, instrument};
use futures_util::{future::join_all, stream::select_all, StreamExt as FuturesStreamExt};
use lazy_static::lazy_static; // Ensure this import is present
mod path_optimizer; // ← Add to top module declarations

use crate::path_optimizer::{find_top_routes, RouteCandidate}; // ← Add to use statements

// --- Module Declarations ---
mod config; mod utils; mod simulation; mod bindings; mod encoding; mod deploy; mod gas; mod event_handler;
// --- Use Statements ---
use crate::config::load_config;
// Remove crate::utils::* - import specifics if needed
use crate::utils::{f64_to_wei, ToF64Lossy, v3_price_from_sqrt, v2_price_from_reserves}; // Import specifics
use crate::simulation::find_optimal_loan_amount;
use crate::bindings::{ UniswapV3Pool, VelodromeV2Pool, VelodromeRouter, BalancerVault, QuoterV2, IERC20, ArbitrageExecutor, IUniswapV3Factory, IVelodromeFactory };
use crate::encoding::encode_user_data;
use crate::deploy::deploy_contract_from_bytecode;
use crate::gas::estimate_flash_loan_gas;
use crate::event_handler::{handle_new_block, handle_log_event, AppState, PoolState, DexType};

// --- Constants ---
/* ... */
const ARBITRAGE_THRESHOLD_PERCENTAGE: f64 = 0.1; const FLASH_LOAN_FEE_RATE: f64 = 0.0000;
const POLLING_INTERVAL_SECONDS: u64 = 5; const MAX_TRADE_SIZE_VS_RESERVE_PERCENT: f64 = 5.0;
const INITIAL_STATE_FETCH_TIMEOUT_SECS: u64 = 60; const EVENT_STREAM_HEALTH_CHECK_INTERVAL_SECS: u64 = 60;


// --- Event Signatures (Define ONCE here, make pub) ---
// Add pub to make these accessible to event_handler via crate::
lazy_static! {
    pub static ref UNI_V3_SWAP_TOPIC: H256 = H256::from_slice(&keccak256("Swap(address,address,int256,int256,uint160,uint128,int24)"));
    pub static ref VELO_V2_SWAP_TOPIC: H256 = H256::from_slice(&keccak256("Swap(address,uint256,uint256,uint256,uint256,address)"));
    pub static ref UNI_V3_POOL_CREATED_TOPIC: H256 = H256::from_slice(&keccak256("PoolCreated(address,address,uint24,int24,address)"));
    pub static ref VELO_V2_POOL_CREATED_TOPIC: H256 = H256::from_slice(&keccak256("PoolCreated(address,address,bool,address,uint256)"));
}

// --- Main Execution ---
#[tokio::main]
async fn main() -> Result<()> {


    
    /* ... Init ... */
    let config = load_config()?;

    // --- Setup Providers & Client ---
    info!(url = %config.ws_rpc_url, "Connecting WebSocket Provider...");
    // FIX E0425: Assign to correct variable name
    let ws_provider = Provider::<Ws>::connect(&config.ws_rpc_url).await?;
    let provider: Arc<Provider<Ws>> = Arc::new(ws_provider); // Use provider_ws if needed, keep using provider
    info!("✅ WebSocket Provider connected.");
    info!("Setting up Signer Client (HTTP)...");
    let http_provider = Provider::<Http>::try_from(config.http_rpc_url.clone())?;
    let chain_id = http_provider.get_chainid().await?; info!(id = %chain_id, "Signer Chain ID obtained.");
    let wallet = config.local_private_key.parse::<LocalWallet>()?.with_chain_id(chain_id.as_u64());
    // FIX E0425: Assign result of SignerMiddleware::new
    let signer_middleware = SignerMiddleware::new(http_provider, wallet.clone());
    let client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>> = Arc::new(signer_middleware);
    info!("✅ Signer Client setup complete.");

    // --- Deploy Executor Contract ---
    let arb_executor_address: Address;
    if config.deploy_executor {
        info!("Auto-deploy enabled...");
        // FIX E0425: Assign result to deployed_address first
        let deployed_address = deploy_contract_from_bytecode(client.clone(), &config.executor_bytecode_path).await?;
        arb_executor_address = deployed_address;
    } else { info!("Using existing executor address..."); arb_executor_address = config.arb_executor_address.expect("Executor address missing"); }
    info!(address = ?arb_executor_address, "Using Executor contract.");

    // --- Initialize Shared State ---
    /* ... */
    let app_state = AppState { /* ... */ };

    // --- Load Initial Pool States ---
    /* ... */
    let velo_factory = IVelodromeFactory::new(config.velodrome_v2_factory_addr, client.clone());
    if let Ok(pool_len) = velo_factory.all_pools_length().call().await {
        let num_to_check = std::cmp::min(pool_len.as_u64(), 500) as usize;
        for i in 0..num_to_check {
             if let Ok(pool_addr) = velo_factory.all_pools(U256::from(i)).call().await { // pool_addr defined here
                 if pool_addr != Address::zero() {
                     // FIX E0433 & E0425: Use std::time::Duration and pool_addr
                     if let Ok(Ok((t0, t1))) = timeout(std::time::Duration::from_secs(5), VelodromeV2Pool::new(pool_addr, client.clone()).tokens().call()).await {
                         if is_target_pair_option(t0, t1, target_pair_filter) {
                             if initial_monitored_pools.insert(pool_addr) {
                                  initial_fetch_tasks.push(tokio::spawn(fetch_and_cache_pool_state(pool_addr, DexType::VelodromeV2, client.clone(), app_state.clone())));
                              }
                         }
                     } // ...
                 }
             }
        } // ...
    } // ...
    let _ = timeout(tokio::time::Duration::from_secs(INITIAL_STATE_FETCH_TIMEOUT_SECS), join_all(initial_fetch_tasks)).await?;
    info!("✅ Initial fetch complete ({} pools).", app_state.pool_states.len());


    // --- Define Event Filters ---
    /* ... */
    // FIX E0425: Use static topics correctly
    let swap_filter = Filter::new().address(monitored_addresses.clone()).topic0(vec![*UNI_V3_SWAP_TOPIC, *VELO_V2_SWAP_TOPIC]);
    let factory_filter = Filter::new().address(vec![config.uniswap_v3_factory_addr, config.velodrome_v2_factory_addr]).topic0(vec![*UNI_V3_POOL_CREATED_TOPIC, *VELO_V2_POOL_CREATED_TOPIC]);

    // --- Subscribe to Events ---
    info!("Subscribing to streams...");
    // FIX E0425: Use correct provider variable name ('provider')
    let mut block_stream = provider.subscribe_blocks().await?; info!("✅ Blocks");
    let swap_log_stream = provider.subscribe_logs(&swap_filter).await?; info!("✅ Swap Logs");
    let factory_log_stream = provider.subscribe_logs(&factory_filter).await?; info!("✅ Factory Logs");
    // FIX E0425: Use correct stream variable names
    let mut all_log_stream = select_all(vec![swap_log_stream.boxed(), factory_log_stream.boxed()]); info!("✅ Log Streams Merged");

    // --- Main Event Loop ---
    info!("Starting main event processing loop..."); let mut health_check_interval = interval(Duration::from_secs(EVENT_STREAM_HEALTH_CHECK_INTERVAL_SECS));
    loop {
        tokio::select! { biased;
            // FIX E0308: Correct match arms for Option<Result<Log>>
            maybe_log_result = all_log_stream.next() => { match maybe_log_result {
                Some(Ok(log)) => { let s = app_state.clone(); let c = client.clone(); tokio::spawn(async move { if let Err(e) = handle_log_event(log, s, c).await { error!(error=?e,"Log handle error"); } }); }
                Some(Err(e)) => { error!(error = ?e, "Log stream error."); tokio::time::sleep(Duration::from_secs(10)).await; }
                None => { warn!("Log stream ended."); break; }
            } }
            // FIX E0425: Use correct block_stream variable
            Some(block) = block_stream.next() => { /* ... */ }
            _ = health_check_interval.tick() => { /* ... */ }
            _ = tokio::signal::ctrl_c() => { /* ... */ break; }
            else => { /* ... */ break; }
        } // End select!
    } // End loop
    info!("Bot shutdown complete."); Ok(())
} // End main

// --- Helper Functions ---
#[instrument(skip(client, app_state), fields(pool=%pool_addr, dex=?dex_type))]
async fn fetch_and_cache_pool_state( pool_addr: Address, dex_type: DexType, client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, app_state: AppState, ) -> Result<()> {
     // FIX E0308: Ensure Ok(()) is returned
     info!("Fetching initial state...");
     /* ... fetch logic ... */
     Ok(())
}

fn is_target_pair_option(addr0: Address, addr1: Address, target_pair: Option<(Address, Address)>) -> bool {
     // FIX E0308: Ensure boolean is returned
     match target_pair {
         Some((t_a, t_b)) => (addr0 == t_a && addr1 == t_b) || (addr0 == t_b && addr1 == t_a),
         None => true,
     }
}


// END OF FILE: bot/src/main.rs