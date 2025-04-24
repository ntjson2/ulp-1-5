// bot/src/event_handler.rs

// FIX E0432: Import necessary items correctly
use crate::bindings::{ // Import all needed types from bindings
    i_uniswap_v3_factory::PoolCreatedFilter as UniV3PoolCreatedFilter,
    i_velodrome_factory::PoolCreatedFilter as VeloPoolCreatedFilter,
    uniswap_v3_pool::SwapFilter as UniV3SwapFilter,
    velodrome_v2_pool::SwapFilter as VeloV2SwapFilter,
    VelodromeV2Pool, UniswapV3Pool, // Keep pool types
};
use crate::utils::{v2_price_from_reserves, v3_price_from_sqrt}; // Keep needed utils
use dashmap::DashMap;
use ethers::{
    abi::RawLog, // Need RawLog
    contract::EthLogDecode, // Need trait for decoding
    prelude::*,
    types::{Address, Log, H256, I256, U256, U64},
};
use eyre::{Result, WrapErr}; // Keep Result and WrapErr
use std::sync::Arc;
use tracing::{debug, error, info, instrument, warn};
// Import helpers defined in main.rs
use crate::{is_target_pair_option, fetch_and_cache_pool_state};
// Import constants defined in main.rs
use crate::{ARBITRAGE_THRESHOLD_PERCENTAGE, UNI_V3_SWAP_TOPIC, VELO_V2_SWAP_TOPIC, UNI_V3_POOL_CREATED_TOPIC, VELO_V2_POOL_CREATED_TOPIC};

// --- State Definitions ---
#[derive(Debug, Clone)] pub struct PoolState { /* ... fields ... */ pub pool_address: Address, pub dex_type: DexType, pub sqrt_price_x96: Option<U256>, pub tick: Option<i32>, pub reserve0: Option<U256>, pub reserve1: Option<U256>, pub token0: Address, pub token1: Address, pub last_update_block: Option<U64>, pub uni_fee: Option<u32>, pub velo_stable: Option<bool>, pub t0_is_weth: Option<bool>, }
#[derive(Debug, Clone, PartialEq, Eq, Hash)] pub enum DexType { UniswapV3, VelodromeV2, Unknown }
#[derive(Debug, Clone)] pub struct AppState { pub pool_states: Arc<DashMap<Address, PoolState>>, pub weth_address: Address, pub usdc_address: Address, pub weth_decimals: u8, pub usdc_decimals: u8, }

// --- Event Handlers ---
pub async fn handle_new_block(block_number: U64, _state: AppState) -> Result<()> { info!("🧱 New Block Received: #{}", block_number); Ok(()) }

#[instrument(skip(log, state, client), fields(tx_hash = ?log.transaction_hash))]
pub async fn handle_log_event( log: Log, state: AppState, client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, ) -> Result<()> {
    let event_sig = match log.topics.get(0) { Some(topic) => *topic, None => { return Ok(()) } };
    let contract_address = log.address;
    // FIX E0277: Use log directly for decode_log if using EthEvent trait method
    // let raw_log: RawLog = (&log).into(); // No longer needed if using EthEvent trait directly

    debug!(address = ?contract_address, topic0 = ?event_sig, block = ?log.block_number, "Log Received");

    // Use EthEvent trait for decoding - simpler syntax
    if event_sig == *UNI_V3_POOL_CREATED_TOPIC {
        info!(factory = ?contract_address, "Handling Uniswap V3 PoolCreated event");
        // FIX E0034: Use EthEvent trait method
        if let Ok(event) = UniV3PoolCreatedFilter::decode_log(&log.clone().into()) { // Need .clone().into() for RawLog conversion needed by EthEvent trait method
              info!(pool = ?event.pool, token0 = ?event.token_0, token1 = ?event.token_1, fee = %event.fee, "New UniV3 Pool Detected");
              if is_target_pair_option(event.token_0, event.token_1, state.target_pair()) {
                  let app_state_clone = state.clone(); let client_clone = client.clone();
                  tokio::spawn(async move {
                     if let Err(e) = fetch_and_cache_pool_state(event.pool, DexType::UniswapV3, client_clone, app_state_clone).await { error!(pool=?event.pool, error=?e, "Failed fetch"); }
                     else { warn!(pool=?event.pool, "Need to update log filter!"); }
                  });
              } else { debug!("Ignoring new UniV3 pool - not target pair."); }
         } else { error!(address = ?contract_address, "Failed to decode UniV3 PoolCreated log"); }

    } else if event_sig == *VELO_V2_POOL_CREATED_TOPIC {
         info!(factory = ?contract_address, "Handling Velodrome V2 PoolCreated event");
         if let Ok(event) = VeloPoolCreatedFilter::decode_log(&log.clone().into()) {
               info!(pool = ?event.pool, token0 = ?event.token_0, token1 = ?event.token_1, stable = %event.stable, "New VeloV2 Pool Detected");
               if is_target_pair_option(event.token_0, event.token_1, state.target_pair()) {
                   let app_state_clone = state.clone(); let client_clone = client.clone();
                   tokio::spawn(async move {
                       if let Err(e) = fetch_and_cache_pool_state(event.pool, DexType::VelodromeV2, client_clone, app_state_clone).await { error!(pool=?event.pool, error=?e, "Failed fetch"); }
                       else { warn!(pool=?event.pool, "Need to update log filter!"); }
                   });
               } else { debug!("Ignoring new VeloV2 pool - not target pair."); }
          } else { error!(address = ?contract_address, "Failed to decode VeloV2 PoolCreated log"); }

    } else if event_sig == *UNI_V3_SWAP_TOPIC {
        if state.pool_states.contains_key(&contract_address) {
             debug!(pool = ?contract_address, "Handling Uniswap V3 Swap event");
             if let Ok(swap) = UniV3SwapFilter::decode_log(&log.clone().into()) {
                 let block_number = match log.block_number { Some(bn) => bn, None => { warn!("Swap log missing block number"); return Ok(()); } }; // Get block number from original log
                 state.pool_states.entry(contract_address).and_modify(|ps| { ps.sqrt_price_x96 = Some(swap.sqrt_price_x96); ps.tick = Some(swap.tick); ps.last_update_block = Some(block_number); });
                 debug!(pool = ?contract_address, "UniV3 State Updated");
                 check_for_arbitrage(contract_address, state.clone(), client.clone()).await?;
             } else { error!(pool=?contract_address, "Failed to decode UniV3 Swap log"); }
        } else { /* ... ignore untracked ... */ }

    } else if event_sig == *VELO_V2_SWAP_TOPIC {
        if state.pool_states.contains_key(&contract_address) {
             debug!(pool = ?contract_address, "Handling Velodrome V2 Swap event");
             // FIX E0034/E0609: VeloV2SwapFilter is correct type, use it; no meta field
             if let Ok(_swap_data) = VeloV2SwapFilter::decode_log(&log.clone().into()) {
                  let state_clone = state.clone(); let client_clone = client.clone();
                  let block_number = match log.block_number { Some(bn) => bn, None => { return Ok(()); } };
                  tokio::spawn(async move {
                     /* ... query reserves ... */
                     let velo_pool = VelodromeV2Pool::new(contract_address, client_clone.clone());
                     match velo_pool.get_reserves().call().await {
                         Ok(reserves) => {
                             state_clone.pool_states.entry(contract_address).and_modify(|ps| { /* ... update reserves ... */ });
                             if let Err(e) = check_for_arbitrage(contract_address, state_clone, client_clone).await { /* ... */ }
                         },
                         Err(e) => { /* ... */ }
                     }
                  });
             } else { error!(pool=?contract_address, "Failed to decode VeloV2 Swap log"); }
        } else { /* ... ignore untracked ... */ }
    } else { debug!(pool=?contract_address, topic0=?event_sig, "Ignoring irrelevant log"); }
    Ok(())
 }

// FIX E0583: Remove skip for client
#[instrument(skip(state, client), fields(updated_pool=%updated_pool_address))]
async fn check_for_arbitrage( updated_pool_address: Address, state: AppState, _client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, ) -> Result<()> {
    debug!("Checking for arbitrage opportunities...");
    // FIX E0382: Clone required fields
    let (updated_pool_state_clone, token0, token1) = match state.pool_states.get(&updated_pool_address) {
        Some(entry) => { let v = entry.value(); if !is_target_pair_option(v.token0, v.token1, state.target_pair()) { return Ok(()); } (v.clone(), v.token0, v.token1) },
        None => { return Ok(()); }
    };
    let updated_price = match calculate_pool_price_weth_per_usdc(&updated_pool_state_clone, &state) { Ok(p) if p > 0.0 => p, _ => { return Ok(()); } };

    for entry in state.pool_states.iter() { /* ... */ }
    Ok(())
}

/// Helper impl for AppState
impl AppState {
    // FIX E0599: Ensure method definition is correct
    pub fn target_pair(&self) -> Option<(Address, Address)> {
        if !self.weth_address.is_zero() && !self.usdc_address.is_zero() { Some((self.weth_address, self.usdc_address)) } else { None }
    }
}

/// Helper to calculate price
fn calculate_pool_price_weth_per_usdc(pool_state: &PoolState, app_state: &AppState) -> Result<f64> {
    // FIX E0308: Ensure all paths return Result<f64>
    let weth_decimals = app_state.weth_decimals; let usdc_decimals = app_state.usdc_decimals;
    let t0_is_weth = pool_state.t0_is_weth.ok_or_else(|| eyre::eyre!("Missing t0_is_weth"))?;
    match pool_state.dex_type {
         DexType::UniswapV3 => { let sqrt_price = pool_state.sqrt_price_x96.ok_or_else(|| eyre::eyre!("Missing sqrtPriceX96"))?; let price_t1_t0 = v3_price_from_sqrt(sqrt_price, weth_decimals, usdc_decimals)?; if t0_is_weth { if price_t1_t0.abs() < f64::EPSILON { Ok(0.0) } else { Ok(1.0 / price_t1_t0) } } else { Ok(price_t1_t0) } },
         DexType::VelodromeV2 => { let r0 = pool_state.reserve0.ok_or_else(|| eyre::eyre!("Missing reserve0"))?; let r1 = pool_state.reserve1.ok_or_else(|| eyre::eyre!("Missing reserve1"))?; let (dec0, dec1) = if t0_is_weth { (weth_decimals, usdc_decimals) } else { (usdc_decimals, weth_decimals) }; let price_t1_t0 = v2_price_from_reserves(r0, r1, dec0, dec1)?; if t0_is_weth { if price_t1_t0.abs() < f64::EPSILON { Ok(0.0) } else { Ok(1.0 / price_t1_t0) } } else { Ok(price_t1_t0) } },
         DexType::Unknown => Err(eyre::eyre!("Unknown DEX type")),
    }
}

// END OF FILE: bot/src/event_handler.rs