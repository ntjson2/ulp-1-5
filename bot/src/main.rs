// bot/src/main.rs

// Use the library crate name 'ulp1_5' to access modules
use ulp1_5::bindings::{AerodromePool, IUniswapV3Factory, IVelodromeFactory, IAerodromeFactory, VelodromeV2Pool}; // Removed unused bindings::self
use ulp1_5::config::load_config; // Removed unused config::self
use ulp1_5::deploy::deploy_contract_from_bytecode; // Removed unused deploy::self
// encoding might not be needed directly in main
use ulp1_5::event_handler::{handle_log_event, handle_new_block}; // Removed unused event_handler::self
// gas might not be needed directly in main
// local_simulator only used when feature enabled, not directly in main runtime
// path_optimizer not needed directly in main
// simulation not needed directly in main
use ulp1_5::state::{self, AppState, DexType}; // Use state module and specific types
use ulp1_5::transaction::NonceManager; // Removed unused transaction::self
// utils might not be needed directly in main

// Import re-exported topics from lib.rs
use ulp1_5::{
    UNI_V3_POOL_CREATED_TOPIC, UNI_V3_SWAP_TOPIC, VELO_AERO_POOL_CREATED_TOPIC,
    VELO_AERO_SWAP_TOPIC,
};


use ethers::prelude::*;
use ethers::providers::{Provider, StreamExt, Ws};
use ethers::types::{
    Address, Filter, U256, H160, H256
};
use eyre::{eyre, Result, WrapErr};
use std::{collections::HashSet, sync::Arc};
use tokio::time::{interval, timeout, Duration};
use tokio::task::JoinHandle;
use chrono::Utc;
use futures_util::{future::join_all, FutureExt};
use tracing::{debug, error, info, warn, Level, trace};
use tracing_subscriber::{fmt, EnvFilter};

// --- Constants ---
const INITIAL_STATE_FETCH_TIMEOUT_SECS: u64 = 120;
const EVENT_STREAM_HEALTH_CHECK_INTERVAL_SECS: u64 = 60;

// --- Main Execution ---
#[tokio::main]
async fn main() -> Result<()> {
    fmt().with_env_filter(EnvFilter::from_default_env().add_directive(Level::INFO.into())).with_target(true).with_line_number(true).init();
    info!("🚀 Starting Arbitrage Bot ULP 1.5 (Scalable Core)...");
    // Use imported load_config directly
    let config = load_config().wrap_err("Config load failed")?; debug!(?config, "Config loaded");

    info!("Setting up providers & client...");
    let provider_ws = Provider::<Ws>::connect(&config.ws_rpc_url).await.wrap_err("WS connection failed")?;
    let provider_ws_arc: Arc<Provider<Ws>> = Arc::new(provider_ws); info!("✅ WS Connected.");
    let http_provider = Provider::<Http>::try_from(config.http_rpc_url.clone()).wrap_err("HTTP provider creation failed")?;
    let chain_id = config.chain_id.unwrap_or(http_provider.get_chainid().await?.as_u64()); info!(%chain_id, "Using Chain ID.");
    let wallet = config.local_private_key.parse::<LocalWallet>()?.with_chain_id(chain_id); let wallet_address = wallet.address();
    let client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>> = Arc::new(SignerMiddleware::new(http_provider.clone(), wallet)); info!(address = ?wallet_address, "✅ Signer Client OK.");

    info!(vault = %config.balancer_vault_address, "ASSUMPTION: Balancer V2 Vault fee is 0%.");

    // Use imported deploy function directly
    let arb_executor_address = if config.deploy_executor { info!("Deploying Executor..."); deploy_contract_from_bytecode(client.clone(), &config.executor_bytecode_path).await? } else { info!("Using existing executor..."); config.arb_executor_address.ok_or_else(|| eyre!("Executor address required when not deploying"))? }; info!(address = ?arb_executor_address, "Using Executor.");

    // Use imported AppState directly
    let nonce_manager = Arc::new(NonceManager::new(wallet_address)); info!("🔑 Nonce Manager initialized.");
    let app_state = Arc::new(AppState::new(
        http_provider.clone(),
        client.clone(),
        nonce_manager.clone(),
        config.clone(),
    ));
    info!("🧠 State initialized."); 
    let target_pair_filter = app_state.target_pair(); 
    info!(?target_pair_filter, "Target pair set.");

    info!("🔍 Fetching initial states..."); let mut tasks: Vec<JoinHandle<()>> = Vec::new(); let mut monitored = HashSet::new(); let fetch_timeout = Duration::from_secs(config.fetch_timeout_secs.unwrap_or(15));
    let mut factory_addresses_for_filter = vec![config.uniswap_v3_factory_addr, config.velodrome_v2_factory_addr]; if let Some(a) = config.aerodrome_factory_addr { factory_addresses_for_filter.push(a); }

    // --- Fetch Initial UniV3 Pools ---
    if let Some((token_a, token_b)) = target_pair_filter {
        let factory_addr = config.uniswap_v3_factory_addr;
        // Use imported binding directly
        let f = IUniswapV3Factory::new(factory_addr, client.clone());
        let fees = [100, 500, 3000, 10000];
        let (q0, q1) = if token_a < token_b { (token_a, token_b) } else { (token_b, token_a) };
        for fee in fees {
            match timeout(fetch_timeout, f.get_pool(q0, q1, fee).call()).await {
                Ok(Ok(pool_addr)) if pool_addr != Address::zero() => {
                    if monitored.insert(pool_addr) {
                        let client_c = client.clone();
                        let app_state_c = app_state.clone();
                        tasks.push(tokio::spawn(
                            // Use imported state function directly
                            state::fetch_and_cache_pool_state(pool_addr, DexType::UniswapV3, factory_addr, client_c, app_state_c).map(move |res| {
                                if let Err(e) = res { error!(pool=%pool_addr, dex=?DexType::UniswapV3, error=?e, "Spawned initial fetch state failed"); }
                            })
                        ));
                    }
                }
                Ok(Err(e)) => warn!(token0=%q0, token1=%q1, fee=fee, error=?e, "UniV3 getPool RPC failed"),
                Err(_) => warn!(token0=%q0, token1=%q1, fee=fee, "UniV3 getPool timeout"),
                _ => {}
            }
        }
    } else {
        warn!("Target pair not configured, skipping initial UniV3 pool fetch.");
    }

    // --- Fetch Initial VelodromeV2 Pools ---
    let velo_factory_addr = config.velodrome_v2_factory_addr;
    // Use imported binding directly
    let vf = IVelodromeFactory::new(velo_factory_addr, client.clone());
    match timeout(fetch_timeout * 2, vf.all_pools_length().call()).await {
        Ok(Ok(len)) => fetch_velo_style_pools(DexType::VelodromeV2, &vf, velo_factory_addr, len, &mut monitored, &mut tasks, client.clone(), app_state.clone()).await,
        Ok(Err(e)) => error!(dex = "VeloV2", error = ?e, "allPoolsLength RPC failed"),
        Err(_) => error!(dex = "VeloV2", "Timeout getting allPoolsLength"),
    }

    // --- Fetch Initial Aerodrome Pools ---
    if let Some(aero_factory_addr) = config.aerodrome_factory_addr {
        // Use imported binding directly
        let af = IAerodromeFactory::new(aero_factory_addr, client.clone());
        match timeout(fetch_timeout * 2, af.all_pools_length().call()).await {
            Ok(Ok(len)) => fetch_aero_style_pools(&af, aero_factory_addr, len, &mut monitored, &mut tasks, client.clone(), app_state.clone()).await,
            Ok(Err(e)) => error!(dex = "Aero", error = ?e, "allPoolsLength RPC failed"),
            Err(_) => error!(dex = "Aero", "Timeout getting allPoolsLength"),
        }
    }

    // --- Wait for Initial Fetches ---
    info!("Waiting initial fetch tasks ({})...", tasks.len());
    let join_results = timeout(Duration::from_secs(INITIAL_STATE_FETCH_TIMEOUT_SECS), join_all(tasks)).await;
    match join_results {
        Ok(results) => {
            let failed_count = results.iter().filter(|r| r.is_err()).count();
            if failed_count > 0 { warn!("{} initial pool fetch tasks failed.", failed_count); }
            else { info!("All initial pool fetch tasks completed."); }
        }
        Err(_) => { warn!("Timeout waiting for initial pool fetch tasks to complete."); }
    }
    info!("✅ Initial fetch process complete. Pools loaded: {}", app_state.pool_states.len());

    // --- Setup Event Filters ---
    let current_monitored_addrs: Vec<Address> = app_state.pool_states.iter().map(|e| *e.key()).collect();
    if current_monitored_addrs.is_empty() { warn!("No target pools found or fetched successfully during initial load. Swap monitoring might be ineffective."); }
    else { info!("Monitoring swaps for {} pools.", current_monitored_addrs.len()); }

    // Use imported topics directly
    let swap_topics = vec![*UNI_V3_SWAP_TOPIC, *VELO_AERO_SWAP_TOPIC];
    let factory_topics = vec![*UNI_V3_POOL_CREATED_TOPIC, *VELO_AERO_POOL_CREATED_TOPIC];

    let combined_addresses: Vec<H160> = current_monitored_addrs.into_iter()
        .chain(factory_addresses_for_filter.into_iter())
        .collect();
    let combined_topics: Vec<H256> = swap_topics.into_iter()
        .chain(factory_topics.into_iter())
        .collect();

    let combined_filter = Filter::new()
        .address(combined_addresses)
        .topic0(combined_topics);

    info!("Subscribing to event streams...");
    let mut block_stream = match provider_ws_arc.subscribe_blocks().await {
        Ok(stream) => stream, Err(e) => return Err(eyre!(e).wrap_err("Failed to subscribe to block stream")),
    };
    let mut log_stream = match provider_ws_arc.subscribe_logs(&combined_filter).await {
         Ok(stream) => stream, Err(e) => return Err(eyre!(e).wrap_err("Failed to subscribe to log stream")),
    };
    info!("✅ Subscribed.");

    // --- Main Event Loop ---
    info!("🚦 Starting main loop...");
    let mut health_check = interval(Duration::from_secs(EVENT_STREAM_HEALTH_CHECK_INTERVAL_SECS));
    let mut last_block_time = Utc::now();
    let mut last_log_time = Utc::now();

    loop { tokio::select! { biased;
        // --- Handle Log Events ---
        maybe_log = log_stream.next() => {
            match maybe_log {
                Some(log) => {
                    last_log_time = Utc::now();
                    trace!(tx_hash = ?log.transaction_hash, block = ?log.block_number, address = %log.address, topics=?log.topics, "Received log");
                    let s = app_state.clone();
                    let c = client.clone();
                    let nm = nonce_manager.clone();
                    tokio::spawn(async move {
                        // Use imported handle_log_event directly
                        if let Err(e) = handle_log_event(log, s, c, nm).await { error!(error = ?e, "handle_log_event failed"); }
                    });
                }
                None => { error!("ALERT: Log stream subscription ended unexpectedly. WS connection may be lost. Shutting down."); break; }
            }
        },
        // --- Handle Block Events ---
        maybe_block = block_stream.next() => {
            match maybe_block {
                 Some(block) => {
                    last_block_time = Utc::now();
                    if let Some(n) = block.number {
                        trace!("Received block #{}", n.as_u64());
                        let s = app_state.clone();
                        tokio::spawn(async move {
                             // Use imported handle_new_block directly
                            if let Err(e) = handle_new_block(n, s).await { error!(block = n.as_u64(), error = ?e, "handle_new_block failed"); }
                        });
                    } else { warn!("Block received without number: {:?}", block.hash); }
                 }
                 None => { error!("ALERT: Block stream subscription ended unexpectedly. WS connection may be lost. Shutting down."); break; }
             }
        },
        // --- Health Check Timer ---
        _ = health_check.tick() => {
            let now = Utc::now();
            let block_lag = (now - last_block_time).num_seconds();
            let log_lag = (now - last_log_time).num_seconds();
            info!(block_lag = block_lag, log_lag = log_lag, pools = app_state.pool_states.len(), snapshots = app_state.pool_snapshots.len(), "🩺 Health");
             // Access config values via app_state.config
            // Fix E0308: Cast u64 config values to i64 for comparison with Duration::num_seconds() result
            let critical_block_lag = app_state.config.critical_block_lag_seconds as i64;
            let critical_log_lag = app_state.config.critical_log_lag_seconds as i64;
            if block_lag > critical_block_lag || log_lag > critical_log_lag {
                 error!(
                    "ALERT: High event stream lag detected (Block: {}s > {}s, Log: {}s > {}s). Streams might be stalled. SHUTTING DOWN.",
                    block_lag, critical_block_lag, log_lag, critical_log_lag
                );
                 break;
            }
        },
        // --- Handle Ctrl+C ---
        _ = tokio::signal::ctrl_c() => { info!("🔌 Shutdown signal received..."); break; },
    }}
    info!("🛑 Bot stopped."); Ok(())
}

/// Helper function to fetch initial pools for Velo-style factories.
async fn fetch_velo_style_pools<M: Middleware + 'static>(
    dex_type: DexType,
    factory_binding: &IVelodromeFactory<M>,
    factory_addr: Address,
    pool_len: U256,
    monitored: &mut HashSet<Address>,
    tasks: &mut Vec<JoinHandle<()>>,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    app_state: Arc<AppState>,
) where M: Middleware + Sync + Send, M::Error: Send + Sync + 'static {
     // Access config via app_state.config
     let fetch_timeout = Duration::from_secs(app_state.config.fetch_timeout_secs.unwrap_or(15));
     let target_pair_opt = app_state.target_pair();

     for i in 0..pool_len.as_usize() {
          let index = U256::from(i);
          match timeout(fetch_timeout, factory_binding.all_pools(index).call()).await {
               Ok(Ok(pool_addr)) if pool_addr != Address::zero() => {
                    let client_c = client.clone();
                    // Use imported binding
                    let pool_binding = VelodromeV2Pool::new(pool_addr, client_c);
                    match timeout(fetch_timeout, pool_binding.tokens().call()).await {
                         Ok(Ok((t0, t1))) => {
                              // Use imported state function
                              if state::is_target_pair_option(t0, t1, target_pair_opt) {
                                   if monitored.insert(pool_addr) {
                                        let client_clone = client.clone();
                                        let app_state_clone = app_state.clone();
                                        tasks.push(tokio::spawn(async move {
                                             // Use imported state function
                                             if let Err(e) = state::fetch_and_cache_pool_state(pool_addr, dex_type, factory_addr, client_clone, app_state_clone).await {
                                                 error!(pool=%pool_addr, dex=?dex_type, error=?e,"Spawned fetch state failed for Velo-style pool");
                                             }
                                        }));
                                   } else {
                                       trace!(pool=%pool_addr, "Already monitoring pool.");
                                   }
                              } else {
                                  trace!(pool=%pool_addr, "Skipping non-target pair: {:?}/{:?}", t0, t1);
                              }
                         }
                         Ok(Err(e)) => warn!(pool=%pool_addr, error=?e, dex=?dex_type, "Failed tokens() RPC"),
                         Err(_) => warn!(pool=%pool_addr, dex=?dex_type, "Timeout tokens()"),
                    }
               }
               Ok(Ok(_)) => {}
               Ok(Err(e)) => warn!(idx=i, error=?e, dex=?dex_type, "allPools RPC failed"),
               Err(_) => warn!(idx=i, dex=?dex_type, "Timeout allPools"),
          }
     }
}

/// Helper function specifically for Aerodrome factory type.
async fn fetch_aero_style_pools(
    factory_binding: &IAerodromeFactory<SignerMiddleware<Provider<Http>, LocalWallet>>,
    factory_addr: Address,
    pool_len: U256,
    monitored: &mut HashSet<Address>,
    tasks: &mut Vec<JoinHandle<()>>,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    app_state: Arc<AppState>,
) {
    let dex_type = DexType::Aerodrome;
    // Access config via app_state.config
    let fetch_timeout = Duration::from_secs(app_state.config.fetch_timeout_secs.unwrap_or(15));
    let target_pair_opt = app_state.target_pair();

     for i in 0..pool_len.as_usize() {
          let index = U256::from(i);
          match timeout(fetch_timeout, factory_binding.all_pools(index).call()).await {
               Ok(Ok(pool_addr)) if pool_addr != Address::zero() => {
                    let client_c = client.clone();
                    // Use imported binding
                    let pool_binding = AerodromePool::new(pool_addr, client_c);
                    match timeout(fetch_timeout, pool_binding.tokens().call()).await {
                         Ok(Ok((t0, t1))) => {
                             // Use imported state function
                              if state::is_target_pair_option(t0, t1, target_pair_opt) {
                                   if monitored.insert(pool_addr) {
                                        let client_clone = client.clone();
                                        let app_state_clone = app_state.clone();
                                        tasks.push(tokio::spawn(async move {
                                             // Use imported state function
                                             if let Err(e) = state::fetch_and_cache_pool_state(pool_addr, dex_type, factory_addr, client_clone, app_state_clone).await {
                                                 error!(pool=%pool_addr, dex=?dex_type, error=?e,"Spawned fetch state failed for Aero pool");
                                             }
                                        }));
                                   } else {
                                        trace!(pool=%pool_addr, "Already monitoring pool.");
                                   }
                              } else {
                                  trace!(pool=%pool_addr, "Skipping non-target pair: {:?}/{:?}", t0, t1);
                              }
                         }
                         Ok(Err(e)) => warn!(pool=%pool_addr, error=?e, dex=?dex_type, "Failed tokens() RPC"),
                         Err(_) => warn!(pool=%pool_addr, dex=?dex_type, "Timeout tokens()"),
                    }
               }
               Ok(Ok(_)) => {}
               Ok(Err(e)) => warn!(idx=i, error=?e, dex=?dex_type, "allPools RPC failed"),
               Err(_) => warn!(idx=i, dex=?dex_type, "Timeout allPools"),
          }
     }
}