// bot/src/state.rs

// --- Imports ---
use crate::{
    config::Config,
    bindings::{
        // Assuming abigen creates modules for each contract
        i_uniswap_v3_pool::Slot0Output, // Assuming Slot0Output is here
        uniswap_v3_pool::UniswapV3Pool, // Assuming UniswapV3Pool is here
        velodrome_v2_pool::VelodromeV2Pool, // Added
        aerodrome_pool::AerodromePool, // Added
    },
    transaction::NonceManager, 
};
use ethers::{
    core::types::{Address, BlockNumber, Filter, Log, H160, H256, U256, U64},
    middleware::SignerMiddleware,
    providers::{Middleware, Provider, StreamExt, Ws, Http}, // Added Http
    signers::LocalWallet, // Added LocalWallet
    utils::keccak256,
};
use eyre::{eyre, Result, Report};
use futures_util::StreamExt;
use std::{sync::Arc, str::FromStr, time::Duration}; 
use tokio::sync::RwLock;
use tracing::{debug, info, instrument, warn}; // Removed error if not used

#[derive(Debug, Clone)]
pub struct PoolState {
    pub pool_address: Address,
    pub dex_type: DexType,
    pub token0: Address,
    pub token1: Address,
    pub factory: Option<Address>, 
    pub uni_fee: Option<u32>, 
    pub velo_stable: Option<bool>,
    pub t0_is_weth: Option<bool>,
}

#[derive(Debug, Clone, Default)]
pub struct PoolSnapshot {
    pub pool_address: Address,
    pub dex_type: DexType,
    pub token0: Address,
    pub token1: Address,
    pub reserve0: Option<U256>,
    pub reserve1: Option<U256>,
    pub sqrt_price_x96: Option<U256>, 
    pub tick: Option<i32>,            
    pub last_update_block: Option<U256>, // Changed from Option<u64> to Option<U256> to match usage
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DexType {
    UniswapV3,
    VelodromeV2,
    Aerodrome,
    Balancer, // Added Balancer
    #[allow(dead_code)] // Keep Unknown if it's a valid state, otherwise remove if not used
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
            "balancer" => Ok(DexType::Balancer), // Added Balancer
            _ => Err(eyre!("Unknown DEX type string: {}", s)),
        }
    }
}

impl Default for DexType {
    fn default() -> Self {
        DexType::Unknown // Or UniswapV3, or whatever makes sense as a default
    }
}

#[derive(Debug)] // Removed Clone because std::sync::Mutex is not Clone
pub struct AppState {
    pub config: Arc<Config>,
    pub known_pools: Arc<DashMap<Address, PoolState>>, // Changed from PoolInfo to PoolState based on usage
    pub pool_snapshots: Arc<DashMap<Address, PoolSnapshot>>,
    pub client_ws: Arc<SignerMiddleware<Provider<Ws>, LocalWallet>>, // Added LocalWallet
    pub client_http: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, // Added Http and LocalWallet
    pub nonce_manager: Arc<NonceManager>,
    pub path_optimizer: Arc<crate::path_optimizer::PathOptimizer>, // Ensure PathOptimizer is pub
    pub test_arb_check_triggered: Arc<RwLock<bool>>, // For testing
}

impl Clone for AppState {
    fn clone(&self) -> Self {
        Self {
            config: Arc::clone(&self.config),
            known_pools: Arc::clone(&self.known_pools),
            pool_snapshots: Arc::clone(&self.pool_snapshots),
            client_ws: Arc::clone(&self.client_ws),
            client_http: Arc::clone(&self.client_http),
            nonce_manager: Arc::clone(&self.nonce_manager),
            path_optimizer: Arc::clone(&self.path_optimizer),
            test_arb_check_triggered: Arc::clone(&self.test_arb_check_triggered),
        }
    }
}


impl AppState {
    pub async fn new(
        config: Config,
        client_ws: Arc<SignerMiddleware<Provider<Ws>, LocalWallet>>, // Added LocalWallet
        client_http: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, // Added Http and LocalWallet
        nonce_manager: Arc<NonceManager>,
        path_optimizer: Arc<crate::path_optimizer::PathOptimizer>,
    ) -> Result<Self> {
        let pool_states = Arc::new(DashMap::<Address, PoolState>::new());
        let pool_snapshots = Arc::new(DashMap::<Address, PoolSnapshot>::new());
        Ok(Self {
            config: Arc::new(config),
            known_pools: pool_states,
            pool_snapshots,
            client_ws,
            client_http,
            nonce_manager,
            path_optimizer,
            test_arb_check_triggered: Arc::new(RwLock::new(false)),
        })
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
        let wallet = LocalWallet::new(&mut rand::thread_rng())
            .with_chain_id(config.chain_id.unwrap_or(1u64)); // Signer trait for with_chain_id

        // Correct way to connect WS provider and await it
        let ws_provider_future = Provider::<Ws>::connect(&config.ws_rpc_url);
        // In a real async context you would .await here. For Default, this is tricky.
        // For simplicity in Default, we might block or panic.
        // Or, Default for AppState might not be suitable if it needs async setup.
        // Assuming for now this is part of a test setup or similar where blocking is acceptable.
        let ws_provider = tokio::runtime::Runtime::new().unwrap().block_on(ws_provider_future)
            .expect("Failed to connect to WS provider for default AppState");

        let client_ws = Arc::new(SignerMiddleware::new(ws_provider, wallet.clone()));
        
        let http_provider_instance = Provider::<Http>::try_from(config.http_rpc_url.clone())
            .expect("Failed to connect to HTTP provider for default AppState");
        let client_http = Arc::new(SignerMiddleware::new(http_provider_instance, wallet.clone())); 
        
        let nonce_manager = Arc::new(NonceManager::new(wallet.address()));

        // Initialize last_block_processed
        // This would ideally be async, but Default is sync.
        // Setting to 0 or a placeholder.
        // Or, Default for AppState might not be suitable if it needs async setup.
        // Assuming for now this is part of a test setup or similar where blocking is acceptable.
        let last_block = config.initial_block_history_to_scan.unwrap_or(0);


        Self {
            config: Arc::new(config),
            known_pools: Arc::new(DashMap::new()),
            pool_snapshots: Arc::new(DashMap::new()),
            client_ws,
            client_http, 
            nonce_manager,
            path_optimizer: Arc::new(crate::path_optimizer::PathOptimizer::new(Arc::new(config.clone()))),
            test_arb_check_triggered: Arc::new(RwLock::new(false)), 
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
            // Assuming UniswapV3Pool is generated in its own module by abigen!
            let pool_contract = crate::bindings::uniswap_v3_pool::UniswapV3Pool::new(pool_addr, rpc_client.clone());
            
            let token0_call = pool_contract.token_0().call();
            let token1_call = pool_contract.token_1().call();
            let fee_call = pool_contract.fee().call();
            // Assuming Slot0Output is generated in i_uniswap_v3_pool module
            let slot0_call = pool_contract.slot_0().call(); 
            
            let (t0_res, t1_res, fee_res, slot0_data_val): (Address, Address, u32, crate::bindings::i_uniswap_v3_pool::Slot0Output) = 
                tokio::try_join!(token0_call, token1_call, fee_call, slot0_call)
                .map_err(|e| eyre!("Failed to fetch UniswapV3 pool details via try_join: {:?}", e))?;

            t0 = t0_res;
            t1 = t1_res;
            fee_res_u32 = fee_res;
            let sqrt_price_x96_val = slot0_data_val.sqrt_price_x96; 
            let tick_val = slot0_data_val.tick; // Accessing the tick field from Slot0Output

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
            app_state.known_pools.insert(pool_addr, pool_state);
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
                    let client_clone_for_call = client.clone(); // client is rpc_client here
                    let (rsv0_sim, rsv1_sim, _): (U256, U256, U256) = if dex_type == DexType::VelodromeV2 {
                        VelodromeV2Pool::new(pool_addr, client_clone_for_call.clone()).get_reserves().call().await.map_err(|e: ethers::contract::ContractError<_>| eyre!(e))?
                    } else {
                        AerodromePool::new(pool_addr, client_clone_for_call.clone()).get_reserves().call().await.map_err(|e: ethers::contract::ContractError<_>| eyre!(e))?
                    };
                    r0 = rsv0_sim;
                    r1 = rsv1_sim;
                    t0 = if dex_type == DexType::VelodromeV2 {
                        VelodromeV2Pool::new(pool_addr, client_clone_for_call.clone()).token_0().call().await.map_err(|e: ethers::contract::ContractError<_>| eyre!(e))?
                    } else {
                        AerodromePool::new(pool_addr, client_clone_for_call.clone()).token_0().call().await.map_err(|e: ethers::contract::ContractError<_>| eyre!(e))?
                    };
                    t1 = if dex_type == DexType::VelodromeV2 {
                        VelodromeV2Pool::new(pool_addr, client_clone_for_call.clone()).token_1().call().await.map_err(|e: ethers::contract::ContractError<_>| eyre!(e))?
                    } else {
                        AerodromePool::new(pool_addr, client_clone_for_call.clone()).token_1().call().await.map_err(|e: ethers::contract::ContractError<_>| eyre!(e))?
                    };
                    s_res = if dex_type == DexType::VelodromeV2 {
                        VelodromeV2Pool::new(pool_addr, client_clone_for_call.clone()).stable().call().await.map_err(|e: ethers::contract::ContractError<_>| eyre!(e))?
                    } else {
                        AerodromePool::new(pool_addr, client_clone_for_call.clone()).stable().call().await.map_err(|e: ethers::contract::ContractError<_>| eyre!(e))?
                    };
                    success = true;
                }
                #[cfg(not(feature = "local_simulation"))]
                {
                    let client_clone_for_call = client.clone(); // client is rpc_client here
                    let (rsv0_live, rsv1_live, _) = if dex_type == DexType::VelodromeV2 {
                        VelodromeV2Pool::new(pool_addr, client_clone_for_call.clone()).get_reserves().call().await.map_err(|e: ethers::contract::ContractError<_>| eyre!(e))?
                    } else {
                        AerodromePool::new(pool_addr, client_clone_for_call.clone()).get_reserves().call().await.map_err(|e: ethers::contract::ContractError<_>| eyre!(e))?
                    };
                    r0 = rsv0_live;
                    r1 = rsv1_live;
                    tokio::time::sleep(call_delay).await; 
                    t0 = if dex_type == DexType::VelodromeV2 { VelodromeV2Pool::new(pool_addr, client_clone_for_call.clone()).token_0().call().await.map_err(|e: ethers::contract::ContractError<_>| eyre!(e))? } else { AerodromePool::new(pool_addr, client_clone_for_call.clone()).token_0().call().await.map_err(|e: ethers::contract::ContractError<_>| eyre!(e))? };
                    tokio::time::sleep(call_delay).await; 
                    t1 = if dex_type == DexType::VelodromeV2 { VelodromeV2Pool::new(pool_addr, client_clone_for_call.clone()).token_1().call().await.map_err(|e: ethers::contract::ContractError<_>| eyre!(e))? } else { AerodromePool::new(pool_addr, client_clone_for_call.clone()).token_1().call().await.map_err(|e: ethers::contract::ContractError<_>| eyre!(e))? };
                    tokio::time::sleep(call_delay).await; 
                    s_res = if dex_type == DexType::VelodromeV2 { VelodromeV2Pool::new(pool_addr, client_clone_for_call.clone()).stable().call().await.map_err(|e: ethers::contract::ContractError<_>| eyre!(e))? } else { AerodromePool::new(pool_addr, client_clone_for_call.clone()).stable().call().await.map_err(|e: ethers::contract::ContractError<_>| eyre!(e))? };
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
            app_state.known_pools.insert(pool_addr, pool_state);
            app_state.pool_snapshots.insert(pool_addr, pool_snapshot);
            Ok(())
        }
        DexType::Balancer => { // Handle Balancer case
            warn!("Balancer pool detail fetching not fully implemented for pool {}", pool_addr);
            // Add placeholder state/snapshot or actual logic if available
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

// Added update_pool_snapshot function (placeholder, needs actual logic)
pub async fn update_pool_snapshot(
    pool_addr: Address,
    dex_type: DexType,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, 
    state: Arc<AppState>,
) -> Result<()> {
    info!("Attempting to update snapshot for pool: {}, dex: {:?}", pool_addr, dex_type);
    let current_block_number = client.get_block_number().await.map_err(|e| eyre!("Failed to get block number for snapshot update: {}", e))?;
    let current_block_number_u256 = U256::from(current_block_number.as_u64());

    match dex_type {
        DexType::UniswapV3 => {
            let pool_contract = crate::bindings::uniswap_v3_pool::UniswapV3Pool::new(pool_addr, client.clone());
            let slot0_data_val: crate::bindings::i_uniswap_v3_pool::Slot0Output = pool_contract.slot_0().call().await?;
            if let Some(mut snapshot) = state.pool_snapshots.get_mut(&pool_addr) {
                snapshot.sqrt_price_x96 = Some(slot0_data_val.sqrt_price_x96);
                snapshot.tick = Some(slot0_data_val.tick);
                snapshot.last_update_block = Some(current_block_number_u256); 
            }
        }
        DexType::VelodromeV2 => {
            let pool_contract = crate::bindings::velodrome_v2_pool::VelodromeV2Pool::new(pool_addr, client.clone());
            let (reserve0, reserve1) = pool_contract.get_reserves().call().await?;
            if let Some(mut snapshot) = state.pool_snapshots.get_mut(&pool_addr) {
                snapshot.reserves = Some((reserve0, reserve1));
                snapshot.last_updated_block = client.get_block_number().await?.as_u64();
            }
        }
        DexType::Aerodrome => {
            let pool_contract = crate::bindings::aerodrome_pool::AerodromePool::new(pool_addr, client.clone());
            let (reserve0, reserve1) = pool_contract.get_reserves().call().await?;
             if let Some(mut snapshot) = state.pool_snapshots.get_mut(&pool_addr) {
                snapshot.reserves = Some((reserve0, reserve1));
                snapshot.last_updated_block = client.get_block_number().await?.as_u64();
            }
        }
        DexType::Balancer => {
             warn!("Snapshot update for Balancer not implemented for pool {}", pool_addr);
        }
        _ => { warn!("Snapshot update not implemented for dex type: {:?}", dex_type); }
    }
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;
    use ethers::providers::{Provider, Http};
    use ethers::signers::LocalWallet;
    use std::str::FromStr;
    use crate::utils::get_http_provider; // Assuming you have this utility
    use crate::path_optimizer::PathOptimizer; // Added import for PathOptimizer

    #[tokio::test]
    #[ignore] 
    async fn test_app_state_creation() -> Result<()> {
        dotenv::dotenv().ok();
        let config = Config::default();
        let http_provider_url = std::env::var("HTTP_RPC_URL").expect("HTTP_RPC_URL must be set");
        let private_key = std::env::var("LOCAL_PRIVATE_KEY").expect("LOCAL_PRIVATE_KEY must be set");

        let http_provider = get_http_provider(&http_provider_url).await?;
        let wallet = LocalWallet::from_str(&private_key)?.with_chain_id(config.chain_id.unwrap_or(31337u64));
        let client_http = Arc::new(SignerMiddleware::new(http_provider.clone(), wallet.clone()));
        
        // For WS client, setup similarly if needed for test, or mock
        let ws_provider_url = std::env::var("WS_RPC_URL").expect("WS_RPC_URL must be set");
        let ws_provider = Provider::<Ws>::connect(&ws_provider_url).await?;
        let client_ws = Arc::new(SignerMiddleware::new(ws_provider, wallet.clone()));

        let nonce_manager = Arc::new(NonceManager::new(config.chain_id.unwrap_or(31337u64), wallet.address()));
        let path_optimizer = Arc::new(PathOptimizer::new(Arc::new(config.clone()))); // PathOptimizer needs Arc<Config>

        let app_state = AppState::new(config, client_ws, client_http, nonce_manager, path_optimizer).await?;
        assert_eq!(app_state.config.http_rpc_url, http_provider_url);
        Ok(())
    }
}