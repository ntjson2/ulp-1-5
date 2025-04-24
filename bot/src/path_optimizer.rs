// bot/src/path_optimizer.rs

use crate::config::Config;
use crate::event_handler::{DexType, PoolState};
use crate::utils::calculate_pool_price_weth_per_usdc; // Ensure this helper is accessible
use dashmap::DashMap;
use ethers::types::{Address, U256};
use eyre::{Result, WrapErr};
use std::sync::Arc;
use tracing::{debug, info, trace, warn};

// Represents a potential arbitrage opportunity (route) found.
#[derive(Debug, Clone)]
pub struct RouteCandidate {
    pub buy_pool_addr: Address,
    pub sell_pool_addr: Address,
    pub buy_dex_type: DexType,
    pub sell_dex_type: DexType,
    pub token_in: Address,         // The token to borrow (e.g., WETH)
    pub token_out: Address,        // The intermediate token (e.g., USDC)
    pub buy_pool_fee: Option<u32>, // UniV3 fee
    pub sell_pool_fee: Option<u32>, // UniV3 fee
    pub buy_pool_stable: Option<bool>, // Velo stability
    pub sell_pool_stable: Option<bool>, // Velo stability
    pub zero_for_one_a: bool, // Direction for swap A (buy_pool: token_in -> token_out?)
    // Profitability metric (using % diff for now, should be updated after simulation)
    pub estimated_profit_usd: f64,
}

// Define the threshold here for now, could be moved to config later
const ARBITRAGE_THRESHOLD_PERCENTAGE: f64 = 0.1; // Example: 0.1% difference needed

/// Identifies potential 2-way arbitrage routes involving the updated pool.
///
/// Compares the price of the updated pool against all other known pools
/// for the same token pair (WETH/USDC) to find price discrepancies.
/// Sorts the found candidates by estimated profitability (descending).
///
/// # Arguments
/// * `updated_pool_state`: The current state of the pool that triggered the check.
/// * `all_pool_states`: A reference to the shared map containing states of all monitored pools.
/// * `config`: Application configuration.
/// * `weth_address`: WETH token address.
/// * `usdc_address`: USDC token address.
/// * `weth_decimals`: WETH token decimals.
/// * `usdc_decimals`: USDC token decimals.
///
/// # Returns
/// * `Vec<RouteCandidate>`: A list of potential arbitrage routes, sorted by estimated profit (desc).
pub fn find_top_routes(
    updated_pool_state: &PoolState,
    all_pool_states: &Arc<DashMap<Address, PoolState>>,
    config: &Config,
    weth_address: Address,
    usdc_address: Address,
    weth_decimals: u8,
    usdc_decimals: u8,
) -> Vec<RouteCandidate> {
    debug!(pool = ?updated_pool_state.pool_address, "Finding routes for updated pool");

    let mut candidates = Vec::new();

    // --- Calculate price for the updated pool ---
    let updated_price = match calculate_pool_price_weth_per_usdc(
        updated_pool_state,
        weth_address,
        usdc_address,
        weth_decimals,
        usdc_decimals,
    ) {
        Ok(price) if price > 0.0 && price.is_finite() => {
            debug!(pool = ?updated_pool_state.pool_address, price = price, "Calculated price for updated pool.");
            price
        }
        _ => {
            // Errors/warnings logged inside calculate function
            return candidates; // Cannot proceed without a valid price for the updated pool
        }
    };

    // --- Iterate through all other known pools ---
    trace!(
        "Iterating through {} known pool states...",
        all_pool_states.len()
    );
    for entry in all_pool_states.iter() {
        let other_pool_state = entry.value();
        let other_pool_addr = entry.key();

        // --- Check 1: Skip self-comparison ---
        if *other_pool_addr == updated_pool_state.pool_address {
            continue;
        }

        // --- Check 2: Ensure the other pool is also for the target pair (WETH/USDC) ---
        let is_other_target_pair =
            (other_pool_state.token0 == weth_address && other_pool_state.token1 == usdc_address)
                || (other_pool_state.token0 == usdc_address
                    && other_pool_state.token1 == weth_address);

        if !is_other_target_pair {
            continue;
        }

        trace!(compare_pool = ?other_pool_addr, "Comparing updated pool against this pool.");

        // --- Calculate price for the other pool ---
        let other_price = match calculate_pool_price_weth_per_usdc(
            other_pool_state,
            weth_address,
            usdc_address,
            weth_decimals,
            usdc_decimals,
        ) {
            Ok(price) if price > 0.0 && price.is_finite() => {
                trace!(pool = ?other_pool_addr, price = price, "Calculated price for comparison pool.");
                price
            }
            _ => {
                // Errors/warnings logged inside calculate function
                trace!(pool = ?other_pool_addr, "Skipping comparison due to invalid price for this pool.");
                continue;
            }
        };

        // --- Compare prices and check threshold ---
        if other_price.abs() < f64::EPSILON {
            warn!(pool = ?other_pool_addr, "Other pool price is zero, skipping comparison.");
            continue;
        }

        let price_diff = updated_price - other_price;
        let lower_price = updated_price.min(other_price);
        let price_diff_percentage = (price_diff.abs() / lower_price) * 100.0;

        trace!(
            pool1 = ?updated_pool_state.pool_address, price1 = updated_price,
            pool2 = ?other_pool_addr, price2 = other_price,
            diff_pct = price_diff_percentage
        );

        if price_diff_percentage >= ARBITRAGE_THRESHOLD_PERCENTAGE {
            // Determine buy/sell pools
            let (buy_pool_state, sell_pool_state, _buy_price, _sell_price) = // Renamed price vars to avoid shadowing
                if updated_price < other_price {
                    // Buy on updated (cheaper WETH), Sell on other (more expensive WETH)
                    (updated_pool_state, other_pool_state, updated_price, other_price)
                } else {
                    // Buy on other (cheaper WETH), Sell on updated (more expensive WETH)
                    (other_pool_state, updated_pool_state, other_price, updated_price)
                };

            info!(
                buy_pool = %buy_pool_state.pool_address, dex = ?buy_pool_state.dex_type, // Use % for address
                sell_pool = %sell_pool_state.pool_address, dex = ?sell_pool_state.dex_type, // Use % for address
                diff_pct = price_diff_percentage,
                "Potential arbitrage opportunity found!"
            );

            // --- Create RouteCandidate ---
            let zero_for_one_a = determine_swap_direction(buy_pool_state, weth_address);
            trace!(buy_pool = %buy_pool_state.pool_address, zero_for_one_a, "Determined swap direction for buy leg");

            let candidate = RouteCandidate {
                buy_pool_addr: buy_pool_state.pool_address,
                sell_pool_addr: sell_pool_state.pool_address,
                buy_dex_type: buy_pool_state.dex_type.clone(),
                sell_dex_type: sell_pool_state.dex_type.clone(),
                token_in: weth_address,  // Loan token is always WETH in this setup
                token_out: usdc_address, // Intermediate token is USDC
                buy_pool_fee: buy_pool_state.uni_fee,
                sell_pool_fee: sell_pool_state.uni_fee,
                buy_pool_stable: buy_pool_state.velo_stable,
                sell_pool_stable: sell_pool_state.velo_stable,
                zero_for_one_a,
                // Use price diff percentage as a preliminary estimate.
                estimated_profit_usd: price_diff_percentage,
            };

            debug!(candidate = ?candidate, "Created RouteCandidate");
            candidates.push(candidate);
        }
    } // End loop through all_pool_states

    // --- Sort candidates by estimated profit (descending) ---
    if !candidates.is_empty() {
        // Use sort_by for custom comparison, comparing f64 requires partial_cmp
        candidates.sort_by(|a, b| {
            b.estimated_profit_usd
                .partial_cmp(&a.estimated_profit_usd)
                .unwrap_or(std::cmp::Ordering::Equal) // Handle NaN or non-comparable floats gracefully
        });
        debug!("Sorted {} candidates by estimated profit (desc).", candidates.len());
        // Log the top candidate found after sorting
         if let Some(top_candidate) = candidates.first() {
             info!(top_candidate = ?top_candidate, "Most promising candidate identified.");
         }
    } else {
         debug!(pool = ?updated_pool_state.pool_address, "No arbitrage candidates found meeting threshold.");
    }

    candidates // Return the sorted list of candidates
}

/// Helper to determine swap direction (zeroForOne) for the first swap (Swap A) in the buy_pool.
fn determine_swap_direction(buy_pool_state: &PoolState, loan_token: Address) -> bool {
    buy_pool_state.token0 == loan_token
}


/// Helper to calculate the price of WETH in terms of USDC for a given pool.
/// NOTE: Duplicate function - should be consolidated into utils.rs later.
#[instrument(level="trace", skip(pool_state), fields(pool=%pool_state.pool_address, dex=?pool_state.dex_type))]
fn calculate_pool_price_weth_per_usdc(
    pool_state: &PoolState,
    weth_address: Address,
    usdc_address: Address,
    weth_decimals: u8,
    usdc_decimals: u8,
) -> Result<f64> {
    // Determine if token0 is WETH for this specific pool
    let t0_is_weth = match pool_state.t0_is_weth {
        Some(is_weth) => is_weth,
        None => {
            warn!(pool = ?pool_state.pool_address, "t0_is_weth flag not cached, determining now.");
            pool_state.token0 == weth_address
        }
    };

    // Calculate the raw price of token1 in terms of token0
    let price_t1_per_t0 = match pool_state.dex_type {
        DexType::UniswapV3 => {
            let sqrt_price = pool_state.sqrt_price_x96.ok_or_else(|| {
                eyre::eyre!(
                    "Missing sqrtPriceX96 for UniV3 pool {}",
                    pool_state.pool_address
                )
            })?;
            let (dec0, dec1) = if t0_is_weth {
                (weth_decimals, usdc_decimals)
            } else {
                (usdc_decimals, weth_decimals)
            };
            crate::utils::v3_price_from_sqrt(sqrt_price, dec0, dec1).wrap_err_with(|| {
                format!(
                    "Failed V3 price calculation for pool {}",
                    pool_state.pool_address
                )
            })?
        }
        DexType::VelodromeV2 => {
            let r0 = pool_state.reserve0.ok_or_else(|| {
                eyre::eyre!(
                    "Missing reserve0 for VeloV2 pool {}",
                    pool_state.pool_address
                )
            })?;
            let r1 = pool_state.reserve1.ok_or_else(|| {
                eyre::eyre!(
                    "Missing reserve1 for VeloV2 pool {}",
                    pool_state.pool_address
                )
            })?;
            let (dec0, dec1) = if t0_is_weth {
                (weth_decimals, usdc_decimals)
            } else {
                (usdc_decimals, weth_decimals)
            };
            crate::utils::v2_price_from_reserves(r0, r1, dec0, dec1).wrap_err_with(|| {
                format!(
                    "Failed V2 price calculation for pool {}",
                    pool_state.pool_address
                )
            })?
        }
        DexType::Unknown => {
            return Err(eyre::eyre!(
                "Cannot calculate price for pool {} with Unknown DEX type",
                pool_state.pool_address
            ))
        }
    };

    // Convert price(T1)/price(T0) to price(WETH)/price(USDC)
    let price_weth_per_usdc = if t0_is_weth {
        if price_t1_per_t0.abs() < f64::EPSILON {
            return Err(eyre::eyre!(
                "Intermediate price (t1/t0) is near zero, cannot invert for WETH/USDC price (pool {})",
                pool_state.pool_address
            ));
        }
        1.0 / price_t1_per_t0
    } else {
        price_t1_per_t0
    };

    if !price_weth_per_usdc.is_finite() {
        return Err(eyre::eyre!(
            "Calculated non-finite WETH/USDC price ({}) for pool {}",
            price_weth_per_usdc,
            pool_state.pool_address
        ));
    }

    trace!(price = price_weth_per_usdc, "Calculated WETH/USDC price");
    Ok(price_weth_per_usdc)
}


// END OF FILE: bot/src/path_optimizer.rs