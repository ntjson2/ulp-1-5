// bot/src/state.rs

// --- Imports ---
// Add necessary imports used within this module
use crate::bindings::{AerodromePool, UniswapV3Pool, VelodromeV2Pool};
use crate::config::Config;
use dashmap::DashMap;
use ethers::{
    prelude::*, // For Middleware, SignerMiddleware, Provider, etc.
    types::{Address, U256, U64},
};
use eyre::{eyre, Result, WrapErr}; // Add imports
use std::{str::FromStr, sync::Arc};
use tokio::time::{timeout, Duration}; // Add imports
use tracing::{debug, error, info, instrument, trace, warn}; // Add imports

// --- Enums / Structs ---
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DexType { UniswapV3, VelodromeV2, Aerodrome, Unknown }

// Add helper method directly to DexType impl
impl DexType {
    pub fn is_velo_style(&self) -> bool {
        matches!(self, DexType::VelodromeV2 | DexType::Aerodrome)
    }
}
impl std::fmt::Display for DexType { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{:?}", self) } }
impl FromStr for DexType { type Err = eyre::Report; fn from_str(s: &str) -> Result<Self, Self::Err> { match s.to_lowercase().as_str() { "univ3"|"uniswapv3"=>Ok(DexType::UniswapV3), "velov2"|"velodrome"|"velodromev2"=>Ok(DexType::VelodromeV2), "aero"|"aerodrome"=>Ok(DexType::Aerodrome), _=>Err(eyre!("Unknown DEX: {}",s)), } } }


#[derive(Debug, Clone)] pub struct PoolState { pub pool_address: Address, pub dex_type: DexType, pub token0: Address, pub token1: Address, pub uni_fee: Option<u32>, pub velo_stable: Option<bool>, pub t0_is_weth: Option<bool> }

// Add Clone derive
#[derive(Debug, Clone)] pub struct PoolSnapshot { pub pool_address: Address, pub dex_type: DexType, pub token0: Address, pub token1: Address, pub reserve0: Option<U256>, pub reserve1: Option<U256>, pub sqrt_price_x96: Option<U256>, pub tick: Option<i32>, pub last_update_block: Option<U64> }

#[derive(Debug, Clone)] pub struct AppState { pub config: Config, pub pool_states: Arc<DashMap<Address, PoolState>>, pub pool_snapshots: Arc<DashMap<Address, PoolSnapshot>>, pub weth_address: Address, pub usdc_address: Address, pub weth_decimals: u8, pub usdc_decimals: u8, pub velo_router_addr: Option<Address>, pub aero_router_addr: Option<Address>, pub uni_quoter_addr: Option<Address> }

// --- AppState Impl ---
impl AppState {
    pub fn new(config: Config) -> Self { /* ... unchanged ... */ Self { weth_address: config.weth_address, usdc_address: config.usdc_address, weth_decimals: config.weth_decimals, usdc_decimals: config.usdc_decimals, velo_router_addr: Some(config.velo_router_addr), aero_router_addr: config.aerodrome_router_addr, uni_quoter_addr: Some(config.quoter_v2_address), config, pool_states: Default::default(), pool_snapshots: Default::default() } }
    pub fn target_pair(&self) -> Option<(Address, Address)> { /* ... unchanged ... */ if !self.weth_address.is_zero()&&!self.usdc_address.is_zero(){if self.weth_address<self.usdc_address{Some((self.weth_address,self.usdc_address))}else{Some((self.usdc_address,self.weth_address))}}else{warn!("WETH/USDC zero"); None }}
}

// --- Helper Functions ---
#[instrument(skip_all, fields(pool=%pool_addr, dex=?dex_type), level="info")]
pub async fn fetch_and_cache_pool_state( pool_addr: Address, dex_type: DexType, client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, app_state: Arc<AppState>) -> Result<()> { /* ... implementation unchanged, already includes needed imports and types ... */
    info!("Fetching state..."); let weth_addr = app_state.weth_address; let timeout_dur = Duration::from_secs(app_state.config.fetch_timeout_secs.unwrap_or(15));
    let fetch_future = async { match dex_type {
        DexType::UniswapV3 => { let p = UniswapV3Pool::new(pool_addr, client.clone()); let (s0, t0, t1, f) = tokio::try_join!(p.slot_0().call(), p.token_0().call(), p.token_1().call(), p.fee().call())?; let (sqrtp_u160, tick, ..)=s0; let is_t0_weth=t0==weth_addr; let sqrtp=U256::from(sqrtp_u160); Ok((PoolState{pool_address:pool_addr, dex_type:dex_type.clone(), token0:t0, token1:t1, uni_fee:Some(f), velo_stable:None, t0_is_weth:Some(is_t0_weth)}, PoolSnapshot{pool_address:pool_addr, dex_type, token0:t0, token1:t1, reserve0:None, reserve1:None, sqrt_price_x96:Some(sqrtp), tick:Some(tick), last_update_block:None})) },
        DexType::VelodromeV2|DexType::Aerodrome => { let (r,t0,t1,s):( (U256,U256,_),_,_,_) = if dex_type==DexType::VelodromeV2{let p=VelodromeV2Pool::new(pool_addr,client.clone()); tokio::try_join!(p.get_reserves().call(), p.token_0().call(), p.token_1().call(), p.stable().call())?} else {let p=AerodromePool::new(pool_addr,client.clone()); tokio::try_join!(p.get_reserves().call(), p.token_0().call(), p.token_1().call(), p.stable().call())?}; let(r0,r1,_)=r; let is_t0_weth=t0==weth_addr; Ok((PoolState{pool_address:pool_addr, dex_type:dex_type.clone(), token0:t0, token1:t1, uni_fee:None, velo_stable:Some(s), t0_is_weth:Some(is_t0_weth)}, PoolSnapshot{pool_address:pool_addr, dex_type, token0:t0, token1:t1, reserve0:Some(r0), reserve1:Some(r1), sqrt_price_x96:None, tick:None, last_update_block:None}))},
        DexType::Unknown => Err(eyre!("Unknown DEX type")),
    }};
    match timeout(timeout_dur, fetch_future).await { Ok(Ok((ps, sn))) => { info!("State fetched."); trace!(?ps, ?sn); app_state.pool_states.insert(pool_addr, ps); app_state.pool_snapshots.insert(pool_addr, sn); Ok(()) }, Ok(Err(e))=>{error!(pool=%pool_addr, error=?e, "Fetch state failed"); Err(e)}, Err(_)=>{error!(pool=%pool_addr,"Fetch state timeout"); Err(eyre!("Timeout"))} }
}

pub fn is_target_pair_option(a0:Address, a1:Address, target:Option<(Address,Address)>) -> bool { match target{Some((ta,tb))=>(a0==ta&&a1==tb)||(a0==tb&&a1==ta), None=>true} }


// END OF FILE: bot/src/state.rs