// bot/src/main.rs

// --- Imports ---
use ethers::prelude::*; // Bring in common traits and types
use ethers::providers::{Provider, StreamExt, Ws}; // Specific provider types
use ethers::types::{
    Address, Block, Filter, Log, H256, // Keep Block<H256> for stream
    I256, U256, U64, TxHash, // Core types
};
use ethers::utils::{format_units, keccak256, parse_units};
use eyre::{eyre, Result, WrapErr}; // Import eyre! macro
use std::{collections::HashSet, sync::Arc, time::Duration as StdDuration}; // StdDuration not used? Remove later if warning persists.
use tokio::sync::Mutex; // For NonceManager
use tokio::time::{interval, timeout, Duration};
use tokio::task::JoinHandle; // For typing fetch tasks
use chrono::Utc;
// use dashmap::DashMap; // Not directly used here
use futures_util::{future::join_all, stream::select_all, FutureExt, StreamExt as FuturesStreamExt};
use lazy_static::lazy_static;
use tracing::{debug, error, info, instrument, warn, Level, trace};
use tracing_subscriber::{fmt, EnvFilter};

// --- Module Declarations ---
mod bindings; mod config; mod deploy; mod encoding; mod event_handler; mod gas; mod local_simulator; mod path_optimizer; mod simulation; mod state; mod transaction; mod utils;

// --- Use Statements ---
use crate::bindings::*; // Use wildcard for all generated bindings
use crate::config::load_config;
use crate::deploy::deploy_contract_from_bytecode;
// use crate::encoding::encode_user_data; // Used transitively
use crate::event_handler::{handle_log_event, handle_new_block};
// use crate::gas::estimate_flash_loan_gas; // Used transitively
// use crate::path_optimizer::{find_top_routes, RouteCandidate}; // Used transitively
// use crate::simulation::find_optimal_loan_amount; // Used transitively
// Import state module AND specific types needed here. **NO `self`**
use crate::state::{self, AppState, DexType, PoolState};
use crate::transaction::NonceManager; // Keep NonceManager
// use crate::transaction::submit_arbitrage_transaction; // Called transitively
// use crate::utils::*; // Avoid wildcard, specific utils used transitively

// --- Constants ---
const INITIAL_STATE_FETCH_TIMEOUT_SECS: u64 = 120;
const EVENT_STREAM_HEALTH_CHECK_INTERVAL_SECS: u64 = 60;

// --- Event Signatures ---
lazy_static! {
    pub static ref UNI_V3_SWAP_TOPIC: H256 = H256::from_slice(&keccak256(b"Swap(address,address,int256,int256,uint160,uint128,int24)"));
    pub static ref UNI_V3_POOL_CREATED_TOPIC: H256 = H256::from_slice(&keccak256(b"PoolCreated(address,address,uint24,int24,address)"));
    // Use distinct names for clarity, even if signatures match for now
    pub static ref VELO_V2_SWAP_TOPIC: H256 = H256::from_slice(&keccak256(b"Swap(address,uint256,uint256,uint256,uint256,address)"));
    pub static ref VELO_V2_POOL_CREATED_TOPIC: H256 = H256::from_slice(&keccak256(b"PoolCreated(address,address,bool,address,uint256)"));
    // Add specific Aero topics if they differ, otherwise handler logic decides
    pub static ref AERO_SWAP_TOPIC: H256 = H256::from_slice(&keccak256(b"Swap(address,uint256,uint256,uint256,uint256,address)")); // Assumed same as Velo
    pub static ref AERO_POOL_CREATED_TOPIC: H256 = H256::from_slice(&keccak256(b"PoolCreated(address,address,bool,address,uint256)")); // Assumed same as Velo
}

// --- Main Execution ---
#[tokio::main]
async fn main() -> Result<()> {
    fmt().with_env_filter(EnvFilter::from_default_env().add_directive(Level::INFO.into())).with_target(true).with_line_number(true).init();
    info!("🚀 Starting Arbitrage Bot ULP 1.5 (Scalable Core)...");
    let config = load_config().wrap_err("Config load failed")?; debug!(?config, "Config loaded");

    // Setup Providers & Client
    info!(url = %config.ws_rpc_url, "Connecting WS..."); let provider_ws = Provider::<Ws>::connect(&config.ws_rpc_url).await?; let provider: Arc<Provider<Ws>> = Arc::new(provider_ws); info!("✅ WS Connected.");
    info!("Setting up Signer Client (HTTP)..."); let http_provider = Provider::<Http>::try_from(config.http_rpc_url.clone())?; let chain_id = config.chain_id.unwrap_or(http_provider.get_chainid().await?.as_u64()); info!(%chain_id, "Using Chain ID.");
    let wallet = config.local_private_key.parse::<LocalWallet>()?.with_chain_id(chain_id); let wallet_address = wallet.address();
    let client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>> = Arc::new(SignerMiddleware::new(http_provider.clone(), wallet)); info!(address = ?wallet_address, "✅ Signer Client OK.");

    info!(vault = %config.balancer_vault_address, "ASSUMPTION: Balancer V2 Vault fee is 0%.");

    // Deploy Executor
    let arb_executor_address = if config.deploy_executor { info!("Deploying Executor..."); deploy_contract_from_bytecode(client.clone(), &config.executor_bytecode_path).await? } else { info!("Using existing executor..."); config.arb_executor_address.ok_or_else(|| eyre!("Executor address required when not deploying"))? }; info!(address = ?arb_executor_address, "Using Executor.");

    // Initialize State & Nonce Manager
    let app_state = Arc::new(AppState::new(config.clone())); info!("🧠 State initialized."); let target_pair_filter = app_state.target_pair(); info!(?target_pair_filter, "Target pair set.");
    let nonce_manager = Arc::new(NonceManager::new(wallet_address)); info!("🔑 Nonce Manager initialized.");

    // Initialize Contract Instances needed downstream
    let velo_router_instance = Arc::new(VelodromeRouter::new(config.velo_router_addr, client.clone()));
    let uni_quoter_instance = Arc::new(QuoterV2::new(config.quoter_v2_address, client.clone()));
    let aero_router_instance = if let Some(addr) = config.aerodrome_router_addr { Some(Arc::new(AerodromeRouter::new(addr, client.clone()))) } else { None };
    info!("✅ DEX Router/Quoter instances initialized.");

    // Load Initial Pool States
    info!("🔍 Fetching initial states..."); let mut tasks: Vec<JoinHandle<()>> = Vec::new(); let mut monitored = HashSet::new(); let fetch_timeout = Duration::from_secs(config.fetch_timeout_secs.unwrap_or(15));
    let mut factory_addresses_for_filter = vec![config.uniswap_v3_factory_addr, config.velodrome_v2_factory_addr]; if let Some(a) = config.aerodrome_factory_addr { factory_addresses_for_filter.push(a); }

    // Fetch UniV3
    if let Some((token_a, token_b)) = target_pair_filter { let f = IUniswapV3Factory::new(config.uniswap_v3_factory_addr, client.clone()); let fees = [100, 500, 3000, 10000]; let (q0, q1) = if token_a<token_b {(token_a,token_b)} else {(token_b,token_a)}; for fee in fees { match timeout(fetch_timeout, f.get_pool(q0, q1, fee).call()).await { Ok(Ok(p)) if p!=Address::zero() => { if monitored.insert(p) { let client_c = client.clone(); let app_state_c = app_state.clone(); tasks.push(tokio::spawn(state::fetch_and_cache_pool_state(p, DexType::UniswapV3, client_c, app_state_c).map(|_|())));}}, _ => {},}} }
    // Fetch VeloV2
    let vf = IVelodromeFactory::new(config.velodrome_v2_factory_addr, client.clone()); match timeout(fetch_timeout*2, vf.all_pools_length().call()).await { Ok(Ok(len)) => fetch_velo_style_pools(DexType::VelodromeV2, &vf, len, &mut monitored, &mut tasks, client.clone(), app_state.clone()).await, Err(e) => error!(dex="VeloV2", error=?e, "Len fetch fail"), _ => error!(dex="VeloV2", "Timeout getPoolsLength"),}
    // Fetch Aerodrome
    if let Some(af_addr) = config.aerodrome_factory_addr { let af = IAerodromeFactory::new(af_addr, client.clone()); match timeout(fetch_timeout*2, af.all_pools_length().call()).await { Ok(Ok(len)) => fetch_velo_style_pools(DexType::Aerodrome, &af, len, &mut monitored, &mut tasks, client.clone(), app_state.clone()).await, Err(e) => error!(dex="Aero", error=?e, "Len fetch fail"), _ => error!(dex="Aero", "Timeout getPoolsLength"),} }

    info!("Waiting fetch tasks ({})...", tasks.len()); let _ = timeout(Duration::from_secs(INITIAL_STATE_FETCH_TIMEOUT_SECS), join_all(tasks)).await; info!("✅ Initial fetch complete. Pools: {}", app_state.pool_states.len());

    // Define Event Filters
    let monitored_addrs: Vec<Address> = app_state.pool_states.iter().map(|e| *e.key()).collect();
    let swap_topics = vec![*UNI_V3_SWAP_TOPIC, *VELO_AERO_SWAP_TOPIC]; // Use combined topic for Velo/Aero
    let swap_filter = Filter::new().address(monitored_addrs.clone()).topic0(swap_topics);
    let factory_topics = vec![*UNI_V3_POOL_CREATED_TOPIC, *VELO_AERO_POOL_CREATED_TOPIC]; // Use combined topic
    let factory_filter = Filter::new().address(factory_addresses_for_filter).topic0(factory_topics);

    // Subscribe to Events
    info!("Subscribing..."); let mut block_stream = provider.subscribe_blocks().await?; let swap_log_stream = if !monitored_addrs.is_empty() { Some(provider.subscribe_logs(&swap_filter).await?.boxed()) } else { warn!("No pools monitored, skipping swap logs."); None }; let factory_log_stream = provider.subscribe_logs(&factory_filter).await?.boxed();
    let mut log_streams = vec![factory_log_stream]; if let Some(s) = swap_log_stream { log_streams.push(s); } let mut all_log_stream = select_all(log_streams); info!("✅ Subscribed.");

    // Main Event Loop
    info!("🚦 Starting main loop..."); let mut health_check = interval(Duration::from_secs(EVENT_STREAM_HEALTH_CHECK_INTERVAL_SECS)); let mut last_block = Utc::now(); let mut last_log = Utc::now();
    loop { tokio::select! { biased;
        log_res = all_log_stream.next() => {
            // Correctly handle Option<Result<Log, _>> from stream
            match log_res {
                Some(Ok(log)) => {
                    last_log = Utc::now(); let s=app_state.clone(); let c=client.clone(); let nm=nonce_manager.clone();
                    // Pass initialized contract instances
                    let vr = velo_router_instance.clone();
                    let uq = uni_quoter_instance.clone();
                    let ar = aero_router_instance.clone(); // Clone optional Arc
                    tokio::spawn(async move {
                        // Pass aero_router Option<Arc> to handler
                        let _ = handle_log_event(log, s, c, nm, vr, uq, ar).await.map_err(|e| error!(error = ?e, "Log handle error"));
                    });
                }
                Some(Err(e)) => { error!(error=?e, "Log stream error."); tokio::time::sleep(Duration::from_secs(10)).await; } // Add delay on stream error
                None => { warn!("Log stream ended."); break; }
            }
        },
        block_res = block_stream.next() => {
             // Correctly handle Option<Result<Block<H256>, _>> from stream
             match block_res {
                  Some(Ok(bh)) => { last_block=Utc::now(); if let Some(n)=bh.number{let s=app_state.clone(); tokio::spawn(async move{let _ = handle_new_block(n,s).await;});}} // Handle error inside task
                  Some(Err(e)) => { error!(error=?e,"Block stream error."); tokio::time::sleep(Duration::from_secs(10)).await; }
                  None => { warn!("Block stream ended."); break; }
             }
        },
        _ = health_check.tick() => { let now=Utc::now(); info!(block_lag=(now-last_block).num_seconds(), log_lag=(now-last_log).num_seconds(), pools=app_state.pool_states.len(), "🩺 Health"); },
        _ = tokio::signal::ctrl_c() => { info!("🔌 Shutdown..."); break; },
        else => { tokio::time::sleep(Duration::from_millis(500)).await; }
    }}
    info!("🛑 Bot stopped."); Ok(())
}

/// Helper function to fetch pools for Velo-style factories. Updated signature.
async fn fetch_velo_style_pools<M: Middleware + 'static>(
    dex_type: DexType,
    factory_binding: &IVelodromeFactory<M>, // Use IVelodromeFactory binding
    pool_len: U256,
    monitored: &mut HashSet<Address>,
    tasks: &mut Vec<JoinHandle<()>>,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    app_state: Arc<AppState>,
) where M: Middleware + Sync + Send, M::Error: Send + Sync + 'static { // Add necessary bounds
     let fetch_timeout = Duration::from_secs(app_state.config.fetch_timeout_secs.unwrap_or(15));
     for i in 0..pool_len.as_usize() {
          match timeout(fetch_timeout, factory_binding.all_pools(U256::from(i)).call()).await {
               Ok(Ok(pool_addr)) if pool_addr != Address::zero() => {
                    let client_c = client.clone();
                    // Use generic binding for tokens() call
                    match timeout(fetch_timeout, VelodromeV2Pool::new(pool_addr, client_c).tokens().call()).await {
                         Ok(Ok((t0, t1))) => {
                              if state::is_target_pair_option(t0, t1, app_state.target_pair()) {
                                   if monitored.insert(pool_addr) {
                                        let dex_type_clone = dex_type.clone();
                                        let client_clone = client.clone();
                                        let app_state_clone = app_state.clone();
                                        tasks.push(tokio::spawn(async move {
                                             if let Err(e) = state::fetch_and_cache_pool_state(pool_addr, dex_type_clone, client_clone, app_state_clone).await { error!(pool=%pool_addr, error=?e,"Fetch pool state failed"); }
                                        }));
                                   }
                              }
                         }
                         Ok(Err(e)) => warn!(pool=%pool_addr, error=?e, "{:?} Failed get tokens", dex_type),
                         Err(_) => warn!(pool=%pool_addr, "{:?} Timeout get tokens", dex_type),
                    }
               }
               Ok(Ok(_))=>{} Ok(Err(e))=>warn!(idx=i, error=?e,"{:?} allPools fail",dex_type), Err(_)=>warn!(idx=i,"{:?} allPools timeout",dex_type),
          }
     }
}

// END OF FILE: bot/src/main.rs