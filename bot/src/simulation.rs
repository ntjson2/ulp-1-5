// src/simulation.rs

use crate::bindings::{
    quoter_v2 as quoter_v2_bindings, velodrome_router as velo_router_bindings,
    AerodromeRouter, QuoterV2, VelodromeRouter, UniswapV3Pool,
};
use crate::config::Config; // Import Config struct
use crate::encoding::encode_user_data;
use crate::gas::estimate_flash_loan_gas;
use crate::state::{AppState, DexType, PoolSnapshot, PoolState};
use crate::path_optimizer::RouteCandidate;
use crate::utils::{f64_to_wei, ToF64Lossy, v2_price_from_reserves, v3_price_from_sqrt};
use ethers::{
    prelude::{Http, LocalWallet, Middleware, Provider, SignerMiddleware},
    types::{Address, I256, U256},
    utils::{format_units, parse_units},
};
use eyre::{eyre, Result, WrapErr};
use std::{str::FromStr, sync::Arc};
use tracing::{debug, error, info, instrument, trace, warn};

// Percentage of pool reserve to consider as max loan size for V2/Aero pools
const V2_RESERVE_PERCENTAGE_LIMIT: u64 = 5;

// --- simulate_swap function (unchanged) ---
#[allow(clippy::too_many_arguments)]
#[instrument(skip(app_state, client), level = "trace", fields(dex = dex_type_str, amount_in = %amount_in))]
pub async fn simulate_swap( app_state: Arc<AppState>, client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, dex_type_str: &str, token_in: Address, token_out: Address, amount_in: U256, is_stable_route: bool, uni_pool_fee: u32, ) -> Result<U256> { /* ... implementation unchanged ... */ }

// --- Profit Calculation Helper (unchanged) ---
#[allow(clippy::too_many_arguments)]
#[instrument(skip_all, level = "debug", fields( amount_in_wei = %amount_in_wei ))]
pub async fn calculate_net_profit( app_state: Arc<AppState>, client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, route: &RouteCandidate, amount_in_wei: U256, gas_price_gwei: f64, gas_limit_buffer_percentage: u64, min_flashloan_gas_limit: u64, ) -> Result<I256> { /* ... implementation unchanged ... */ }


// --- Optimal Loan Amount Search Function (unchanged) ---
#[allow(clippy::too_many_arguments)]
#[instrument(skip_all, level = "info", fields( route = ?route ))]
pub async fn find_optimal_loan_amount( client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, app_state: Arc<AppState>, route: &RouteCandidate, buy_pool_snapshot: Option<&PoolSnapshot>, sell_pool_snapshot: Option<&PoolSnapshot>, gas_price_gwei: f64, ) -> Result<Option<(U256, I256)>> { /* ... implementation unchanged ... */ }


/// Calculates a dynamic maximum loan size based on pool depth from snapshots.
/// Checks config flag before attempting (placeholder) UniV3 sizing.
#[instrument(level="debug", skip(buy_pool_snapshot, sell_pool_snapshot, config))]
fn calculate_dynamic_max_loan(
    config_max_loan_wei: U256,
    buy_pool_snapshot: Option<&PoolSnapshot>,
    _sell_pool_snapshot: Option<&PoolSnapshot>, // Mark unused
    loan_token: Address,
    config: &Config, // <-- Pass Config reference
) -> U256 {
    trace!("Calculating dynamic max loan based on pool depth...");
    let mut dynamic_max = config_max_loan_wei; // Start with the absolute maximum

    if let Some(buy_snap) = buy_pool_snapshot {
        match buy_snap.dex_type {
            DexType::VelodromeV2 | DexType::Aerodrome => {
                let reserve_option = if buy_snap.token0 == loan_token { buy_snap.reserve0 }
                                     else if buy_snap.token1 == loan_token { buy_snap.reserve1 }
                                     else { warn!(pool = %buy_snap.pool_address, "Loan token mismatch V2"); None }; // Log warning

                if let Some(reserve) = reserve_option {
                    if !reserve.is_zero() {
                         let limit = reserve * U256::from(V2_RESERVE_PERCENTAGE_LIMIT) / U256::from(100);
                         dynamic_max = std::cmp::min(dynamic_max, limit);
                         trace!(pool = %buy_snap.pool_address, dex=%buy_snap.dex_type.to_string(), limit = %limit, "Applied V2/Aero depth limit");
                    } else { dynamic_max = U256::zero(); trace!(pool = %buy_snap.pool_address,"V2/Aero Reserve zero"); }
                } else { dynamic_max = U256::zero(); } // Default to zero if loan token not found
            }
            DexType::UniswapV3 => {
                // --- Check Config Flag for UniV3 ---
                if config.enable_univ3_dynamic_sizing {
                    // Flag is enabled, proceed with placeholder logic
                    warn!(pool = %buy_snap.pool_address, "UniV3 dynamic loan sizing enabled but NOT IMPLEMENTED. Using config max as limit.");
                    trace!("Accurate UniV3 sizing requires tick liquidity analysis. TODO.");
                    // No change to dynamic_max, effectively using config_max_loan_wei
                    dynamic_max = std::cmp::min(dynamic_max, config_max_loan_wei);
                } else {
                    // Flag is disabled (default), explicitly use config max and log at trace level
                    trace!(pool = %buy_snap.pool_address, "UniV3 dynamic sizing disabled by config. Using config max.");
                    dynamic_max = std::cmp::min(dynamic_max, config_max_loan_wei);
                }
            }
            DexType::Unknown => { warn!(pool = %buy_snap.pool_address, "Cannot apply depth limit for Unknown DEX type."); }
        }
    } else { warn!("Buy pool snapshot missing for dynamic sizing."); }

    // --- Sell Pool Limit (Placeholder) ---

    // Final check: ensure dynamic_max never exceeds the absolute config_max_loan_wei
    let final_dynamic_max = std::cmp::min(dynamic_max, config_max_loan_wei);

    if final_dynamic_max < config_max_loan_wei { debug!(dynamic_max_wei = %final_dynamic_max, "Dynamic depth limit applied."); }
    else { trace!(dynamic_max_wei = %final_dynamic_max, "Using config max (or less) as limit."); }
    final_dynamic_max
}

// --- Trait Implementations (Unchanged) ---
impl ToString for DexType { fn to_string(&self) -> String { match self { DexType::UniswapV3=>"UniV3", DexType::VelodromeV2=>"VeloV2", DexType::Aerodrome=>"Aero", DexType::Unknown=>"Unknown", }.to_string() } }
impl FromStr for DexType { type Err = eyre::Report; fn from_str(s: &str) -> Result<Self, Self::Err> { match s.to_lowercase().as_str() { "univ3"|"uniswapv3"=>Ok(DexType::UniswapV3), "velov2"|"velodrome"|"velodromev2"=>Ok(DexType::VelodromeV2), "aero"|"aerodrome"=>Ok(DexType::Aerodrome), _=>Err(eyre!("Unknown DEX: {}",s)), } } }
impl DexType { fn is_velo_style(&self) -> bool { matches!(self, DexType::VelodromeV2 | DexType::Aerodrome) } }

// END OF FILE: bot/src/simulation.rs