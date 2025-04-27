// bot/src/state.rs

use crate::bindings::{AerodromePool, UniswapV3Pool, VelodromeV2Pool}; // Keep pool bindings
use crate::config::Config;
use dashmap::DashMap;
use ethers::{
    prelude::*,
    types::{Address, U256, U64},
};
use eyre::{Result, WrapErr};
use std::sync::Arc;
use tokio::time::{timeout, Duration};
use tracing::{debug, error, info, instrument, trace, warn};

// Represents the type of DEX pool
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DexType {
    UniswapV3,
    VelodromeV2,
    Aerodrome, // Added Aerodrome
    Unknown,
}

// Holds detailed state for a specific pool
#[derive(Debug, Clone)]
pub struct PoolState {
    pub pool_address: Address,
    pub dex_type: DexType,
    pub token0: Address,
    pub token1: Address,
    pub uni_fee: Option<u32>,
    pub velo_stable: Option<bool>, // Used by Velo & Aero
    pub t0_is_weth: Option<bool>,
}

// Holds minimal, frequently updated state for rapid lookups (Hot Cache).
#[derive(Debug, Clone)]
pub struct PoolSnapshot {
    pub pool_address: Address,
    pub dex_type: DexType,
    pub token0: Address,
    pub token1: Address,
    pub reserve0: Option<U256>, // VeloV2 / Aero / V2 AMMs
    pub reserve1: Option<U256>, // VeloV2 / Aero / V2 AMMs
    pub sqrt_price_x96: Option<U256>, // UniV3
    pub tick: Option<i32>,            // UniV3
    pub last_update_block: Option<U64>,
}

// Shared application state container.
#[derive(Debug, Clone)]
pub struct AppState {
    pub config: Config, // Store full configuration
    pub pool_states: Arc<DashMap<Address, PoolState>>,
    pub pool_snapshots: Arc<DashMap<Address, PoolSnapshot>>,
    // Cached common values from config
    pub weth_address: Address,
    pub usdc_address: Address,
    pub weth_decimals: u8,
    pub usdc_decimals: u8,
    // Store key contract addresses from config for easier access
    pub velo_router_addr: Option<Address>, // Made Option<> consistent
    pub aero_router_addr: Option<Address>, // From config directly
    pub uni_quoter_addr: Option<Address>, // Made Option<> consistent
}

impl AppState {
    /// Creates a new AppState instance.
    pub fn new(config: Config) -> Self {
        // Cache addresses needed frequently or for dynamic initialization
        let velo_router_addr = Some(config.velo_router_addr); // Assume Velo required? Or check config?
        let aero_router_addr = config.aerodrome_router_addr; // Optional based on config load
        let uni_quoter_addr = Some(config.quoter_v2_address); // Assume Quoter required?

        Self {
            weth_address: config.weth_address,
            usdc_address: config.usdc_address,
            weth_decimals: config.weth_decimals,
            usdc_decimals: config.usdc_decimals,
            velo_router_addr, // Store address
            aero_router_addr, // Store address
            uni_quoter_addr,  // Store address
            config, // Store full config
            pool_states: Arc::new(DashMap::new()),
            pool_snapshots: Arc::new(DashMap::new()),
        }
    }

    /// Returns the configured target token pair (WETH/USDC), ordered consistently.
    pub fn target_pair(&self) -> Option<(Address, Address)> {
        if !self.weth_address.is_zero() && !self.usdc_address.is_zero() {
            if self.weth_address < self.usdc_address { Some((self.weth_address, self.usdc_address)) }
            else { Some((self.usdc_address, self.weth_address)) }
        } else { warn!("WETH or USDC address is zero, target pair filtering disabled."); None }
    }
}


/// Fetches initial or updated state for a specific pool and caches it.
#[instrument(skip_all, fields(pool=%pool_addr, dex=?dex_type), level="info")]
pub async fn fetch_and_cache_pool_state(
    pool_addr: Address,
    dex_type: DexType,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    app_state: Arc<AppState>,
) -> Result<()> {
    // ... (implementation unchanged from Task 3.2d, already handles Aero using bindings) ...
    info!("Fetching state for {:?} pool {}...", dex_type, pool_addr);
    let weth_address = app_state.weth_address;
    let timeout_secs = app_state.config.fetch_timeout_secs.unwrap_or(15);
    let timeout_duration = Duration::from_secs(timeout_secs);

    let fetch_future = async {
        match dex_type {
            DexType::UniswapV3 => {
                 let pool = UniswapV3Pool::new(pool_addr, client.clone());
                 let (slot0_res, token0_res, token1_res, fee_res) = tokio::try_join!( pool.slot_0().call(), pool.token_0().call(), pool.token_1().call(), pool.fee().call() )?;
                 let (sqrt_price_x96_u160, tick, ..) = slot0_res;
                 let (token0, token1, fee) = (token0_res, token1_res, fee_res);
                 let t0_is_weth = token0 == weth_address;
                 let sqrt_price_x96 = U256::from(sqrt_price_x96_u160);
                 let pool_state = PoolState { pool_address: pool_addr, dex_type: dex_type.clone(), token0, token1, uni_fee: Some(fee), velo_stable: None, t0_is_weth: Some(t0_is_weth) };
                 let pool_snapshot = PoolSnapshot { pool_address: pool_addr, dex_type, token0, token1, reserve0: None, reserve1: None, sqrt_price_x96: Some(sqrt_price_x96), tick: Some(tick), last_update_block: None };
                 Ok((pool_state, pool_snapshot))
            }
             DexType::VelodromeV2 | DexType::Aerodrome => {
                 let (reserves_res, token0_res, token1_res, stable_res) = if dex_type == DexType::VelodromeV2 {
                      let pool = VelodromeV2Pool::new(pool_addr, client.clone());
                      tokio::try_join!( pool.get_reserves().call(), pool.token_0().call(), pool.token_1().call(), pool.stable().call() )?
                 } else {
                      let pool = AerodromePool::new(pool_addr, client.clone());
                      tokio::try_join!( pool.get_reserves().call(), pool.token_0().call(), pool.token_1().call(), pool.stable().call() )?
                 };
                 let (reserve0, reserve1, _timestamp) = reserves_res;
                 let (token0, token1, stable) = (token0_res, token1_res, stable_res);
                 let t0_is_weth = token0 == weth_address;
                 let pool_state = PoolState { pool_address: pool_addr, dex_type: dex_type.clone(), token0, token1, uni_fee: None, velo_stable: Some(stable), t0_is_weth: Some(t0_is_weth) };
                 let pool_snapshot = PoolSnapshot { pool_address: pool_addr, dex_type, token0, token1, reserve0: Some(reserve0), reserve1: Some(reserve1), sqrt_price_x96: None, tick: None, last_update_block: None };
                 Ok((pool_state, pool_snapshot))
             }
            DexType::Unknown => Err(eyre!("Cannot fetch state for Unknown DEX type")),
        }
    };

    match timeout(timeout_duration, fetch_future).await {
        Ok(Ok((pool_state, pool_snapshot))) => {
            info!("State fetched successfully. Caching detailed state and snapshot.");
            trace!(?pool_state, ?pool_snapshot);
            app_state.pool_states.insert(pool_addr, pool_state);
            app_state.pool_snapshots.insert(pool_addr, pool_snapshot);
            Ok(())
        }
         Ok(Err(e)) => { error!(pool = %pool_addr, error = ?e, "Failed to fetch pool state (RPC error)"); Err(e.wrap_err("Pool state fetch failed")) } // Wrap error
         Err(_) => { error!(pool = %pool_addr, timeout_secs, "Timeout fetching pool state"); Err(eyre!("Timeout fetching pool state for {}", pool_addr)) }
    }
}

/// Checks if the given token pair matches the target pair, ignoring order.
pub fn is_target_pair_option( addr0: Address, addr1: Address, target_pair: Option<(Address, Address)>, ) -> bool { /* ... unchanged ... */ match target_pair { Some((t_a, t_b)) => (addr0 == t_a && addr1 == t_b) || (addr0 == t_b && addr1 == t_a), None => true } }


// END OF FILE: bot/src/state.rs