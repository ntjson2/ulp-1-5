// bot/src/path_optimizer.rs

use crate::config::Config;
// Import necessary types from state module
use crate::state::{AppState, DexType, PoolSnapshot, PoolState}; // Ensure DexType is imported
// Import base price calculation utilities FROM utils.rs
use crate::utils::{v2_price_from_reserves, v3_price_from_sqrt}; // Correct import path
use dashmap::DashMap;
use ethers::types::{Address, U256};
use eyre::{eyre, Result, WrapErr}; // Import eyre components
use std::sync::Arc;
use tracing::{debug, error, info, instrument, trace, warn};

// Represents a potential arbitrage opportunity (route) found.
#[derive(Debug, Clone)]
pub struct RouteCandidate {
    pub buy_pool_addr: Address,
    pub sell_pool_addr: Address,
    pub buy_dex_type: DexType, // Correctly populated with Velo or Aero
    pub sell_dex_type: DexType, // Correctly populated with Velo or Aero
    pub token_in: Address,
    pub token_out: Address,
    pub buy_pool_fee: Option<u32>,
    pub sell_pool_fee: Option<u32>,
    pub buy_pool_stable: Option<bool>,
    pub sell_pool_stable: Option<bool>,
    pub zero_for_one_a: bool,
    pub estimated_profit_usd: f64, // Placeholder metric
}

// Define the threshold here for now, could be moved to config later
const ARBITRAGE_THRESHOLD_PERCENTAGE: f64 = 0.1; // Example: 0.1% difference needed

/// Identifies potential 2-way arbitrage routes involving the updated pool's snapshot.
/// Compares prices derived from snapshots in the hot cache.
#[instrument(skip(all_pool_states, all_pool_snapshots, config), level="debug", fields(pool=%updated_pool_snapshot.pool_address))]
pub fn find_top_routes(
    updated_pool_snapshot: &PoolSnapshot, // Triggering snapshot
    all_pool_states: &Arc<DashMap<Address, PoolState>>, // Source of detailed state context
    all_pool_snapshots: &Arc<DashMap<Address, PoolSnapshot>>, // Map to iterate for comparison (hot cache)
    config: &Config,
    weth_address: Address,
    usdc_address: Address,
    weth_decimals: u8,
    usdc_decimals: u8,
) -> Vec<RouteCandidate> {
    trace!("Finding routes for updated pool snapshot");

    let mut candidates = Vec::new();
    let updated_pool_address = updated_pool_snapshot.pool_address;

    // Get context for the updated pool
    let updated_pool_state_context = match all_pool_states.get(&updated_pool_address) {
        Some(state_ref) => state_ref,
        None => { warn!(pool = %updated_pool_address, "Ctx missing for updated snapshot"); return vec![]; }
    };

    // Calculate price for the updated pool using its snapshot + context
    let updated_price = match calculate_cached_price( updated_pool_snapshot, &updated_pool_state_context, weth_address, usdc_address, weth_decimals, usdc_decimals ) {
        Ok(price) => { trace!(pool = %updated_pool_address, price = price, "Calculated updated price from snapshot."); price },
        Err(e) => { warn!(pool = %updated_pool_address, error = ?e, "Failed updated price calc"); return vec![]; }
    };

    // Iterate through snapshots in the hot cache for comparison
    trace!( "Iterating through {} snapshots...", all_pool_snapshots.len() );
    for snapshot_entry in all_pool_snapshots.iter() {
        let other_pool_snapshot = snapshot_entry.value();
        let other_pool_addr = snapshot_entry.key();

        // Check 1: Skip self-comparison
        if *other_pool_addr == updated_pool_address { continue; }

        // Check 2: Ensure the other pool involves the target pair
        let is_other_target_pair = (other_pool_snapshot.token0 == weth_address && other_pool_snapshot.token1 == usdc_address) || (other_pool_snapshot.token0 == usdc_address && other_pool_snapshot.token1 == weth_address);
        if !is_other_target_pair { continue; }

        trace!(compare_pool = %other_pool_addr, "Comparing against snapshot.");

        // Get context for the comparison pool
        let other_pool_state_context = match all_pool_states.get(other_pool_addr) { Some(r)=>r, None=>{warn!(pool=%other_pool_addr, "Ctx missing"); continue;}};

        // Calculate price for the other pool using its snapshot + context
        let other_price = match calculate_cached_price( other_pool_snapshot, &other_pool_state_context, weth_address, usdc_address, weth_decimals, usdc_decimals ) {
            Ok(p)=>p, Err(e)=>{trace!(pool=%other_pool_addr, error=?e, "Skip: Other price failed"); continue;}
        };

        // Compare prices and check threshold
        if other_price.abs() < f64::EPSILON { trace!(pool=%other_pool_addr,"Skip: Other price zero"); continue; }

        let price_diff = updated_price - other_price;
        let lower_price = updated_price.min(other_price);
        let price_diff_percentage = (price_diff.abs() / lower_price) * 100.0;

        trace!( pool1 = %updated_pool_address, price1 = updated_price, pool2 = %other_pool_addr, price2 = other_price, diff_pct = price_diff_percentage );

        if price_diff_percentage >= ARBITRAGE_THRESHOLD_PERCENTAGE {
            let (buy_snapshot, sell_snapshot, buy_state, sell_state) =
                if updated_price < other_price {
                    (updated_pool_snapshot, other_pool_snapshot, updated_pool_state_context.value(), other_pool_state_context.value())
                } else {
                    (other_pool_snapshot, updated_pool_snapshot, other_pool_state_context.value(), updated_pool_state_context.value())
                };

             info!( buy_pool = %buy_snapshot.pool_address, dex = ?buy_snapshot.dex_type, sell_pool = %sell_snapshot.pool_address, dex = ?sell_snapshot.dex_type, diff_pct = price_diff_percentage, "Potential arbitrage opportunity found!" );

            // Create RouteCandidate
            let zero_for_one_a = determine_swap_direction(buy_state, weth_address);
            trace!(buy_pool = %buy_state.pool_address, zero_for_one_a, "Determined swap direction");

            let candidate = RouteCandidate {
                buy_pool_addr: buy_snapshot.pool_address,
                sell_pool_addr: sell_snapshot.pool_address,
                buy_dex_type: buy_snapshot.dex_type.clone(), // Use DexType from snapshot
                sell_dex_type: sell_snapshot.dex_type.clone(), // Use DexType from snapshot
                token_in: weth_address, token_out: usdc_address,
                buy_pool_fee: buy_state.uni_fee, sell_pool_fee: sell_state.uni_fee, // Use fee/stable from detail state
                buy_pool_stable: buy_state.velo_stable, sell_pool_stable: sell_state.velo_stable,
                zero_for_one_a, estimated_profit_usd: price_diff_percentage,
            };

            debug!(candidate = ?candidate, "Created RouteCandidate");
            candidates.push(candidate);
        }
    } // End loop through snapshots

    // Sort candidates by estimated profit (descending)
    if !candidates.is_empty() {
        candidates.sort_by(|a, b| b.estimated_profit_usd.partial_cmp(&a.estimated_profit_usd).unwrap_or(std::cmp::Ordering::Equal));
        debug!("Sorted {} candidates by estimated profit (desc).", candidates.len());
         if let Some(top_candidate) = candidates.first() { info!(top_candidate = ?top_candidate, "Most promising candidate identified."); }
    } else { trace!("No arbitrage candidates found meeting threshold."); }

    candidates
}

/// Helper to determine swap direction (zeroForOne) for the first swap (Swap A) in the buy_pool.
fn determine_swap_direction(buy_pool_state: &PoolState, loan_token: Address) -> bool {
    buy_pool_state.token0 == loan_token
}


/// Internal helper to calculate WETH/USDC price using snapshot data + state context.
/// Calls base price calculation functions from utils.rs.
#[instrument(level="trace", skip(snapshot, state_context), fields(pool=%snapshot.pool_address, dex=?snapshot.dex_type))]
fn calculate_cached_price(
    snapshot: &PoolSnapshot,
    state_context: &PoolState,
    weth_address: Address,
    _usdc_address: Address,
    weth_decimals: u8,
    usdc_decimals: u8,
) -> Result<f64> {
    // Sanity checks
    if snapshot.pool_address != state_context.pool_address { return Err(eyre!("Snapshot/State address mismatch")); }
    if snapshot.dex_type != state_context.dex_type { warn!(pool=%snapshot.pool_address, "Snapshot/State DEX mismatch!"); }

    // Determine t0_is_weth from reliable PoolState context
    let t0_is_weth = match state_context.t0_is_weth {
        Some(is_weth) => is_weth,
        None => { warn!(pool = %state_context.pool_address, "t0_is_weth flag not cached"); state_context.token0 == weth_address }
    };

    // Calculate raw price t1/t0 using snapshot data and utils functions
    let price_t1_per_t0 = match snapshot.dex_type {
        DexType::UniswapV3 => {
            let sqrt_price = snapshot.sqrt_price_x96.ok_or_else(|| eyre!("Snapshot missing sqrtPriceX96"))?;
            let (dec0, dec1) = if t0_is_weth { (weth_decimals, usdc_decimals) } else { (usdc_decimals, weth_decimals) };
            // *** Calling function from utils.rs ***
            crate::utils::v3_price_from_sqrt(sqrt_price, dec0, dec1)?
        }
        DexType::VelodromeV2 | DexType::Aerodrome => {
            let r0 = snapshot.reserve0.ok_or_else(|| eyre!("Snapshot missing reserve0"))?;
            let r1 = snapshot.reserve1.ok_or_else(|| eyre!("Snapshot missing reserve1"))?;
            let (dec0, dec1) = if t0_is_weth { (weth_decimals, usdc_decimals) } else { (usdc_decimals, weth_decimals) };
            // *** Calling function from utils.rs ***
            crate::utils::v2_price_from_reserves(r0, r1, dec0, dec1)?
        }
        DexType::Unknown => return Err(eyre!("Unknown DEX type in snapshot"))
    }.wrap_err_with(|| format!("Base price calculation failed for pool {}", snapshot.pool_address))?;


    // Convert price(T1)/price(T0) to price(WETH)/price(USDC)
    let price_weth_per_usdc = if t0_is_weth {
        if price_t1_per_t0.abs() < f64::EPSILON { return Err(eyre!("Intermediate price zero, cannot invert")); }
        1.0 / price_t1_per_t0
    } else {
        price_t1_per_t0
    };

    if !price_weth_per_usdc.is_finite() { return Err(eyre!("Calculated non-finite WETH/USDC price")); }

    trace!(price = price_weth_per_usdc, "Calculated WETH/USDC price from snapshot");
    Ok(price_weth_per_usdc)
}

// *** Removed the duplicate calculate_pool_price_weth_per_usdc function ***

// END OF FILE: bot/src/path_optimizer.rs