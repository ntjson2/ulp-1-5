// bot/src/state.rs

// --- Imports ---
use crate::bindings::{AerodromePool, UniswapV3Pool, VelodromeV2Pool};
use crate::config::Config;
use crate::transaction::NonceManager;
use dashmap::DashMap;
use ethers::{
    prelude::*,
    types::{Address, U256, U64},
    utils::parse_units,
};
use eyre::{eyre, Result};
use std::{str::FromStr, sync::Arc};
#[cfg(feature = "local_simulation")]
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration, sleep};
use tracing::{error, info, instrument, trace, warn, debug};

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
    #[cfg(feature = "local_simulation")]
    pub test_arb_check_triggered: Option<Arc<Mutex<bool>>>,
    /// HTTP provider for on-chain reads
    pub http_provider: Provider<Http>,
    /// Signer middleware client for state-changing txns
    pub client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    /// Nonce manager for transaction sequencing
    pub nonce_manager: Arc<NonceManager>,
}

impl AppState {
    pub fn new(
        http_provider: Provider<Http>,
        client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
        nonce_manager: Arc<NonceManager>,
        config: Config,
    ) -> Self {
        Self {
            http_provider,
            client,
            nonce_manager,
            weth_address: config.weth_address,
            usdc_address: config.usdc_address,
            weth_decimals: config.weth_decimals,
            usdc_decimals: config.usdc_decimals,
            config,
            pool_states: Default::default(),
            pool_snapshots: Default::default(),
            #[cfg(feature = "local_simulation")]
            test_arb_check_triggered: None,
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

    #[cfg(feature = "local_simulation")]
    pub fn set_test_arb_check_flag(&mut self, flag: Arc<Mutex<bool>>) {
        self.test_arb_check_triggered = Some(flag);
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
    let call_delay = Duration::from_millis(200);

    let fetch_logic = async {
        match dex_type {
            DexType::UniswapV3 => {
                let pool = UniswapV3Pool::new(pool_addr, client.clone());
                let (mut t0_res, mut t1_res, mut fee_res_val, mut sqrtp_val, mut tick_val) =
                    (Address::zero(), Address::zero(), 0u32, U256::zero(), 0i32);
                let mut success = false;

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
                        t0_res = app_state.weth_address;
                        t1_res = app_state.usdc_address;
                        fee_res_val = 500;
                        sqrtp_val = U256::from_dec_str("33762070975509198366879000000")?;
                        tick_val = 204000;
                        warn!("Using placeholder sqrtP={}, tick={} for local sim fallback for pool {}", sqrtp_val, tick_val, pool_addr);
                        success = true;
                    }
                    #[cfg(not(feature = "local_simulation"))]
                    {
                         (sqrtp_val, tick_val, ..) = pool.slot_0().call().await.map_err(|e| eyre!(e).wrap_err("Failed slot0 call even in non-sim"))?;
                         if t0_res.is_zero() { t0_res = pool.token_0().call().await.map_err(|e| eyre!(e).wrap_err("Failed token0 call"))?; sleep(call_delay).await; }
                         if t1_res.is_zero() { t1_res = pool.token_1().call().await.map_err(|e| eyre!(e).wrap_err("Failed token1 call"))?; sleep(call_delay).await; }
                         if fee_res_val == 0 { fee_res_val = pool.fee().call().await.map_err(|e| eyre!(e).wrap_err("Failed fee call"))?; }
                         if t0_res.is_zero() || t1_res.is_zero() {
                            eyre::bail!("Failed to fetch all required UniV3 pool data for {} after individual attempts.", pool_addr);
                         }
                         success = true;
                    }
                }
                if !success {
                    eyre::bail!("Failed to obtain all necessary UniV3 data for pool {}", pool_addr);
                }

                let is_t0_weth = t0_res == weth_addr;
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
                let (mut r0, mut r1, mut t0, mut t1, mut s_res) =
                    (U256::zero(), U256::zero(), Address::zero(), Address::zero(), false);
                let mut success = false;

                let p_reserves = if dex_type == DexType::VelodromeV2 {
                    VelodromeV2Pool::new(pool_addr, client.clone()).get_reserves().call().await
                } else {
                    AerodromePool::new(pool_addr, client.clone()).get_reserves().call().await
                };

                if let Ok(reserves_data) = p_reserves {
                    sleep(call_delay).await;
                    let p_token0 = if dex_type == DexType::VelodromeV2 { VelodromeV2Pool::new(pool_addr, client.clone()).token_0().call().await } else { AerodromePool::new(pool_addr, client.clone()).token_0().call().await };
                    if let Ok(token0_data) = p_token0 {
                        sleep(call_delay).await;
                        let p_token1 = if dex_type == DexType::VelodromeV2 { VelodromeV2Pool::new(pool_addr, client.clone()).token_1().call().await } else { AerodromePool::new(pool_addr, client.clone()).token_1().call().await };
                        if let Ok(token1_data) = p_token1 {
                            sleep(call_delay).await;
                             let p_stable = if dex_type == DexType::VelodromeV2 { VelodromeV2Pool::new(pool_addr, client.clone()).stable().call().await } else { AerodromePool::new(pool_addr, client.clone()).stable().call().await };
                             if let Ok(stable_data) = p_stable {
                                (r0, r1, _) = reserves_data;
                                t0 = token0_data;
                                t1 = token1_data;
                                s_res = stable_data;
                                success = true;
                                info!("Successfully fetched {:?} pool data via direct calls for {}", dex_type, pool_addr);
                             }
                        }
                    }
                }

                if !success {
                    #[cfg(feature = "local_simulation")]
                    {
                        warn!("LOCAL SIMULATION: Direct {:?} pool calls failed for {}. Using fallback.", dex_type, pool_addr);
                        t0 = app_state.weth_address;
                        t1 = app_state.usdc_address;
                        if t0 == app_state.weth_address {
                            r0 = parse_units("100", app_state.weth_decimals as u32)?.into(); // Cast u8 to u32
                            r1 = parse_units("200000", app_state.usdc_decimals as u32)?.into(); // Cast u8 to u32
                        } else {
                            r0 = parse_units("200000", app_state.usdc_decimals as u32)?.into(); // Cast u8 to u32
                            r1 = parse_units("100", app_state.weth_decimals as u32)?.into(); // Cast u8 to u32
                        }
                        s_res = true;
                        warn!("Using placeholder reserves r0={}, r1={}, stable={} for local sim fallback for pool {}", r0, r1, s_res, pool_addr);
                        success = true;
                    }
                    #[cfg(not(feature = "local_simulation"))]
                    {
                         let (rsv0, rsv1, _) = if dex_type == DexType::VelodromeV2 { VelodromeV2Pool::new(pool_addr, client.clone()).get_reserves().call().await.map_err(|e| eyre!(e))? } else { AerodromePool::new(pool_addr, client.clone()).get_reserves().call().await.map_err(|e| eyre!(e))? };
                         r0 = rsv0; r1 = rsv1;
                         sleep(call_delay).await;
                         t0 = if dex_type == DexType::VelodromeV2 { VelodromeV2Pool::new(pool_addr, client.clone()).token_0().call().await.map_err(|e| eyre!(e))? } else { AerodromePool::new(pool_addr, client.clone()).token_0().call().await.map_err(|e| eyre!(e))? };
                         sleep(call_delay).await;
                         t1 = if dex_type == DexType::VelodromeV2 { VelodromeV2Pool::new(pool_addr, client.clone()).token_1().call().await.map_err(|e| eyre!(e))? } else { AerodromePool::new(pool_addr, client.clone()).token_1().call().await.map_err(|e| eyre!(e))? };
                         sleep(call_delay).await;
                         s_res = if dex_type == DexType::VelodromeV2 { VelodromeV2Pool::new(pool_addr, client.clone()).stable().call().await.map_err(|e| eyre!(e))? } else { AerodromePool::new(pool_addr, client.clone()).stable().call().await.map_err(|e| eyre!(e))? };
                         success = true;
                    }
                }
                 if !success {
                    eyre::bail!("Failed to obtain all necessary {:?} data for pool {}", dex_type, pool_addr);
                }

                let is_t0_weth = t0 == weth_addr;
                let ps = PoolState {
                    pool_address: pool_addr, dex_type, token0: t0, token1: t1,
                    uni_fee: None, velo_stable: Some(s_res), t0_is_weth: Some(is_t0_weth),
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
            let err_msg = format!("Fetch state logic failed for pool {} ({:?}): {:?}", pool_addr, dex_type, e);
            error!("{}", err_msg);
            Err(eyre!(err_msg))
        }
        Err(_) => {
            let err_msg = format!("Timeout fetching pool state for {} ({:?}) after {}s", pool_addr, dex_type, timeout_dur.as_secs());
            error!("{}", err_msg);
            Err(eyre!(err_msg))
        }
    }
}

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