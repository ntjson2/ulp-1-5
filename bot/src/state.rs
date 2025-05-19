// bot/src/state.rs

// --- Imports ---
use crate::{
    bindings::{AerodromePool, UniswapV3Pool, VelodromeV2Pool, // UniswapV3Pool is the contract struct
        // Assuming abigen created a module like `i_uniswap_v3_pool` for the contract
        // if the contract name in abigen! was IUniswapV3Pool.
        // Or if the contract name was UniswapV3Pool and it generated `uniswap_v3_pool_contract` module.
        // Given the error "no Slot0Output in bindings::uniswap_v3_pool",
        // it implies `uniswap_v3_pool` is a module.
        // Let's try to be very specific if the contract is named IUniswapV3Pool in abigen
        i_uniswap_v3_pool::Slot0Output, // Trying this path
    },
    config::Config,
    transaction::NonceManager,
};
use dashmap::DashMap;
use ethers::{
    core::types::{Address, U256},
    middleware::SignerMiddleware,
    providers::{Http, Provider}, 
    signers::LocalWallet, // Removed unused Signer import
};
use eyre::{eyre, Result, WrapErr};
// use futures_util::TryFutureExt; // Marked as unused in last log
use std::sync::Arc;
use std::str::FromStr;
use tokio::time::Duration; 
use tracing::instrument;

#[derive(Debug, Clone)]
pub struct PoolState {
    pub pool_address: Address,
    pub dex_type: DexType,
    pub token0: Address,
    pub token1: Address,
    pub factory: Option<Address>, // Renamed from factory_address
    // Fields like reserve0, reserve1, sqrt_price_x96, tick were causing E0560,
    // implying they are not part of PoolState. They are in PoolSnapshot.
    // If they are needed in PoolState, the struct definition must be updated.
    // For now, assuming they are not in PoolState based on the error "available fields are: factory".
    // This means the assignments to these fields in fetch_and_cache_pool_state for PoolState were incorrect.
    pub uni_fee: Option<u32>, // Changed from U256 to u32 based on E0308
    pub velo_stable: Option<bool>,
    pub t0_is_weth: Option<bool>,
    // last_update_block was also causing E0560 for PoolState.
}

#[derive(Debug, Clone, Default)]
pub struct PoolSnapshot {
    pub pool_address: Address,
    pub dex_type: DexType,
    pub token0: Address,
    pub token1: Address,
    pub reserve0: Option<U256>,
    pub reserve1: Option<U256>,
    pub sqrt_price_x96: Option<U256>, // For UniV3
    pub tick: Option<i32>,            // For UniV3
    // pub uni_fee: Option<U256>, // Field not found in error E0560, assuming removed or moved to PoolState
    // pub velo_stable: Option<bool>, // Field not found in error E0560
    // pub t0_is_weth: Option<bool>, // Field not found in error E0560
    pub last_update_block: Option<U256>,
}

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
            "uniswapv3" | "univ3" => Ok(DexType::UniswapV3),
            "velodrome" | "velov2" => Ok(DexType::VelodromeV2),
            "aerodrome" | "aero" => Ok(DexType::Aerodrome),
            _ => Err(eyre!("Unknown DEX type string: {}", s)),
        }
    }
}

impl Default for DexType {
    fn default() -> Self {
        DexType::Unknown // Or UniswapV3, or whatever makes sense as a default
    }
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub config: Config,
    pub client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    pub http_provider: Arc<Provider<Http>>,
    pub pool_states: Arc<DashMap<Address, PoolState>>,
    pub pool_snapshots: Arc<DashMap<Address, PoolSnapshot>>,
    pub nonce_manager: Arc<NonceManager>,
    #[cfg(feature = "local_simulation")]
    pub test_arb_check_triggered: bool,
}

impl AppState {
    pub fn new(
        config: Config,
        client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
        http_provider: Arc<Provider<Http>>,
        nonce_manager: Arc<NonceManager>, // Added NonceManager to constructor
    ) -> Self {
        Self {
            config,
            client,
            http_provider,
            pool_states: Arc::new(DashMap::new()),
            pool_snapshots: Arc::new(DashMap::new()),
            nonce_manager,
            #[cfg(feature = "local_simulation")]
            test_arb_check_triggered: false,
        }
    }

    pub fn target_pair(&self) -> Option<(Address, Address)> {
        // Assuming Config struct has a method or logic to determine the target pair
        // If target_pair is defined directly on Config, self.config.target_pair() is correct.
        // If it's based on weth_address and usdc_address from config:
        if self.config.weth_address != Address::zero() && self.config.usdc_address != Address::zero() {
            Some((self.config.weth_address, self.config.usdc_address))
        } else {
            None
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        let config = Config::default();
        // Ensure `rand = "0.8.5"` is in Cargo.toml for this to work
        let wallet = LocalWallet::new(&mut rand::thread_rng());
        // let wallet_pk_hex = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        // let wallet = LocalWallet::from_str(wallet_pk_hex)
        //     .expect("Failed to create default wallet from PK")
        //     .with_chain_id(config.chain_id.unwrap_or(1u64)); // Provide default if Option is None

        let provider = Provider::<Http>::try_from(config.http_rpc_url.clone()).expect("Failed to connect to HTTP provider for default AppState");
        let client = Arc::new(SignerMiddleware::new(provider.clone(), wallet));
        let http_provider = Arc::new(provider);
        
        let nonce_manager = Arc::new(NonceManager::new(client.address()));


        Self {
            config,
            client,
            http_provider,
            pool_states: Arc::new(DashMap::new()),
            pool_snapshots: Arc::new(DashMap::new()),
            nonce_manager,
            #[cfg(feature = "local_simulation")]
            test_arb_check_triggered: false,
        }
    }
}

/// Fetches the detailed state for a given pool and caches it.
#[instrument(skip_all, fields(pool=%pool_addr, dex=?dex_type), level="info")]
pub async fn fetch_and_cache_pool_state(
    pool_addr: Address,
    dex_type: DexType,
    factory_address: Address,
    rpc_client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    app_state: Arc<AppState>,
) -> Result<()> {
    let mut r0 = U256::zero();
    let mut r1 = U256::zero();
    let mut t0 = Address::zero();
    let mut t1 = Address::zero();
    let mut s_res = false;
    let mut fee_res_u32 = 0u32; // For UniV3 fee, type u32

    match dex_type {
        DexType::UniswapV3 => {
            let pool_contract = UniswapV3Pool::new(pool_addr, rpc_client.clone());
            // Fetch token0, token1, fee, and slot0 concurrently
            let (t0_res, t1_res, fee_res, slot0_data): (Address, Address, u32, Slot0Output) = tokio::try_join!(
                pool_contract.token_0().call(),
                pool_contract.token_1().call(),
                pool_contract.fee().call(),
                pool_contract.slot_0().call() // This returns Slot0Output directly
            )
            .wrap_err_with(|| format!("Failed to fetch UniswapV3 pool details for {}", pool_addr))?;

            t0 = t0_res;
            t1 = t1_res;
            fee_res_u32 = fee_res;
            let sqrt_price_x96_val = slot0_data.sqrt_price_x96; // Accessing the sqrt_price_x96 field from Slot0Output
            let tick_val = slot0_data.tick; // Accessing the tick field from Slot0Output

            r0 = sqrt_price_x96_val; // Store sqrt_price_x96 for snapshot
            // r1 = tick_val.into(); // tick is i32, not directly convertible to U256 for r1. Snapshot has separate tick field.

            let weth_addr = app_state.config.weth_address;
            let is_t0_weth = t0 == weth_addr;

            let pool_state = PoolState {
                pool_address: pool_addr,
                dex_type,
                token0: t0,
                token1: t1,
                factory: Some(factory_address),
                uni_fee: Some(fee_res_u32),
                velo_stable: None,
                t0_is_weth: Some(is_t0_weth),
                // Removed fields not in PoolState: reserve0, reserve1, sqrt_price_x96, tick, last_update_block
            };
            let pool_snapshot = PoolSnapshot {
                pool_address: pool_addr,
                dex_type,
                token0: t0,
                token1: t1,
                reserve0: None, // UniV3 snapshots don't use reserves directly here
                reserve1: None,
                sqrt_price_x96: Some(sqrt_price_x96_val),
                tick: Some(tick_val),
                last_update_block: None,
            };
            app_state.pool_states.insert(pool_addr, pool_state);
            app_state.pool_snapshots.insert(pool_addr, pool_snapshot);
            Ok(())
        }
        DexType::VelodromeV2 | DexType::Aerodrome => {
            let mut success = false;
            let mut attempts = 0;
            let max_attempts = 3;
            let call_delay = Duration::from_millis(200);

            while !success && attempts < max_attempts {
                attempts += 1;
                let client = rpc_client.clone();

                #[cfg(feature = "local_simulation")]
                {
                    let (rsv0_sim, rsv1_sim, _) = if dex_type == DexType::VelodromeV2 {
                        VelodromeV2Pool::new(pool_addr, client.clone()).get_reserves().call().await.map_err(|e| eyre!(e))?
                    } else {
                        AerodromePool::new(pool_addr, client.clone()).get_reserves().call().await.map_err(|e| eyre!(e))?
                    };
                    r0 = rsv0_sim;
                    r1 = rsv1_sim;
                    t0 = if dex_type == DexType::VelodromeV2 {
                        VelodromeV2Pool::new(pool_addr, client.clone()).token_0().call().await.map_err(|e| eyre!(e))?
                    } else {
                        AerodromePool::new(pool_addr, client.clone()).token_0().call().await.map_err(|e| eyre!(e))?
                    };
                    t1 = if dex_type == DexType::VelodromeV2 {
                        VelodromeV2Pool::new(pool_addr, client.clone()).token_1().call().await.map_err(|e| eyre!(e))?
                    } else {
                        AerodromePool::new(pool_addr, client.clone()).token_1().call().await.map_err(|e| eyre!(e))?
                    };
                    s_res = if dex_type == DexType::VelodromeV2 {
                        VelodromeV2Pool::new(pool_addr, client.clone()).stable().call().await.map_err(|e| eyre!(e))?
                    } else {
                        AerodromePool::new(pool_addr, client.clone()).stable().call().await.map_err(|e| eyre!(e))?
                    };
                    success = true;
                }
                #[cfg(not(feature = "local_simulation"))]
                {
                    let (rsv0_live, rsv1_live, _) = if dex_type == DexType::VelodromeV2 {
                        VelodromeV2Pool::new(pool_addr, client.clone()).get_reserves().call().await.map_err(|e| eyre!(e))?
                    } else {
                        AerodromePool::new(pool_addr, client.clone()).get_reserves().call().await.map_err(|e| eyre!(e))?
                    };
                    r0 = rsv0_live;
                    r1 = rsv1_live;
                    tokio::time::sleep(call_delay).await; // Use tokio::time::sleep
                    t0 = if dex_type == DexType::VelodromeV2 { VelodromeV2Pool::new(pool_addr, client.clone()).token_0().call().await.map_err(|e| eyre!(e))? } else { AerodromePool::new(pool_addr, client.clone()).token_0().call().await.map_err(|e| eyre!(e))? };
                    tokio::time::sleep(call_delay).await; // Use tokio::time::sleep
                    t1 = if dex_type == DexType::VelodromeV2 { VelodromeV2Pool::new(pool_addr, client.clone()).token_1().call().await.map_err(|e| eyre!(e))? } else { AerodromePool::new(pool_addr, client.clone()).token_1().call().await.map_err(|e| eyre!(e))? };
                    tokio::time::sleep(call_delay).await; // Use tokio::time::sleep
                    s_res = if dex_type == DexType::VelodromeV2 { VelodromeV2Pool::new(pool_addr, client.clone()).stable().call().await.map_err(|e| eyre!(e))? } else { AerodromePool::new(pool_addr, client.clone()).stable().call().await.map_err(|e| eyre!(e))? };
                    success = true;
                }
            }

            if !success {
                return Err(eyre!(
                    "Failed to fetch Velo/Aero pool details after {} attempts for {}",
                    max_attempts,
                    pool_addr
                ));
            }

            let weth_addr = app_state.config.weth_address;
            let is_t0_weth = t0 == weth_addr;

            let pool_state = PoolState {
                pool_address: pool_addr,
                dex_type,
                token0: t0,
                token1: t1,
                factory: Some(factory_address),
                // Removed fields not in PoolState: reserve0, reserve1, sqrt_price_x96, tick
                uni_fee: None,
                velo_stable: Some(s_res),
                t0_is_weth: Some(is_t0_weth),
                // Removed last_update_block
            };
            let pool_snapshot = PoolSnapshot {
                pool_address: pool_addr,
                dex_type,
                token0: t0,
                token1: t1,
                reserve0: Some(r0),
                reserve1: Some(r1),
                sqrt_price_x96: None,
                tick: None,
                // Removed fields not present in PoolSnapshot based on E0560
                // uni_fee: None, 
                // velo_stable: Some(s_res), 
                // t0_is_weth: Some(is_t0_weth),
                last_update_block: None,
            };
            app_state.pool_states.insert(pool_addr, pool_state);
            app_state.pool_snapshots.insert(pool_addr, pool_snapshot);
            Ok(())
        }
        DexType::Unknown => Err(eyre!("Cannot fetch state for Unknown DEX type")),
    }
}

pub fn is_target_pair_option(token0: Address, token1: Address, target_pair_opt: Option<(Address, Address)>) -> bool {
    match target_pair_opt {
        Some((ta, tb)) => (token0 == ta && token1 == tb) || (token0 == tb && token1 == ta),
        None => true,
    }
}