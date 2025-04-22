// bot/src/main.rs

// --- Imports ---
use ethers::{
    prelude::*,
    providers::{Provider, Ws, StreamExt},
    // Add Filter, Log, H256; remove unused Tx types
    types::{Address, BlockId, BlockNumber, Filter, Log, H256, I256, U256, U64},
    utils::{format_units, keccak256}, // Keep format_units, add keccak256
};
use eyre::Result;
use std::{sync::Arc, cmp::max};
use tokio::{sync::Mutex, time::{interval, Duration}}; // Add sync::Mutex if needed for state
use chrono::Utc;
use dashmap::DashMap; // For concurrent state map
use tracing::{info, error, warn, debug, instrument}; // Use tracing macros

// --- Module Declarations ---
mod config; mod utils; mod simulation; mod bindings; mod encoding; mod deploy; mod gas; mod event_handler;
// --- Use Statements ---
use crate::config::load_config; use crate::utils::*; use crate::simulation::find_optimal_loan_amount; // Keep find_optimal for later
use crate::bindings::{ UniswapV3Pool, VelodromeV2Pool, VelodromeRouter, BalancerVault, QuoterV2, IERC20, ArbitrageExecutor, };
use crate::encoding::encode_user_data; use crate::deploy::deploy_contract_from_bytecode; use crate::gas::estimate_flash_loan_gas;
// Import event handlers and state structs
use crate::event_handler::{handle_new_block, handle_log_event, create_event_filter, AppState, PoolState, DexType};

// --- Constants ---
// Constants related to fixed simulation amount or polling interval are removed or repurposed
const ARBITRAGE_THRESHOLD_PERCENTAGE: f64 = 0.1; // Keep for arbitrage check
const FLASH_LOAN_FEE_RATE: f64 = 0.0000; // Keep for potential future use in profit calcs if moved
const MAX_TRADE_SIZE_VS_RESERVE_PERCENT: f64 = 5.0; // Keep for liquidity check

// --- Main Execution ---
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).with_timer(tracing_subscriber::fmt::time::uptime()).with_level(true).init();
    info!("🚀 Starting ULP 1.5 Arbitrage Bot (Event Monitoring Mode)...");
    let config = load_config()?;

    // --- Setup WebSocket Provider ---
    info!(url = %config.local_rpc_url, "Connecting WebSocket Provider...");
    let ws_provider = Provider::<Ws>::connect(&config.local_rpc_url).await?;
    let provider = Arc::new(ws_provider);
    info!("✅ WebSocket Provider connected.");

    // --- Setup Signer Client ---
    info!("Setting up Signer Client (HTTP)...");
    let http_rpc_url = config.local_rpc_url.replace("ws://", "http://").replace("wss://", "https://");
    let http_provider = Provider::<Http>::try_from(http_rpc_url)?;
    let chain_id = http_provider.get_chainid().await?;
    info!(id = %chain_id, "Signer Chain ID obtained.");
    let wallet = config.local_private_key.parse::<LocalWallet>()?.with_chain_id(chain_id.as_u64());
    let client = Arc::new(SignerMiddleware::new(http_provider, wallet.clone()));
    info!("✅ Signer Client setup complete.");

    // --- Deploy Executor Contract ---
    let arb_executor_address: Address; /* ... deployment logic ... */
    if config.deploy_executor { arb_executor_address = deploy_contract_from_bytecode(client.clone(), &config.executor_bytecode_path).await?; } else { arb_executor_address = config.arb_executor_address.expect("Executor address missing"); }
    info!(address = ?arb_executor_address, "Using Executor contract.");

    // --- Initialize Shared State ---
    info!("Initializing application state...");
    let app_state = AppState {
        pool_states: Arc::new(DashMap::new()), // Concurrent map for pool data
        weth_address: config.weth_address,
        usdc_address: config.usdc_address,
        weth_decimals: config.weth_decimals,
        usdc_decimals: config.usdc_decimals,
    };
    info!("✅ Application state initialized.");

    // --- Load Initial Pool States & Setup Monitoring ---
    // TODO: Replace hardcoding with dynamic loading/discovery
    warn!("Using hardcoded initial pool list - implement dynamic loading!");
    let target_pairs_file = config.matched_pairs_file_path.clone(); // Get file path
    let initial_monitored_pools: Vec<(Address, DexType)> = vec![
        // Example: Add pools from config or a loaded file
         (config.uni_v3_pool_addr, DexType::UniswapV3),
         (config.velo_v2_pool_addr, DexType::VelodromeV2),
         // Load more from target_pairs_file?
    ];
    // TODO: Fetch initial state (reserves, sqrtPrice, tokens, fee, stable) for each pool
    // and populate app_state.pool_states map. Handle errors gracefully.
    // Example for one pool (needs loop and error handling):
    if let Err(e) = fetch_and_cache_pool_state(config.uni_v3_pool_addr, DexType::UniswapV3, client.clone(), app_state.clone()).await {
        error!(pool=?config.uni_v3_pool_addr, error=?e, "Failed to fetch initial state for UniV3 pool");
    }
     if let Err(e) = fetch_and_cache_pool_state(config.velo_v2_pool_addr, DexType::VelodromeV2, client.clone(), app_state.clone()).await {
        error!(pool=?config.velo_v2_pool_addr, error=?e, "Failed to fetch initial state for VeloV2 pool");
    }


    // --- Define Event Filters ---
    // TODO: Define these based on actual event signatures from ABIs
    let uni_v3_swap_topic = H256::from_slice(&keccak256("Swap(address,address,int256,int256,uint160,uint128,int24)"));
    let velo_v2_swap_topic = H256::from_slice(&keccak256("Swap(address,uint256,uint256,uint256,uint256,address)"));
    let event_topics = vec![uni_v3_swap_topic, velo_v2_swap_topic];

    // Filter for Swap events from the initial list of monitored pools
    let log_filter = Filter::new()
        .address(initial_monitored_pools.iter().map(|(addr, _)| *addr).collect::<Vec<Address>>()) // Filter by pool addresses
        .topic0(event_topics); // Filter by Swap event signatures

    // --- Subscribe to Events ---
    info!("Subscribing to new block headers...");
    let mut block_stream = provider.subscribe_blocks().await?;
    info!("✅ Subscribed to block headers.");

    info!("Subscribing to Swap logs...");
    let mut log_stream = provider.subscribe_logs(&log_filter).await?;
    info!("✅ Subscribed to logs.");


    // --- Main Event Loop ---
    info!("Starting main event processing loop...");
    loop {
        tokio::select! {
            biased; // Prioritize logs over blocks slightly if they arrive together

            // --- Handle Log Events ---
            Some(log_result) = log_stream.next() => {
                match log_result {
                    Ok(log) => {
                        // Spawn non-blocking task to handle log processing
                        let state_clone = app_state.clone();
                        let provider_clone = provider.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_log_event(log, state_clone, provider_clone).await {
                                error!(error = ?e, "Error handling log event");
                            }
                        });
                    }
                    Err(e) => {
                        error!(error = ?e, "Error receiving log from stream. Stream may have ended.");
                        // TODO: Implement robust reconnection/resubscription logic here
                        tokio::time::sleep(Duration::from_secs(10)).await; // Wait before potential retry
                    }
                }
            }

            // --- Handle New Blocks ---
            Some(block) = block_stream.next() => {
                if let Some(block_number) = block.number {
                    // Spawn non-blocking task to handle block
                    let provider_clone = provider.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_new_block(block_number, provider_clone).await {
                            error!(block = %block_number, error = ?e, "Error handling new block");
                        }
                    });
                } else {
                     warn!(hash=?block.hash.unwrap_or_default(), "Received block without number");
                }
            }

             // --- Graceful Shutdown Signal Handling ---
             _ = tokio::signal::ctrl_c() => {
                 info!("Ctrl-C received, shutting down...");
                 break; // Exit the loop
             }

            // --- Prevent Tight Loop (If streams close unexpectedly) ---
             else => {
                 warn!("Event stream closed unexpectedly? Pausing before retry/exit.");
                 tokio::time::sleep(Duration::from_secs(5)).await;
                 // TODO: Consider attempting to resubscribe here? Or exiting.
                 // break; // Exit loop if streams end for now
             }

        } // End tokio::select!
    } // End loop

    info!("Bot shutdown complete.");
    Ok(())
} // End main


/// Helper function to fetch initial state for a pool and cache it.
async fn fetch_and_cache_pool_state(
    pool_addr: Address,
    dex_type: DexType,
    // Needs client for calls, not just provider, as contract instances use Middleware
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    app_state: AppState,
) -> Result<()> {
    info!(pool = ?pool_addr, dex = ?dex_type, "Fetching initial state...");
    let mut initial_state = PoolState {
        pool_address: pool_addr,
        dex_type: dex_type.clone(),
        sqrt_price_x96: None, tick: None, reserve0: None, reserve1: None,
        token0: Address::zero(), token1: Address::zero(), // Placeholder
        last_update_block: None, uni_fee: None, velo_stable: None, t0_is_weth: None,
    };

    match dex_type {
        DexType::UniswapV3 => {
            let pool = UniswapV3Pool::new(pool_addr, client.clone());
            let (slot0_data, token0_addr, token1_addr, fee) = tokio::try_join!(
                pool.slot_0().call(),
                pool.token_0().call(),
                pool.token_1().call(),
                pool.fee().call()
            ).wrap_err("Failed to fetch UniV3 initial state")?;
            initial_state.sqrt_price_x96 = Some(slot0_data.0);
            initial_state.tick = Some(slot0_data.1);
            initial_state.token0 = token0_addr;
            initial_state.token1 = token1_addr;
            initial_state.uni_fee = Some(fee);
             // Determine t0_is_weth based on fetched addresses
             initial_state.t0_is_weth = Some(token0_addr == app_state.weth_address && token1_addr == app_state.usdc_address);
        }
        DexType::VelodromeV2 => {
             let pool = VelodromeV2Pool::new(pool_addr, client.clone());
             let (reserves_data, token0_addr, token1_addr, is_stable) = tokio::try_join!(
                 pool.get_reserves().call(),
                 pool.token_0().call(),
                 pool.token_1().call(),
                 pool.stable().call()
             ).wrap_err("Failed to fetch VeloV2 initial state")?;
             initial_state.reserve0 = Some(reserves_data.0.into());
             initial_state.reserve1 = Some(reserves_data.1.into());
             initial_state.token0 = token0_addr;
             initial_state.token1 = token1_addr;
             initial_state.velo_stable = Some(is_stable);
              // Determine t0_is_weth based on fetched addresses
             initial_state.t0_is_weth = Some(token0_addr == app_state.weth_address && token1_addr == app_state.usdc_address);
        }
        DexType::Unknown => { return Err(eyre::eyre!("Cannot fetch state for Unknown DEX type")); }
    }

    // Use latest block number as update block?
    let block_num = client.get_block_number().await?;
    initial_state.last_update_block = Some(block_num);

    debug!(pool=?pool_addr, state=?initial_state, "Initial state fetched.");
    // Insert into shared state map
    app_state.pool_states.insert(pool_addr, initial_state);

    Ok(())
}

// END OF FILE: bot/src/main.rs