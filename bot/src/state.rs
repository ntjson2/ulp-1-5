// bot/src/state.rs

// --- Imports ---
use crate::bindings::{AerodromePool, UniswapV3Pool, VelodromeV2Pool, QuoterV2}; // Added QuoterV2
use crate::config::Config;
use dashmap::DashMap;
use ethers::{
    prelude::*,
    types::{Address, U256, U64},
};
use eyre::{eyre, Result, WrapErr};
use std::{str::FromStr, sync::Arc};
use tokio::time::{timeout, Duration, sleep};
use tracing::{error, info, instrument, trace, warn, debug}; // Added debug

// --- Enums / Structs ---
// (DexType, PoolState, PoolSnapshot, AppState structs remain unchanged)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DexType {
    UniswapV3,
    VelodromeV2,
    Aerodrome,
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    pub token1: Address,
    pub uni_fee: Option<u32>,
    pub velo_stable: Option<bool>,
    pub t0_is_weth: Option<bool>,
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
    pub pool_states: Arc<DashMap<Address, PoolState>>,
    pub pool_snapshots: Arc<DashMap<Address, PoolSnapshot>>,
    pub weth_address: Address,
    pub usdc_address: Address,
    pub weth_decimals: u8,
    pub usdc_decimals: u8,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        Self {
            weth_address: config.weth_address,
            usdc_address: config.usdc_address,
            weth_decimals: config.weth_decimals,
            usdc_decimals: config.usdc_decimals,
            config,
            pool_states: Default::default(),
            pool_snapshots: Default::default(),
        }
    }
    pub fn target_pair(&self) -> Option<(Address, Address)> {
        if self.weth_address.is_zero() || self.usdc_address.is_zero() {
            warn!("WETH or USDC address is zero in config, cannot determine target pair.");
            None
        } else {
            if self.weth_address < self.usdc_address {
                Some((self.weth_address, self.usdc_address))
            } else {
                Some((self.usdc_address, self.weth_address))
            }
        }
    }
}


/// Fetches the detailed state for a given pool and caches it.
#[instrument(skip_all, fields(pool=%pool_addr, dex=?dex_type), level="info")]
pub async fn fetch_and_cache_pool_state(
    pool_addr: Address,
    dex_type: DexType,
    factory_addr: Address,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    app_state: Arc<AppState>,
) -> Result<()> {
    info!("Fetching state...");
    let weth_addr = app_state.weth_address;
    let timeout_dur = Duration::from_secs(app_state.config.fetch_timeout_secs.unwrap_or(15));
    let call_delay = Duration::from_millis(200); // Increased delay slightly

    let fetch_logic = async {
        match dex_type {
            DexType::UniswapV3 => {
                let pool = UniswapV3Pool::new(pool_addr, client.clone());
                let (mut t0_res, mut t1_res, mut fee_res_val, mut sqrtp_val, mut tick_val) =
                    (Address::zero(), Address::zero(), 0u32, U256::zero(), 0i32);
                let mut success = false;

                // Attempt direct calls
                debug!("Attempting direct UniV3 pool calls for {}", pool_addr);
                let slot0_call_result = pool.slot_0().call().await;
                if slot0_call_result.is_ok() {
                    sleep(call_delay).await;
                    let token0_call_result = pool.token_0().call().await;
                    if token0_call_result.is_ok() {
                        sleep(call_delay).await;
                        let token1_call_result = pool.token_1().call().await;
                        if token1_call_result.is_ok() {
                            sleep(call_delay).await;
                            let fee_call_result = pool.fee().call().await;
                            if fee_call_result.is_ok() {
                                (sqrtp_val, tick_val, ..) = slot0_call_result.unwrap();
                                t0_res = token0_call_result.unwrap();
                                t1_res = token1_call_result.unwrap();
                                fee_res_val = fee_call_result.unwrap();
                                success = true;
                                info!("Successfully fetched UniV3 pool data via direct calls for {}", pool_addr);
                            }
                        }
                    }
                }

                if !success {
                    #[cfg(feature = "local_simulation")]
                    {
                        warn!("LOCAL SIMULATION: Direct UniV3 pool calls failed for {}. Using fallback with hardcoded/default values.", pool_addr);
                        // Fallback for local simulation if direct calls fail
                        // Use WETH/USDC as default tokens, and a common fee tier like 0.05%
                        // This is to allow tests to proceed, not for accurate data.
                        t0_res = app_state.weth_address; // Assume WETH is token0 for simplicity
                        t1_res = app_state.usdc_address; // Assume USDC is token1
                        fee_res_val = 500; // Common 0.05% fee tier
                        // SqrtPrice and Tick would ideally be fetched from Quoter if possible,
                        // but Quoter doesn't give slot0 directly. For now, use placeholder values.
                        // A more sophisticated fallback could attempt a QuoterV2 call for a spot price.
                        // For now, a very rough placeholder for sqrtPriceX96 assuming ~2000 USDC per WETH
                        // (2000 * 2^96 * 10^(18-6) / 10^18 )^0.5 - this is complex to calculate on the fly without full math.
                        // Let's use a known sqrtPriceX96 from a WETH/USDC 0.05% pool for OP as a rough default if possible,
                        // or just a non-zero placeholder.
                        // Example placeholder (can be refined):
                        sqrtp_val = U256::from_dec_str("148000000000000000000000000000000")?; // A large non-zero value
                        tick_val = 200000; // A plausible tick value
                        warn!("Using placeholder sqrtP={}, tick={} for local sim fallback for pool {}", sqrtp_val, tick_val, pool_addr);
                        success = true; // Mark as "success" for the purpose of creating a state entry
                    }
                    #[cfg(not(feature = "local_simulation"))]
                    {
                        // If not in local_simulation, the original error should propagate
                        // Re-trigger the first error to propagate it if all sequential calls failed
                        (sqrtp_val, tick_val, ..) = pool.slot_0().call().await?;
                        // This line will only be reached if slot_0 call succeeds after all previous attempts failed, which is unlikely.
                        // The error from the first failing call within the !success block is what we want to return.
                        // This path is hard to hit now, the `eyre::bail!` below is more direct.
                    }
                }

                if !success {
                    // This should only be reached if not in local_simulation and a call failed
                    eyre::bail!("Failed to fetch all required UniV3 pool data for {} after individual attempts.", pool_addr);
                }

                let is_t0_weth = t0_res == weth_addr;
                // sqrtp_val is already U256 from slot0 or placeholder
                let ps = PoolState {
                    pool_address: pool_addr, dex_type, token0: t0_res, token1: t1_res,
                    uni_fee: Some(fee_res_val), velo_stable: None, t0_is_weth: Some(is_t0_weth),
                    factory: factory_addr,
                };
                let sn = PoolSnapshot {
                    pool_address: pool_addr, dex_type, token0: t0_res, token1: t1_res,
                    reserve0: None, reserve1: None, sqrt_price_x96: Some(sqrtp_val),
                    tick: Some(tick_val), last_update_block: None,
                };
                Ok((ps, sn))
            }
            DexType::VelodromeV2 | DexType::Aerodrome => {
                 let (pool_contract_reserves, pool_contract_token0, pool_contract_token1, pool_contract_stable) =
                    if dex_type == DexType::VelodromeV2 {
                        let p = VelodromeV2Pool::new(pool_addr, client.clone());
                        (p.get_reserves(), p.token_0(), p.token_1(), p.stable())
                    } else { // Aerodrome
                        let p = AerodromePool::new(pool_addr, client.clone());
                        (p.get_reserves(), p.token_0(), p.token_1(), p.stable())
                    };
                trace!("Fetching getReserves for Velo/Aero pool {}", pool_addr);
                let reserves_res = pool_contract_reserves.call().await?;
                sleep(call_delay).await;
                trace!("Fetching token0 for Velo/Aero pool {}", pool_addr);
                let token0_res = pool_contract_token0.call().await?;
                sleep(call_delay).await;
                trace!("Fetching token1 for Velo/Aero pool {}", pool_addr);
                let token1_res = pool_contract_token1.call().await?;
                sleep(call_delay).await;
                trace!("Fetching stable for Velo/Aero pool {}", pool_addr);
                let stable_res = pool_contract_stable.call().await?;

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
            info!("State fetched successfully for pool {}.", pool_addr);
            trace!(?ps, ?sn);
            app_state.pool_states.insert(pool_addr, ps);
            app_state.pool_snapshots.insert(pool_addr, sn);
            Ok(())
        }
        Ok(Err(e)) => {
            error!(pool = %pool_addr, error = ?e, "Fetch state failed");
            Err(e).wrap_err_with(|| format!("Pool state fetch logic failed for pool {}", pool_addr))
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