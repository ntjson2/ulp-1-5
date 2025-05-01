// bot/src/path_optimizer.rs

use crate::config::Config;
use crate::state::{DexType, PoolSnapshot, PoolState};
use ethers::types::Address;
use eyre::{eyre, Result, WrapErr};
use dashmap::DashMap;
use std::sync::Arc;
use tracing::{debug, info, instrument, trace, warn};

// Represents a potential arbitrage opportunity (route) found.
#[derive(Debug, Clone)]
pub struct RouteCandidate {
    pub buy_pool_addr: Address,
    pub sell_pool_addr: Address,
    pub buy_dex_type: DexType,
    pub sell_dex_type: DexType,
    pub token_in: Address,  // The token being borrowed (e.g., WETH)
    pub token_out: Address, // The intermediate token (e.g., USDC)
    // Pool-specific details needed for simulation/execution
    pub buy_pool_fee: Option<u32>,     // UniV3 fee
    pub sell_pool_fee: Option<u32>,    // UniV3 fee
    pub buy_pool_stable: Option<bool>, // Velo/Aero stability flag
    pub sell_pool_stable: Option<bool>,// Velo/Aero stability flag
    pub buy_pool_factory: Address,    // Factory that created the buy pool
    pub sell_pool_factory: Address,   // Factory that created the sell pool
    // Execution parameters
    pub zero_for_one_a: bool, // Direction for the first swap (buy pool)
    // Metadata
    pub estimated_profit_usd: f64, // Placeholder metric (e.g., price diff %)
}

// Define the threshold here for now, could be moved to config later
const ARBITRAGE_THRESHOLD_PERCENTAGE: f64 = 0.1; // Example: 0.1% difference needed

/// Identifies potential 2-way arbitrage routes involving the updated pool's snapshot.
/// Compares prices derived from snapshots in the hot cache. Uses PoolState for context.
// FIX: Remove 'config' from skip list as parameter is '_config' (unused)
#[instrument(skip(all_pool_states, all_pool_snapshots), level="debug", fields(pool=%updated_pool_snapshot.pool_address))]
pub fn find_top_routes(
    updated_pool_snapshot: &PoolSnapshot, // Triggering snapshot
    all_pool_states: &Arc<DashMap<Address, PoolState>>, // Source of detailed state context (incl. factory)
    all_pool_snapshots: &Arc<DashMap<Address, PoolSnapshot>>, // Map to iterate for comparison (hot cache)
    _config: &Config, // Mark config as unused for now
    // Target pair info (passed directly for clarity)
    weth_address: Address,
    usdc_address: Address,
    weth_decimals: u8,
    usdc_decimals: u8,
) -> Vec<RouteCandidate> {
    trace!("Finding routes for updated pool snapshot");

    let mut candidates = Vec::new();
    let updated_pool_address = updated_pool_snapshot.pool_address;

    // --- Get Context for Updated Pool ---
    let updated_pool_state_entry = match all_pool_states.get(&updated_pool_address) {
        Some(state_ref) => state_ref,
        None => {
            warn!(pool = %updated_pool_address, "PoolState context missing for updated snapshot. Cannot find routes.");
            return vec![];
        }
    };
    let updated_pool_state_context = updated_pool_state_entry.value().clone();
    drop(updated_pool_state_entry);

    let updated_price = match calculate_cached_price(
        updated_pool_snapshot,
        &updated_pool_state_context,
        weth_address, usdc_address, weth_decimals, usdc_decimals
    ) {
        Ok(price) => {
            trace!(pool = %updated_pool_address, price = price, "Calculated price for updated pool from snapshot.");
            price
        },
        Err(e) => {
            warn!(pool = %updated_pool_address, error = ?e, "Failed price calculation for updated pool. Cannot find routes.");
            return vec![];
        }
    };

    // --- Iterate Through Other Snapshots for Comparison ---
    trace!( "Iterating through {} snapshots...", all_pool_snapshots.len() );
    for snapshot_entry in all_pool_snapshots.iter() {
        let other_pool_snapshot = snapshot_entry.value();
        let other_pool_addr = *snapshot_entry.key();

        if other_pool_addr == updated_pool_address { continue; }

        let is_other_target = crate::state::is_target_pair_option(
            other_pool_snapshot.token0,
            other_pool_snapshot.token1,
            Some((weth_address, usdc_address))
        );
        if !is_other_target { continue; }

        trace!(compare_pool = %other_pool_addr, "Comparing against snapshot.");

        // --- Get Context for Comparison Pool ---
        let other_pool_state_entry = match all_pool_states.get(&other_pool_addr) {
            Some(r) => r,
            None => {
                warn!(pool=%other_pool_addr, "PoolState context missing for comparison pool snapshot. Skipping.");
                continue;
            }
        };
        let other_pool_state_context = other_pool_state_entry.value().clone();
        drop(other_pool_state_entry);


        let other_price = match calculate_cached_price(
            other_pool_snapshot,
            &other_pool_state_context,
            weth_address, usdc_address, weth_decimals, usdc_decimals
        ) {
            Ok(p) => p,
            Err(e) => {
                trace!(pool=%other_pool_addr, error=?e, "Price calculation failed for comparison pool. Skipping.");
                continue;
            }
        };

        // --- Compare Prices & Check Threshold ---
        if other_price.abs() < f64::EPSILON {
            trace!(pool=%other_pool_addr,"Skip: Comparison pool price is zero.");
            continue;
        }

        let price_diff = updated_price - other_price;
        let lower_price = updated_price.min(other_price);

        let price_diff_percentage = if lower_price.abs() > f64::EPSILON {
            (price_diff.abs() / lower_price) * 100.0
        } else {
            f64::INFINITY
        };

        trace!(
            pool1 = %updated_pool_address, price1 = updated_price,
            pool2 = %other_pool_addr, price2 = other_price,
            diff_pct = price_diff_percentage
        );

        // --- Create Route Candidate if Threshold Met ---
        if price_diff_percentage >= ARBITRAGE_THRESHOLD_PERCENTAGE {
            let (buy_snapshot, sell_snapshot, buy_state, sell_state) =
                if updated_price < other_price {
                    (updated_pool_snapshot, other_pool_snapshot, &updated_pool_state_context, &other_pool_state_context)
                } else {
                    (other_pool_snapshot, updated_pool_snapshot, &other_pool_state_context, &updated_pool_state_context)
                };

             let (log_buy_price, log_sell_price) = if updated_price < other_price {
                 (updated_price, other_price)
             } else {
                 (other_price, updated_price)
             };
             info!(
                 buy_pool = %buy_snapshot.pool_address, buy_dex = ?buy_snapshot.dex_type, buy_price = log_buy_price,
                 sell_pool = %sell_snapshot.pool_address, sell_dex = ?sell_snapshot.dex_type, sell_price = log_sell_price,
                 diff_pct = price_diff_percentage,
                 "Potential arbitrage opportunity found!"
             );

            let zero_for_one_a = determine_swap_direction(buy_state, weth_address);
            trace!(buy_pool = %buy_state.pool_address, zero_for_one_a, "Determined swap direction for Swap A");

            let candidate = RouteCandidate {
                buy_pool_addr: buy_snapshot.pool_address,
                sell_pool_addr: sell_snapshot.pool_address,
                buy_dex_type: buy_snapshot.dex_type,
                sell_dex_type: sell_snapshot.dex_type,
                token_in: weth_address,
                token_out: usdc_address,
                buy_pool_fee: buy_state.uni_fee,
                sell_pool_fee: sell_state.uni_fee,
                buy_pool_stable: buy_state.velo_stable,
                sell_pool_stable: sell_state.velo_stable,
                buy_pool_factory: buy_state.factory,
                sell_pool_factory: sell_state.factory,
                zero_for_one_a,
                estimated_profit_usd: price_diff_percentage,
            };

            debug!(candidate = ?candidate, "Created RouteCandidate");
            candidates.push(candidate);
        }
    } // End loop through snapshots

    if !candidates.is_empty() {
        candidates.sort_by(|a, b| b.estimated_profit_usd.partial_cmp(&a.estimated_profit_usd).unwrap_or(std::cmp::Ordering::Equal));
        debug!("Sorted {} candidates by estimated profit (desc).", candidates.len());
         if let Some(top_candidate) = candidates.first() {
              info!(?top_candidate, "Most promising candidate identified.");
         }
    } else {
        trace!("No arbitrage candidates found meeting threshold.");
    }

    candidates
}

/// Helper to determine swap direction (zeroForOne) for the first swap (Swap A) in the buy_pool.
fn determine_swap_direction(buy_pool_state: &PoolState, loan_token: Address) -> bool {
    buy_pool_state.token0 == loan_token
}


/// Internal helper to calculate WETH/USDC price using snapshot data + state context.
#[instrument(level="trace", skip(snapshot, state_context), fields(pool=%snapshot.pool_address, dex=?snapshot.dex_type))]
fn calculate_cached_price(
    snapshot: &PoolSnapshot,
    state_context: &PoolState,
    weth_address: Address,
    _usdc_address: Address,
    weth_decimals: u8,
    usdc_decimals: u8,
) -> Result<f64> {
    // ... (implementation remains the same) ...
    if snapshot.pool_address != state_context.pool_address {
        return Err(eyre!("Snapshot/State address mismatch during price calculation for {}", snapshot.pool_address));
    }
    if snapshot.dex_type != state_context.dex_type {
        warn!(pool=%snapshot.pool_address, snap_dex=?snapshot.dex_type, state_dex=?state_context.dex_type, "Snapshot/State DEX type mismatch!");
    }

    let t0_is_weth = match state_context.t0_is_weth {
        Some(is_weth) => is_weth,
        None => {
            warn!(pool = %state_context.pool_address, "t0_is_weth flag missing in PoolState context, deriving from tokens.");
            state_context.token0 == weth_address
        }
    };

    let price_t1_per_t0_result: Result<f64> = match snapshot.dex_type {
        DexType::UniswapV3 => {
            let sqrt_price = snapshot.sqrt_price_x96.ok_or_else(|| eyre!("Snapshot missing sqrtPriceX96 for UniV3 pool {}", snapshot.pool_address))?;
            let (dec0, dec1) = if t0_is_weth { (weth_decimals, usdc_decimals) } else { (usdc_decimals, weth_decimals) };
            crate::utils::v3_price_from_sqrt(sqrt_price, dec0, dec1)
        }
        DexType::VelodromeV2 | DexType::Aerodrome => {
            let r0 = snapshot.reserve0.ok_or_else(|| eyre!("Snapshot missing reserve0 for V2 pool {}", snapshot.pool_address))?;
            let r1 = snapshot.reserve1.ok_or_else(|| eyre!("Snapshot missing reserve1 for V2 pool {}", snapshot.pool_address))?;
            let (dec0, dec1) = if t0_is_weth { (weth_decimals, usdc_decimals) } else { (usdc_decimals, weth_decimals) };
            crate::utils::v2_price_from_reserves(r0, r1, dec0, dec1)
        }
        DexType::Unknown => Err(eyre!("Unknown DEX type in snapshot for pool {}", snapshot.pool_address)),
    };

    let price_t1_per_t0 = price_t1_per_t0_result
        .wrap_err_with(|| format!("Base price calculation failed for pool {}", snapshot.pool_address))?;

    let price_usdc_per_weth = if t0_is_weth {
        price_t1_per_t0
    } else {
        if price_t1_per_t0.abs() < f64::EPSILON {
            return Err(eyre!("Intermediate price (WETH/USDC) is zero for pool {}, cannot invert", snapshot.pool_address));
        }
        1.0 / price_t1_per_t0
    };

    if !price_usdc_per_weth.is_finite() {
        return Err(eyre!("Calculated non-finite USDC/WETH price for pool {}", snapshot.pool_address));
    }

    trace!(price = price_usdc_per_weth, "Calculated USDC/WETH price from snapshot");
    Ok(price_usdc_per_weth)
}