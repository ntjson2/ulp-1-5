// bot/src/path_optimizer.rs

use crate::state::{AppState, DexType, PoolSnapshot, PoolState};
use crate::config::Config;
use ethers::types::{Address, U256, I256};
use eyre::{eyre, Result};
use std::sync::Arc;
use tracing::{debug, info, warn, instrument};
use uniswap_v3_math; // Direct crate import

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
        format!("{:?}-{:?}", self.buy_pool_addr, self.sell_pool_addr)
    }
}

/// Internal helper to calculate WETH/USDC price using snapshot data + state context.
// This function was marked async in previous iterations. If it's used by the main async path, it should remain async.
// However, the main async find_top_routes doesn't call this directly.
// It's kept here for now, but its usage needs to be clarified or it might be unused by the primary async logic.
#[tracing::instrument(level="trace", skip(snapshot, state_context, config, app_state), fields(pool=%snapshot.pool_address, dex=?snapshot.dex_type))]
pub async fn calculate_price_usdc_per_weth(
    snapshot: &PoolSnapshot,
    state_context: &PoolState,
    config: &Config,
    _app_state: Arc<AppState>, // app_state was for decimals, now directly from config
) -> eyre::Result<f64> { 
    if snapshot.pool_address != state_context.pool_address {
        return Err(eyre!("Snapshot/State address mismatch during price calculation for {}", snapshot.pool_address));
    }
    let weth_address = config.weth_address;
    // let usdc_address = config.usdc_address; // Not directly used for price logic here
    let weth_decimals = config.weth_decimals;
    let usdc_decimals = config.usdc_decimals;


    let t0_is_weth = state_context.token0 == Some(weth_address); // state_context.token0 is Option<Address>
    let price: f64 = match snapshot.dex_type {
        DexType::UniswapV3 => {
            let sqrt_price = snapshot.sqrt_price_x96.ok_or_else(|| eyre!("Snapshot missing sqrtPriceX96 for UniV3 pool {}", snapshot.pool_address))?;
            let (dec0, dec1) = if t0_is_weth { (weth_decimals, usdc_decimals) } else { (usdc_decimals, weth_decimals) };
            // Corrected path for sqrt_price_x96_to_price
            uniswap_v3_math::price_math::sqrt_price_x96_to_price(sqrt_price, dec0.into(), dec1.into())?
        }
        DexType::VelodromeV2 | DexType::Aerodrome => {
            let r0 = snapshot.reserve0.ok_or_else(|| eyre!("Snapshot missing reserve0 for V2 pool {}", snapshot.pool_address))?;
            let r1 = snapshot.reserve1.ok_or_else(|| eyre!("Snapshot missing reserve1 for V2 pool {}", snapshot.pool_address))?;
            let (dec0, dec1) = if t0_is_weth { (weth_decimals, usdc_decimals) } else { (usdc_decimals, weth_decimals) };
            if r0.is_zero() || r1.is_zero() { return Ok(0.0); } 
            let price_token0_per_token1 = r1.as_u128() as f64 * 10f64.powi(dec0 as i32) / (r0.as_u128() as f64 * 10f64.powi(dec1 as i32));
            price_token0_per_token1 
        }
        DexType::Unknown => return Err(eyre!("Unknown DEX type in snapshot for pool {}", snapshot.pool_address)),
    };

    let usdc_per_weth_price = if state_context.token0 == Some(weth_address) { // If token0 is WETH
        // price is price of token0 (WETH) in terms of token1 (USDC)
        // So, price is WETH/USDC. We want USDC/WETH, so invert.
        if price.abs() < f64::EPSILON { return Err(eyre!("Intermediate price (WETH/USDC) is zero for pool {}, cannot invert", snapshot.pool_address)); }
        1.0 / price 
    } else { // token0 is USDC, token1 is WETH
        // price is price of token0 (USDC) in terms of token1 (WETH)
        // So, price is USDC/WETH. This is what we want.
        price
    };

    if !usdc_per_weth_price.is_finite() {
        return Err(eyre!("Calculated non-finite USDC/WETH price for pool {}", snapshot.pool_address));
    }
    Ok(usdc_per_weth_price)
}

// This is the main async find_top_routes function (previously around line 265)
#[instrument(skip_all, level = "info")]
pub async fn find_top_routes(
    app_state: Arc<AppState>,
) -> Vec<RouteCandidate> {
    info!("Searching for top arbitrage routes...");
    let mut candidates = Vec::new();
    let config = app_state.config.clone(); // Clone Arc<Config>
    let client = app_state.client.clone();

    let weth_address = config.weth_address; 
    let usdc_address = config.usdc_address; 

    for buy_pool_entry in app_state.pool_states.iter() {
        let buy_pool_addr = *buy_pool_entry.key(); 
        let buy_pool_state = buy_pool_entry.value();

        if !((buy_pool_state.token0 == Some(weth_address) && buy_pool_state.token1 == Some(usdc_address)) ||
               (buy_pool_state.token0 == Some(usdc_address) && buy_pool_state.token1 == Some(weth_address))) {
            continue;
        }

        for sell_pool_entry in app_state.pool_states.iter() {
            let sell_pool_addr = *sell_pool_entry.key(); 
            let sell_pool_state = sell_pool_entry.value();

            if buy_pool_addr == sell_pool_addr {
                continue;
            }

            if !((sell_pool_state.token0 == Some(weth_address) && sell_pool_state.token1 == Some(usdc_address)) ||
                   (sell_pool_state.token0 == Some(usdc_address) && sell_pool_state.token1 == Some(weth_address))) {
                continue;
            }
            
            let token_in_buy_pool = weth_address;
            let token_out_buy_pool = usdc_address;
            let token_in_sell_pool = usdc_address;
            let token_out_sell_pool = weth_address;

            let buy_is_t0_weth = buy_pool_state.token0 == Some(weth_address); 
            let zero_for_one_a = buy_is_t0_weth; 

            let sell_is_t0_usdc = sell_pool_state.token0 == Some(usdc_address); 
            // If selling USDC for WETH:
            // zero_for_one_b is true if token_in_sell_pool (USDC) is token0 of sell_pool.
            // This means we are selling token0 (USDC) for token1 (WETH).
            let zero_for_one_b = sell_is_t0_usdc;


            let route_candidate = RouteCandidate {
                path: vec![buy_pool_addr, sell_pool_addr], 
                dex_path: vec![buy_pool_state.dex_type, sell_pool_state.dex_type], 
                estimated_profit_wei: U256::zero(), 
                optimal_loan_amount_wei: U256::zero(),
                zero_for_one_a, 
                zero_for_one_b, 
                buy_pool_addr,    
                sell_pool_addr,  
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

    let mut profitable_routes = Vec::new();
    let mut simulation_tasks = Vec::new();

    for route_template in candidates {
        let app_state_clone = app_state.clone();
        let client_clone = client.clone();
        let config_clone_for_task = config.clone(); // Clone Arc<Config> for the task
        simulation_tasks.push(tokio::spawn(async move {
            match crate::simulation::find_optimal_loan_amount(app_state_clone, client_clone, &route_template, config_clone_for_task).await { 
                Ok((optimal_loan, estimated_profit)) => { 
                    if estimated_profit > I256::zero() {
                        let mut finalized_route = route_template.clone();
                        finalized_route.optimal_loan_amount_wei = optimal_loan;
                        finalized_route.estimated_profit_wei = estimated_profit.into_raw();
                        Some(finalized_route)
                    } else {
                        None
                    }
                }
                Err(e) => {
                    warn!(route = ?route_template.id(), error = ?e, "Error finding optimal loan for route");
                    None
                }
            }
        }));
    }

    let simulation_results = futures_util::future::join_all(simulation_tasks).await;
    for result in simulation_results {
        match result {
            Ok(Some(route)) => profitable_routes.push(route),
            Ok(None) => { /* No profitable loan found or already logged */ }
            Err(e) => warn!(error = ?e, "Simulation task panicked"),
        }
    }

    profitable_routes.sort_by(|a, b| b.estimated_profit_wei.cmp(&a.estimated_profit_wei));
    
    info!("Found {} potentially profitable routes after simulation.", profitable_routes.len());
    for route in profitable_routes.iter().take(5) {
        debug!(route_id = %route.id(), profit_wei = %route.estimated_profit_wei, loan_wei = %route.optimal_loan_amount_wei, "Top route candidate");
    }
    profitable_routes
}

pub fn calculate_price_example(
    sqrt_price: U256,
    dec0: u8,
    dec1: u8,
) -> Result<f64> {
    uniswap_v3_math::price_math::sqrt_price_x96_to_price(sqrt_price, dec0.into(), dec1.into())
        .map_err(|e| eyre!("Failed to convert sqrt_price_x96 to price: {:?}", e))
}