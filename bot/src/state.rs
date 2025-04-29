// bot/src/state.rs

// --- Imports ---
use crate::bindings::{AerodromePool, UniswapV3Pool, VelodromeV2Pool};
use crate::config::Config;
use dashmap::DashMap;
use ethers::{
    prelude::*, // Add prelude for common types/traits
    types::{Address, U256, U64},
};
use eyre::{eyre, Result, WrapErr}; // Keep WrapErr
use std::{str::FromStr, sync::Arc};
use tokio::time::{timeout, Duration};
use tracing::{debug, error, info, instrument, trace, warn}; // Add debug

// --- Enums / Structs ---
#[derive(Debug, Clone, PartialEq, Eq, Hash)] pub enum DexType { UniswapV3, VelodromeV2, Aerodrome, Unknown }
impl DexType { pub fn is_velo_style(&self) -> bool { matches!(self, DexType::VelodromeV2 | DexType::Aerodrome) } }
impl std::fmt::Display for DexType { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{:?}", self) } }
impl FromStr for DexType { type Err = eyre::Report; fn from_str(s: &str) -> Result<Self, Self::Err> { match s.to_lowercase().as_str() { "univ3"|"uniswapv3"=>Ok(DexType::UniswapV3), "velov2"|"velodrome"|"velodromev2"=>Ok(DexType::VelodromeV2), "aero"|"aerodrome"=>Ok(DexType::Aerodrome), _=>Err(eyre!("Unknown DEX: {}",s)), } } }

#[derive(Debug, Clone)] pub struct PoolState { pub pool_address: Address, pub dex_type: DexType, pub token0: Address, pub token1: Address, pub uni_fee: Option<u32>, pub velo_stable: Option<bool>, pub t0_is_weth: Option<bool> }
#[derive(Debug, Clone)] pub struct PoolSnapshot { pub pool_address: Address, pub dex_type: DexType, pub token0: Address, pub token1: Address, pub reserve0: Option<U256>, pub reserve1: Option<U256>, pub sqrt_price_x96: Option<U256>, pub tick: Option<i32>, pub last_update_block: Option<U64> }
#[derive(Debug, Clone)] pub struct AppState { pub config: Config, pub pool_states: Arc<DashMap<Address, PoolState>>, pub pool_snapshots: Arc<DashMap<Address, PoolSnapshot>>, pub weth_address: Address, pub usdc_address: Address, pub weth_decimals: u8, pub usdc_decimals: u8, pub velo_router_addr: Option<Address>, pub aero_router_addr: Option<Address>, pub uni_quoter_addr: Option<Address> }

// --- AppState Impl ---
impl AppState {
    pub fn new(config: Config) -> Self { /* ... unchanged ... */ Self { weth_address: config.weth_address, usdc_address: config.usdc_address, weth_decimals: config.weth_decimals, usdc_decimals: config.usdc_decimals, velo_router_addr: Some(config.velo_router_addr), aero_router_addr: config.aerodrome_router_addr, uni_quoter_addr: Some(config.quoter_v2_address), config, pool_states: Default::default(), pool_snapshots: Default::default() } }
    pub fn target_pair(&self) -> Option<(Address, Address)> { /* ... unchanged ... */ if !self.weth_address.is_zero()&&!self.usdc_address.is_zero(){if self.weth_address<self.usdc_address{Some((self.weth_address,self.usdc_address))}else{Some((self.usdc_address,self.weth_address))}}else{warn!("WETH/USDC zero"); None }}
}

// --- Helper Functions ---
#[instrument(skip_all, fields(pool=%pool_addr, dex=?dex_type), level="info")]
pub async fn fetch_and_cache_pool_state( pool_addr: Address, dex_type: DexType, client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, app_state: Arc<AppState>) -> Result<()> {
    info!("Fetching state..."); let weth_addr = app_state.weth_address; let timeout_dur = Duration::from_secs(app_state.config.fetch_timeout_secs.unwrap_or(15));

    // Define the async block that performs the fetches
    let fetch_logic = async {
        match dex_type {
            DexType::UniswapV3 => {
                 let pool = UniswapV3Pool::new(pool_addr, client.clone());
                 // Bind futures BEFORE joining to ensure temporaries live long enough
                 let slot0_fut = pool.slot_0().call();
                 let token0_fut = pool.token_0().call();
                 let token1_fut = pool.token_1().call();
                 let fee_fut = pool.fee().call();
                 // Now join the bound futures
                 let (slot0_res, token0_res, token1_res, fee_res) = tokio::try_join!(slot0_fut, token0_fut, token1_fut, fee_fut)?;
                 let (sqrtp_u160, tick, ..)=slot0_res; let (t0, t1, f) = (token0_res, token1_res, fee_res); let is_t0_weth=t0==weth_addr; let sqrtp=U256::from(sqrtp_u160);
                 let ps = PoolState{pool_address:pool_addr, dex_type:dex_type.clone(), token0:t0, token1:t1, uni_fee:Some(f), velo_stable:None, t0_is_weth:Some(is_t0_weth)};
                 let sn = PoolSnapshot{pool_address:pool_addr, dex_type, token0:t0, token1:t1, reserve0:None, reserve1:None, sqrt_price_x96:Some(sqrtp), tick:Some(tick), last_update_block:None};
                 Ok((ps, sn))
            }
             DexType::VelodromeV2 | DexType::Aerodrome => {
                  // Determine which pool type to use
                  let (reserves_fut, token0_fut, token1_fut, stable_fut) = if dex_type==DexType::VelodromeV2{
                      let p=VelodromeV2Pool::new(pool_addr,client.clone());
                      (p.get_reserves().call(), p.token_0().call(), p.token_1().call(), p.stable().call())
                  } else { // Aerodrome
                      let p=AerodromePool::new(pool_addr,client.clone());
                      (p.get_reserves().call(), p.token_0().call(), p.token_1().call(), p.stable().call())
                  };
                  // Join the bound futures
                  let (r,t0,t1,s) = tokio::try_join!(reserves_fut, token0_fut, token1_fut, stable_fut)?;
                  let(r0,r1,_)=r; let is_t0_weth=t0==weth_addr;
                  let ps = PoolState{pool_address:pool_addr, dex_type:dex_type.clone(), token0:t0, token1:t1, uni_fee:None, velo_stable:Some(s), t0_is_weth:Some(is_t0_weth)};
                  let sn = PoolSnapshot{pool_address:pool_addr, dex_type, token0:t0, token1:t1, reserve0:Some(r0), reserve1:Some(r1), sqrt_price_x96:None, tick:None, last_update_block:None};
                  Ok((ps, sn))
             }
            DexType::Unknown => Err(eyre!("Cannot fetch state for Unknown DEX type")),
        }
    };

    // Apply timeout AFTER defining the async block
    match timeout(timeout_dur, fetch_logic).await {
         Ok(Ok((ps, sn))) => { info!("State fetched successfully."); trace!(?ps, ?sn); app_state.pool_states.insert(pool_addr, ps); app_state.pool_snapshots.insert(pool_addr, sn); Ok(()) },
         Ok(Err(e))=>{error!(pool=%pool_addr, error=?e, "Fetch state failed"); Err(e.wrap_err("Pool state fetch logic failed"))}, // Add context with wrap_err
         Err(_)=>{error!(pool=%pool_addr,"Fetch state timeout"); Err(eyre!("Timeout fetching pool state for {}", pool_addr))}
    }
}

pub fn is_target_pair_option(a0:Address, a1:Address, target:Option<(Address,Address)>) -> bool { match target{Some((ta,tb))=>(a0==ta&&a1==tb)||(a0==tb&&a1==ta), None=>true} }


// END OF FILE: bot/src/state.rs