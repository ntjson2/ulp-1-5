// bot/src/state.rs

// --- Imports ---
use crate::bindings::{AerodromePool, UniswapV3Pool, VelodromeV2Pool};
use crate::config::Config;
use dashmap::DashMap;
use ethers::{
    prelude::*,
    types::{Address, U256, U64},
};
use eyre::{eyre, Result, WrapErr};
use std::{str::FromStr, sync::Arc};
use tokio::time::{timeout, Duration};
use tracing::{error, info, instrument, trace, warn};

// --- Enums / Structs ---
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DexType {
    UniswapV3,
    VelodromeV2,
    Aerodrome,
    #[allow(dead_code)] // Allow dead code for this variant as it's for robustness
    Unknown,
}
impl DexType {
    pub fn is_velo_style(&self) -> bool {
        matches!(self, DexType::VelodromeV2 | DexType::Aerodrome)
    }
}
impl std::fmt::Display for DexType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}
impl FromStr for DexType {
    type Err = eyre::Report;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "univ3" | "uniswapv3" => Ok(DexType::UniswapV3),
            "velov2" | "velodrome" | "velodromev2" => Ok(DexType::VelodromeV2),
            "aero" | "aerodrome" => Ok(DexType::Aerodrome),
            _ => Err(eyre!("Unknown DEX: {}", s)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PoolState {
    pub pool_address: Address,
    pub dex_type: DexType,
    pub token0: Address,
    #[allow(dead_code)] // Allow dead code for this field, kept for context/future use
    pub token1: Address,
    pub uni_fee: Option<u32>,
    pub velo_stable: Option<bool>,
    pub t0_is_weth: Option<bool>, // Flag indicating if token0 is WETH
    pub factory: Address,
}
#[derive(Debug, Clone)]
pub struct PoolSnapshot {
    pub pool_address: Address,
    pub dex_type: DexType,
    pub token0: Address,
    pub token1: Address,
    pub reserve0: Option<U256>,
    pub reserve1: Option<U256>,
    pub sqrt_price_x96: Option<U256>,
    pub tick: Option<i32>,
    pub last_update_block: Option<U64>,
}
#[derive(Debug, Clone)]
pub struct AppState {
    pub config: Config,
    pub pool_states: Arc<DashMap<Address, PoolState>>, // Detailed, less frequently updated state
    pub pool_snapshots: Arc<DashMap<Address, PoolSnapshot>>, // Minimal, frequently updated state (hot-cache)
    // Commonly used config values cached for quick access
    pub weth_address: Address,
    pub usdc_address: Address,
    pub weth_decimals: u8,
    pub usdc_decimals: u8,
    // FIX: Remove unused fields causing dead_code warnings
    // pub velo_router_addr: Option<Address>,
    // pub aero_router_addr: Option<Address>,
    // pub uni_quoter_addr: Option<Address>,
}

// --- AppState Impl ---
impl AppState {
    pub fn new(config: Config) -> Self {
        Self {
            // Cache frequently accessed config values
            weth_address: config.weth_address,
            usdc_address: config.usdc_address,
            weth_decimals: config.weth_decimals,
            usdc_decimals: config.usdc_decimals,
            // FIX: Remove initialization of unused fields
            // velo_router_addr: Some(config.velo_router_addr),
            // aero_router_addr: config.aerodrome_router_addr,
            // uni_quoter_addr: Some(config.quoter_v2_address),
            // Store the full config
            config, // Keep the full config accessible
            // Initialize state maps
            pool_states: Default::default(),
            pool_snapshots: Default::default(),
        }
    }

    /// Returns the target token pair (WETH, USDC) sorted by address.
    /// Returns None if addresses are not configured (zero address).
    pub fn target_pair(&self) -> Option<(Address, Address)> {
        if self.weth_address.is_zero() || self.usdc_address.is_zero() {
            warn!("WETH or USDC address is zero in config, cannot determine target pair.");
            None
        } else {
            // Always return addresses sorted low->high
            if self.weth_address < self.usdc_address {
                Some((self.weth_address, self.usdc_address))
            } else {
                Some((self.usdc_address, self.weth_address))
            }
        }
    }
}

// --- Helper Functions ---

/// Fetches the detailed state for a given pool and caches it in `pool_states`.
/// Also creates an initial snapshot and caches it in `pool_snapshots`.
/// Handles different DEX types.
#[instrument(skip_all, fields(pool=%pool_addr, dex=?dex_type), level="info")]
pub async fn fetch_and_cache_pool_state(
    pool_addr: Address,
    dex_type: DexType,
    factory_addr: Address, // Pass the factory address that created this pool
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    app_state: Arc<AppState>,
) -> Result<()> {
    info!("Fetching state...");
    let weth_addr = app_state.weth_address; // Cache WETH address locally
    let timeout_dur = Duration::from_secs(app_state.config.fetch_timeout_secs.unwrap_or(15));

    // Define the async block that performs the fetches
    let fetch_logic = async {
        match dex_type {
            DexType::UniswapV3 => {
                let pool = UniswapV3Pool::new(pool_addr, client.clone());
                let slot0_call = pool.slot_0();
                let token0_call = pool.token_0();
                let token1_call = pool.token_1();
                let fee_call = pool.fee();

                let slot0_fut = slot0_call.call();
                let token0_fut = token0_call.call();
                let token1_fut = token1_call.call();
                let fee_fut = fee_call.call();

                let (slot0_res, token0_res, token1_res, fee_res) = tokio::try_join!(
                    slot0_fut, token0_fut, token1_fut, fee_fut
                )?;

                let (sqrtp_u160, tick, ..) = slot0_res;
                let (t0, t1, f) = (token0_res, token1_res, fee_res);
                let is_t0_weth = t0 == weth_addr;
                let sqrtp = U256::from(sqrtp_u160);

                let ps = PoolState {
                    pool_address: pool_addr, dex_type, token0: t0, token1: t1,
                    uni_fee: Some(f), velo_stable: None, t0_is_weth: Some(is_t0_weth),
                    factory: factory_addr,
                };
                let sn = PoolSnapshot {
                    pool_address: pool_addr, dex_type, token0: t0, token1: t1,
                    reserve0: None, reserve1: None, sqrt_price_x96: Some(sqrtp),
                    tick: Some(tick), last_update_block: None,
                };
                Ok((ps, sn))
            }
            DexType::VelodromeV2 | DexType::Aerodrome => {
                 let (reserves_call, token0_call, token1_call, stable_call) =
                    if dex_type == DexType::VelodromeV2 {
                        let p = VelodromeV2Pool::new(pool_addr, client.clone());
                        (p.get_reserves(), p.token_0(), p.token_1(), p.stable())
                    } else {
                        let p = AerodromePool::new(pool_addr, client.clone());
                        (p.get_reserves(), p.token_0(), p.token_1(), p.stable())
                    };

                let reserves_fut = reserves_call.call();
                let token0_fut = token0_call.call();
                let token1_fut = token1_call.call();
                let stable_fut = stable_call.call();

                let (reserves_res, token0_res, token1_res, stable_res) = tokio::try_join!(
                    reserves_fut, token0_fut, token1_fut, stable_fut
                )?;

                let (r0, r1, _block_timestamp_last): (U256, U256, U256) = reserves_res;
                let (t0, t1, s) = (token0_res, token1_res, stable_res);
                let is_t0_weth = t0 == weth_addr;

                let ps = PoolState {
                    pool_address: pool_addr, dex_type, token0: t0, token1: t1,
                    uni_fee: None, velo_stable: Some(s), t0_is_weth: Some(is_t0_weth),
                    factory: factory_addr,
                };
                let sn = PoolSnapshot {
                    pool_address: pool_addr, dex_type, token0: t0, token1: t1,
                    reserve0: Some(r0), reserve1: Some(r1), sqrt_price_x96: None,
                    tick: None, last_update_block: None,
                };
                Ok((ps, sn))
            }
            DexType::Unknown => Err(eyre!("Cannot fetch state for Unknown DEX type")),
        }
    };

    match timeout(timeout_dur, fetch_logic).await {
        Ok(Ok((ps, sn))) => {
            info!("State fetched successfully.");
            trace!(?ps, ?sn);
            app_state.pool_states.insert(pool_addr, ps);
            app_state.pool_snapshots.insert(pool_addr, sn);
            Ok(())
        }
        Ok(Err(e)) => {
            error!(pool = %pool_addr, error = ?e, "Fetch state failed");
            Err(e).wrap_err("Pool state fetch logic failed")
        }
        Err(_) => {
            error!(pool = %pool_addr, timeout_secs = timeout_dur.as_secs(), "Fetch state timeout");
            Err(eyre!(
                "Timeout fetching pool state for {}",
                pool_addr
            ))
        }
    }
}

/// Helper function to check if two token addresses match a target pair, ignoring order.
/// If target is None, always returns true.
pub fn is_target_pair_option(
    a0: Address,
    a1: Address,
    target: Option<(Address, Address)>,
) -> bool {
    match target {
        Some((ta, tb)) => (a0 == ta && a1 == tb) || (a0 == tb && a1 == ta),
        None => true,
    }
}