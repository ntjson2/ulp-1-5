// bot/src/path_optimizer.rs

use crate::state::{DexType, PoolSnapshot, PoolState};
use crate::config::Config;
use dashmap::DashMap;
use ethers::types::{Address, U256};
use eyre::{eyre, Result}; 
use std::sync::Arc;
use tracing::{debug, info, warn, trace, error}; // Ensured all are present
// use crate::utils; // Keep this if utils::sqrt_price_x96_to_price is used

// Represents a potential arbitrage opportunity (route) found.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteCandidate {
    pub path: Vec<Address>,
    pub dex_path: Vec<DexType>,
    pub estimated_profit_wei: U256,
    pub optimal_loan_amount_wei: U256,
    pub zero_for_one_a: bool, // Direction for pool A (buy_pool)
    pub zero_for_one_b: bool, // Direction for pool B (sell_pool)
    pub buy_pool_addr: Address,
    pub sell_pool_addr: Address,
    pub buy_dex_type: DexType,
    pub sell_dex_type: DexType,
    pub token_in_buy_pool: Address,  // Token we are "buying" from buy_pool (this is WETH)
    pub token_out_buy_pool: Address, // Token we are "selling" to buy_pool (this is USDC)
    pub token_in_sell_pool: Address, // Token we are "selling" to sell_pool (this is USDC)
    pub token_out_sell_pool: Address,// Token we are "getting" from sell_pool (this is WETH)
}

impl RouteCandidate {
    // Basic ID for logging/tracing, can be made more sophisticated
    pub fn id(&self) -> String {
        format!("{:?}-{:?}", self.buy_pool_addr, self.sell_pool_addr) // Ensured field names
    }
}

/// Internal helper to calculate WETH/USDC price using snapshot data + state context.
// Made synchronous and takes &Config
#[tracing::instrument(level="trace", skip(snapshot, state_context, config), fields(pool=%snapshot.pool_address, dex=?snapshot.dex_type))]
fn calculate_price_usdc_per_weth( // Removed async
    snapshot: &PoolSnapshot,
    state_context: &PoolState, 
    config: &Config,       // Changed to &Config
    // Pass decimals directly as they are now sourced from config in find_top_routes
    weth_decimals_val: u8,
    usdc_decimals_val: u8,
) -> Result<f64> { 
    let weth_address = config.weth_address; 
    let usdc_address = config.usdc_address;

    let sqrt_price = snapshot.sqrt_price_x96.ok_or_else(|| {
        eyre!("Missing sqrt_price_x96 for pool {}", snapshot.pool_address)
    })?;

    // Determine token order for price calculation using PoolState's token0/token1
    let (dec0, dec1) = if state_context.token0 == weth_address && state_context.token1 == usdc_address {
        (weth_decimals_val, usdc_decimals_val) 
    } else if state_context.token0 == usdc_address && state_context.token1 == weth_address {
        (usdc_decimals_val, weth_decimals_val) 
    } else {
        return Err(eyre!("Pool {} (snapshot {}) does not involve WETH and USDC directly according to PoolState tokens {} and {}", 
            state_context.pool_address, snapshot.pool_address, state_context.token0, state_context.token1));
    };

    // Call the helper via crate::utils
    let price = crate::utils::sqrt_price_x96_to_price(sqrt_price, dec0.into(), dec1.into())?;

    if state_context.token0 == weth_address { // Direct comparison for Address
        Ok(1.0 / price) 
    } else { 
        Ok(price)
    }
}

#[tracing::instrument(level = "info", skip_all)]
pub fn find_top_routes( // Removed async, changed signature
    _updated_pool_snapshot: &PoolSnapshot, // Renamed as it's not directly used in this simplified version
    pool_states: &Arc<DashMap<Address, PoolState>>,
    pool_snapshots: &Arc<DashMap<Address, PoolSnapshot>>,
    config: &Config, // Changed to &Config
    weth_address: Address,
    usdc_address: Address,
    weth_decimals: u8,
    usdc_decimals: u8,
) -> Vec<RouteCandidate> { // Return Vec directly
    info!("Searching for top arbitrage routes (synchronous)...");
    let mut candidates = Vec::new();

    // Parameter checks from Point 1 (applied to direct params now)
    if weth_address == Address::zero() {
        error!("WETH address not configured or zero");
        return Vec::new();
    }
    if usdc_address == Address::zero() {
        error!("USDC address not configured or zero");
        return Vec::new();
    }
    if weth_decimals == 0 { // Assuming 0 is an invalid decimal value
        error!("WETH decimals not configured or zero");
        return Vec::new();
    }
    if usdc_decimals == 0 { // Assuming 0 is an invalid decimal value
        error!("USDC decimals not configured or zero");
        return Vec::new();
    }


    for buy_entry in pool_snapshots.iter() { 
        let buy_pool_address_key = buy_entry.key();
        let buy_snap = buy_entry.value();

        let buy_pool_state_dashmap_ref = match pool_states.get(buy_pool_address_key) {
            Some(state_ref) => state_ref,
            None => {
                warn!("PoolState not found for buy pool {} while it exists in snapshots", buy_pool_address_key);
                continue;
            }
        };
        let buy_pool_state = buy_pool_state_dashmap_ref.value();


        if !((buy_pool_state.token0 == weth_address && buy_pool_state.token1 == usdc_address) ||
               (buy_pool_state.token0 == usdc_address && buy_pool_state.token1 == weth_address)) {
            trace!("Buy pool {} is not WETH/USDC, skipping", buy_pool_address_key);
            continue;
        }

        for sell_entry in pool_snapshots.iter() { 
            let sell_pool_address_key = sell_entry.key();
            let sell_snap = sell_entry.value();

            if buy_pool_address_key == sell_pool_address_key {
                continue;
            }

            let sell_pool_state_dashmap_ref = match pool_states.get(sell_pool_address_key) {
                Some(state_ref) => state_ref,
                None => {
                    warn!("PoolState not found for sell pool {} while it exists in snapshots", sell_pool_address_key);
                    continue;
                }
            };
            let sell_pool_state = sell_pool_state_dashmap_ref.value();

            if !((sell_pool_state.token0 == weth_address && sell_pool_state.token1 == usdc_address) ||
                   (sell_pool_state.token0 == usdc_address && sell_pool_state.token1 == weth_address)) {
                trace!("Sell pool {} is not WETH/USDC, skipping", sell_pool_address_key);
                continue;
            }

            let buy_is_t0_weth = buy_pool_state.token0 == weth_address; 
            let buy_price_usdc_per_weth = match calculate_price_usdc_per_weth(buy_snap, buy_pool_state, config, weth_decimals, usdc_decimals) {
                Ok(price) => price,
                Err(e) => {
                    warn!("Failed to calculate buy price for pool {}: {}", buy_pool_address_key, e);
                    continue;
                }
            };
            
            let sell_is_t0_usdc_in_sell_pool = sell_pool_state.token0 == usdc_address; 
            let sell_price_usdc_per_weth = match calculate_price_usdc_per_weth(sell_snap, sell_pool_state, config, weth_decimals, usdc_decimals) {
                Ok(price) => price,
                Err(e) => {
                    warn!("Failed to calculate sell price for pool {}: {}", sell_pool_address_key, e);
                    continue;
                }
            };

            let token_in_buy_pool = weth_address;
            let token_out_buy_pool = usdc_address;
            let token_in_sell_pool = usdc_address;
            let token_out_sell_pool = weth_address;

            let zero_for_one_a = buy_is_t0_weth; 
            let zero_for_one_b = sell_is_t0_usdc_in_sell_pool; 

            let implied_profit = (sell_price_usdc_per_weth - buy_price_usdc_per_weth).max(0.0);

            if implied_profit < 0.001 {
                trace!(route = ?format!("{}-{}", buy_pool_address_key, sell_pool_address_key), implied_profit, "Implied profit below threshold, skipping route");
                continue;
            }

            let route_candidate = RouteCandidate {
                path: vec![*buy_pool_address_key, *sell_pool_address_key],
                dex_path: vec![buy_pool_state.dex_type, sell_pool_state.dex_type],
                estimated_profit_wei: U256::zero(),
                optimal_loan_amount_wei: U256::zero(),
                zero_for_one_a,
                zero_for_one_b,
                buy_pool_addr: *buy_pool_address_key,    // Ensured field name
                sell_pool_addr: *sell_pool_address_key,  // Ensured field name
                buy_dex_type: buy_pool_state.dex_type, 
                sell_dex_type: sell_pool_state.dex_type,
                token_in_buy_pool,
                token_out_buy_pool,
                token_in_sell_pool,
                token_out_sell_pool,
            };
            
            candidates.push(route_candidate);
        }
    }
    
    if candidates.is_empty() {
        info!("No potential WETH/USDC arbitrage routes found from pool states.");
        return Vec::new(); 
    }

    // Removed tokio::spawn and simulation logic for this first-aid fix.
    // The `profitable_routes` vector is now `candidates`.
    // Sorting and logging will apply to `candidates`.
    candidates.sort_by(|a, b| b.estimated_profit_wei.cmp(&a.estimated_profit_wei)); // Profit is not estimated yet.
    
    info!("Found {} potential candidate routes (simulations not run).", candidates.len());
    for route in candidates.iter().take(5) { 
        debug!(route_id = %route.id(), "Candidate route (profit not yet simulated)"); // Uses id() which uses correct field names
    }
    candidates // Return candidates directly
}

pub fn calculate_price_example(
    sqrt_price: U256,
    dec0: u8,
    dec1: u8,
) -> Result<f64> {
    crate::utils::sqrt_price_x96_to_price(sqrt_price, dec0.into(), dec1.into()) // Use crate::utils
        .map_err(|e| eyre!("Failed to convert sqrt_price_x96 to price: {:?}", e))
}

async fn old_calculate_price_usdc_per_weth( // This function is old and likely unused, but kept as per original file structure
    snapshot: &PoolSnapshot,
    weth_address: Address,
    usdc_address: Address,
    weth_decimals: u8,
    usdc_decimals: u8,
) -> Result<f64> { // Changed to eyre::Result
    let sqrt_price = snapshot.sqrt_price_x96.ok_or_else(|| {
        eyre!("Missing sqrt_price_x96 for pool {}", snapshot.pool_address)
    })?;

    // Determine token order based on snapshot's token0/token1 if available, otherwise error.
    // This part needs robust fetching of token0/token1 for the snapshot's pool_address
    // For now, we assume we know which one is token0 and token1 for the price calculation.
    // This logic needs to be revisited based on how PoolSnapshot stores token order or if it needs to fetch it.
    // Placeholder: assume token0 is WETH and token1 is USDC for this calculation if not otherwise specified by snapshot.
    // This is a critical simplification and needs to be accurate.
    // Let's assume snapshot.token0 and snapshot.token1 are reliable Address types.
    // This function does not use config for decimals, it takes them as params.
    
    let (dec0_val, dec1_val) = if snapshot.token0 == weth_address { // Direct comparison
        (weth_decimals, usdc_decimals)
    } else if snapshot.token0 == usdc_address { // Direct comparison
        (usdc_decimals, weth_decimals)
    } else {
        return Err(eyre!("Pool {} does not involve WETH or USDC as token0", snapshot.pool_address));
    };

    let price = crate::utils::sqrt_price_x96_to_price(sqrt_price, dec0_val.into(), dec1_val.into())?; // Use crate::utils

    if snapshot.token0 == weth_address { // Direct comparison
        // price is token1/token0 = USDC/WETH
        Ok(price)
    } else {
        // price is token1/token0 = WETH/USDC, so invert for USDC/WETH
        Ok(1.0 / price)
    }
}