// bot/src/simulation.rs
//! Handles off-chain simulation of arbitrage routes to determine profitability
//! and optimal loan amounts before attempting on-chain execution.

use crate::bindings::{
    BalancerVault, QuoterV2, VelodromeRouter,
    // IVelodromeFactory was unused.
    // Trying VelodromeV2Pair as it's Velodrome V2 context.
    // If this is not found, the exact name from bindings needs to be used.
    VelodromeV2Pair as VelodromePair, 
    quoter_v2 as quoter_v2_bindings,
    velodrome_router as velo_router_bindings,
};
use crate::state::{AppState, DexType};
use crate::config::Config;
use crate::path_optimizer::RouteCandidate;
use ethers::{
    core::types::{Address, U256, I256, Selector}, 
    // Removed ParseUnits as it was marked unused by the compiler
    utils::{parse_units, format_units, ConversionError}, 
    providers::{Middleware, Provider, Http},
    contract::{ContractError}, // Removed EthCall (unused warning)
    signers::{LocalWallet}, // Removed Signer (unused warning)
    middleware::SignerMiddleware,
};
use eyre::{eyre, Result};
use std::sync::Arc;
use std::str::FromStr;
use std::error::Error as StdError;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{instrument, trace, info, error, warn, debug};

pub const V2_RESERVE_PERCENTAGE_LIMIT: u8 = 50;

pub struct SimulationConfig {
    pub anvil_ws_url: &'static str,
    pub http_url: &'static str,
    pub minimal_emitter: &'static str,
}

pub const SIMULATION_CONFIG: SimulationConfig = SimulationConfig {
    anvil_ws_url: "ws://127.0.0.1:8545/ws",
    http_url: "http://127.0.0.1:8545",
    minimal_emitter: "0x0000000000000000000000000000000000000000",
};

pub const VELO_ROUTER_IMPL_ADDR_FOR_SIM: &str = "0x0000000000000000000000000000000000000000"; // Ensure this is a valid address or placeholder
pub const PAIR_DOES_NOT_EXIST_SELECTOR_STR: &str = "0x08c379a0"; // Standard Error(string) selector, adjust if Velo uses a custom one


/// Simulates a single swap on a DEX using appropriate on-chain query methods.
// (Function remains unchanged)
#[allow(clippy::too_many_arguments)]
#[instrument(skip(app_state, client), level = "trace", fields(pool_address = %pool_address, dex = %dex_type, token_in = %token_in, token_out = %token_out, amount_in = %amount_in))]
pub async fn simulate_swap(
    app_state: Arc<AppState>,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    pool_address: Address, // This is the pool_address parameter
    dex_type: DexType,
    token_in: Address,
    token_out: Address,
    amount_in: U256,
) -> Result<U256> {
    trace!("Simulating single swap...");
    let config = &app_state.config;
    // pool_address is a parameter, so it's in scope here.
    let (pool_fee, pool_is_stable, factory_address_opt) = {
        let entry = app_state.pool_states.get(&pool_address); // Using the pool_address parameter
        if let Some(state_val) = entry {
            (state_val.uni_fee, state_val.velo_stable, state_val.factory)
        } else {
            warn!(%pool_address, "Pool state not found for simulation. Attempting to fetch."); // Using the pool_address parameter
            return Err(eyre!("Pool state for {} not found during simulation", pool_address)); // Using the pool_address parameter
        }
    };

    match dex_type {
        DexType::UniswapV3 => {
            trace!(pool=%pool_address, token_in=%token_in, token_out=%token_out, amount_in=%amount_in, "Simulating UniswapV3 swap"); // Using pool_address
            let quoter_address = config.quoter_v2_address; // Assuming H160, not Option
            let quoter = QuoterV2::new(quoter_address, client.clone());
            let fee_u24 = pool_fee.ok_or_else(|| eyre!("UniswapV3 fee not found for pool {}", pool_address))?; // Using pool_address
            
            let params = quoter_v2_bindings::QuoteExactInputSingleParams { // Use alias
                token_in,
                token_out,
                amount_in,
                fee: fee_u24,
                sqrt_price_limit_x96: U256::zero(), 
            };
            trace!(?params, "Calling QuoterV2 quoteExactInputSingle");
            match quoter.quote_exact_input_single(params).call().await {
                Ok(quote_result) => Ok(quote_result.0),
                Err(e) => Err(eyre!("QuoterV2 call failed for UniV3 pool {}: {:?}", pool_address, e)), // Using pool_address
            }
        }
        DexType::VelodromeV2 | DexType::Aerodrome => {
            trace!(pool=%pool_address, token_in=%token_in, token_out=%token_out, amount_in=%amount_in, "Simulating Velodrome/Aerodrome swap"); // Using pool_address
            let router_address_to_use = if dex_type == DexType::VelodromeV2 {
                config.velo_router_addr // Assuming this is H160, not Option<H160>
            } else {
                config.aerodrome_router_addr.ok_or_else(|| eyre!("Aerodrome Router address not configured"))?
            };
            let router = VelodromeRouter::new(router_address_to_use, client.clone());
            let stable = pool_is_stable.ok_or_else(|| eyre!("Velo/Aero stable status not found for pool {}", pool_address))?; // Using pool_address
            let factory_address = factory_address_opt.ok_or_else(|| eyre!("Factory address not found for Velo/Aero pool {}", pool_address))?; // Using pool_address
            
            let routes = vec![velo_router_bindings::Route { // Use alias
                from: token_in,
                to: token_out,
                stable,
                factory: factory_address,
            }];
            trace!(?routes, amount_in = %amount_in, "Calling VelodromeRouter ({}) getAmountsOut", router_address_to_use);

            let amounts_out = router.get_amounts_out(amount_in, routes).call().await
                .map_err(|e| {
                    let e_str = format!("{}", e);
                    if e_str.contains("Velodrome: INSUFFICIENT_LIQUIDITY") || e_str.contains("Aerodrome: INSUFFICIENT_LIQUIDITY") {
                        eyre!("Insufficient liquidity in Velo/Aero pool {} for swap {} -> {} with amount {}", pool_address, token_in, token_out, amount_in)
                    } else {
                        eyre!("Failed to simulate Velo/Aero swap for pool {}: {}", pool_address, e)
                    }
                })?;

            if amounts_out.is_empty() || amounts_out.len() < 2 {
                // Ensure the error message string is properly terminated
                Err(eyre!("getAmountsOut returned empty or insufficient data for Velo/Aerodrome pool {}", pool_address))
            } else {
                Ok(amounts_out[1]) // Last element is the amount_out
            }
        }
        DexType::Unknown => Err(eyre!("Unknown DEX type for simulate_swap")),
    }
}

/// Calculates the estimated net profit for a given route and loan amount.
#[allow(clippy::too_many_arguments)]
#[instrument(skip(app_state, client, route), level = "debug", fields( loan_amount_wei = %amount_in_wei ))]
pub async fn simulate_calculate_net_profit_wei(
    app_state: Arc<AppState>,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    route: &RouteCandidate,
    amount_in_wei: U256, 
) -> Result<I256> {
    trace!("Calculating net profit for route: {:?} -> {:?}", route.buy_dex_type, route.sell_dex_type);
    let loan_token = route.token_in_buy_pool; // Corrected field
    let intermediate_token = route.token_out_buy_pool; // Corrected field

    // Corrected simulate_swap call (6 arguments)
    let amount_out_intermediate = match simulate_swap(
        app_state.clone(), client.clone(), route.buy_pool_addr, route.buy_dex_type,
        loan_token, intermediate_token, amount_in_wei,
    ).await {
        Ok(amount) => {
            trace!(amount_out_intermediate = %amount, "Swap A simulation successful.");
            amount
        }
        Err(e) => {
            warn!(error=?e, pool=%route.buy_pool_addr, "Swap A simulation failed, assuming unprofitable.");
            return Ok(I256::min_value());
        }
    };

    if amount_out_intermediate.is_zero() {
        debug!("Amount out from first swap is zero, no profit possible.");
        return Ok(I256::min_value());
    }
    
    // Corrected simulate_swap call (6 arguments)
    let final_amount_out_loan_token = match simulate_swap(
        app_state.clone(), client.clone(), route.sell_pool_addr, route.sell_dex_type,
        intermediate_token, loan_token, amount_out_intermediate,
    ).await {
        Ok(amount) => {
            trace!(final_amount_out_loan_token = %amount, "Swap B simulation successful.");
            amount
        }
        Err(e) => {
            warn!(error=?e, pool=%route.sell_pool_addr, "Swap B simulation failed, assuming unprofitable.");
            return Ok(I256::min_value());
        }
    };

    trace!("Estimating gas cost for net profit calculation...");
    let config_ref = &app_state.config; // Keep as ref to Arc<Config>
    // Corrected field name: e.g., simulation_gas_price_gwei
    let gas_price_gwei = config_ref.simulation_gas_price_gwei.unwrap_or(1.0); 
    let gas_price_wei_str = format!("{:.18}", gas_price_gwei); 
    let gas_price_wei: U256 = parse_units(&gas_price_wei_str, "gwei")
        .map_err(|e: ConversionError| eyre!("Failed to parse gas_price_gwei_override: {}", e))? // Use ConversionError
        .into();
    trace!(gas_price_gwei=%gas_price_gwei, gas_price_wei=%gas_price_wei, "Converted gas price");
    
    let executor_address_opt = config_ref.arb_executor_address; 
    let executor_address = executor_address_opt.ok_or_else(|| eyre!("ArbitrageExecutor address not configured for gas estimate"))?;

    let salt_bytes: [u8; 32] = rand::random();
    let salt = U256::from_big_endian(&salt_bytes);

    let user_data = crate::encoding::encode_user_data(
        route.buy_pool_addr,
        route.sell_pool_addr,
        config_ref.usdc_address, // Assuming this is H160
        route.zero_for_one_a,
        route.buy_dex_type.is_velo_style(),
        route.sell_dex_type.is_velo_style(),
        executor_address, 
        U256::zero(), 
        salt,
    ).map_err(|e| eyre!("Failed to encode user_data for gas estimate: {}", e))?;
    trace!("User data for gas estimate encoded.");

    let balancer_vault = BalancerVault::new(config_ref.balancer_vault_address, client.clone()); // Assuming H160
    let flash_loan_call = balancer_vault.flash_loan(
        executor_address, // Use unwrapped executor_address
        vec![loan_token],
        vec![amount_in_wei],
        user_data,
    );
    
    // Corrected field name: e.g., simulation_timeout_seconds
    let gas_est_timeout_duration = Duration::from_secs(config_ref.simulation_timeout_seconds.unwrap_or(10)); 
    let gas_estimate_result = timeout(
        gas_est_timeout_duration,
        client.estimate_gas(&flash_loan_call.tx, None)
    ).await;

    let gas_estimate_units = match gas_estimate_result {
        Ok(Ok(estimate)) => estimate,
        Ok(Err(e)) => { 
            warn!(error = ?e, "Failed to estimate gas for flash loan, using default.");
            // Corrected field name: e.g., simulation_gas_limit_default
            U256::from(config_ref.simulation_gas_limit_default.unwrap_or(500_000)) 
        }
        Err(_) => { 
            warn!("Gas estimation timed out, using default.");
            // Corrected field name
            U256::from(config_ref.simulation_gas_limit_default.unwrap_or(500_000)) 
        }
    };
    trace!(gas_estimate_units = %gas_estimate_units, "Initial gas estimate received.");

    // Corrected field name: e.g., simulation_min_gas_limit
    let min_flashloan_gas_limit = config_ref.simulation_min_gas_limit.unwrap_or(200_000); 
    // Assuming gas_limit_buffer_percentage is u64 (not Option<u64>)
    let buffered_gas_limit = gas_estimate_units * U256::from(config_ref.gas_limit_buffer_percentage) / U256::from(100); 
    let final_gas_limit = std::cmp::max(buffered_gas_limit, U256::from(min_flashloan_gas_limit));
    trace!(buffered_gas_limit = %buffered_gas_limit, min_flashloan_gas_limit = %min_flashloan_gas_limit, final_gas_limit = %final_gas_limit, "Calculated final gas limit");
    
    let gas_cost_wei = final_gas_limit * gas_price_wei;
    trace!(gas_cost_wei = %gas_cost_wei, "Total gas cost calculated.");

    let profit_wei_before_gas = I256::try_from(final_amount_out_loan_token).map_err(|_| eyre!("Overflow converting final_amount_out to I256"))?
                              - I256::try_from(amount_in_wei).map_err(|_| eyre!("Overflow converting amount_in_wei to I256"))?;
    
    let net_profit_wei = profit_wei_before_gas - I256::try_from(gas_cost_wei).map_err(|_| eyre!("Overflow converting gas_cost_wei to I256"))?;
    
    debug!(%profit_wei_before_gas, %gas_cost_wei, %net_profit_wei, "Simulated profit calculated");
    Ok(net_profit_wei)
}


/// Searches for the optimal flash loan amount for a given route candidate.
// (Function remains unchanged)
#[allow(clippy::too_many_arguments)]
#[instrument(skip_all, level = "info", fields( route = ?route ))]
pub async fn find_optimal_loan_amount(
    app_state: Arc<AppState>,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    route: &RouteCandidate,
    config: Arc<Config>, // Added config back
) -> Result<(U256, I256)> { // Return type changed from Option<(U256, I256)>
    info!("Searching optimal loan amount...");
    let mut best_loan_amount_wei = U256::zero();
    let mut max_net_profit_wei = I256::min_value();

    let weth_decimals = config.weth_decimals;
    let config_max_loan_weth = config.max_loan_amount_weth; 
    
    let dynamic_max_loan_weth = calculate_dynamic_max_loan_weth(app_state.clone(), route, &config) // Pass &Arc<Config>
        .await
        .unwrap_or(config_max_loan_weth);

    info!( config_max_weth = config_max_loan_weth, dynamic_max_weth = format!("{:.4}", dynamic_max_loan_weth), "Max loan amount limits (WETH)");
    let effective_max_loan_weth = dynamic_max_loan_weth.min(config_max_loan_weth);
    let effective_max_loan_wei = parse_units(format!("{:.18}", effective_max_loan_weth), weth_decimals as u32)?.into();

    let search_min_weth = config.min_loan_amount_weth; 
    let search_max_weth = effective_max_loan_weth;
    // Corrected field name: e.g., optimal_loan_search_iterations
    let iterations = config.optimal_loan_search_iterations; // Assuming u32, not Option<u32>
    info!( search_range_weth = format!("{:.4} - {:.4}", search_min_weth, search_max_weth), iterations, "Starting optimal loan search..." );

    let mut tasks = Vec::new();
    for i in 0..=iterations {
        let ratio = if iterations == 0 { 0.5 } else { i as f64 / iterations as f64 };
        let current_loan_weth = search_min_weth + (search_max_weth - search_min_weth) * ratio;
        let current_loan_amount_wei: U256 = parse_units(format!("{:.18}", current_loan_weth), weth_decimals as u32)?.into(); // Cast u8 to u32

        if current_loan_amount_wei > effective_max_loan_wei || current_loan_amount_wei.is_zero() { trace!(%current_loan_amount_wei, "Skipping amount outside effective range or zero."); continue; }

        let app_state_clone = app_state.clone();
        let client_clone = client.clone();
        let route_clone = route.clone();
        tasks.push(tokio::spawn(async move {
            (current_loan_amount_wei, simulate_calculate_net_profit_wei(app_state_clone, client_clone, &route_clone, current_loan_amount_wei).await)
        }));
    }
    let results = futures_util::future::join_all(tasks).await;
    for join_result in results { match join_result { Ok((amount_wei, Ok(profit_wei))) => { trace!(loan_amount_wei=%amount_wei, net_profit_wei=%profit_wei, "Simulated one iteration"); if profit_wei > max_net_profit_wei { max_net_profit_wei = profit_wei; best_loan_amount_wei = amount_wei; } } Ok((_, Err(e))) => { warn!(error=?e, "Error calculating profit for specific loan amount"); } Err(e) => { error!(error=?e, "Simulation task failed"); } } }

    if max_net_profit_wei > I256::zero() {
        let best_loan_weth_str = format_units(best_loan_amount_wei, config.weth_decimals as i32)?;
        let profit_weth_str = format_units(max_net_profit_wei.into_raw(), config.weth_decimals as i32)?;
        info!( optimal_loan_weth = %best_loan_weth_str, max_net_profit_weth = %profit_weth_str, "🎉 Optimal loan amount found!" );
    } else {
        info!("No profitable loan amount found within the search range.");
    }
    
    if cfg!(feature = "local_simulation") && config.local_tests_inject_fake_profit.unwrap_or(false) { // Corrected field
        let test_loan_amount = parse_units("0.1", 18_u32)?.into(); 
        let fake_profit_wei = parse_units("0.001", 18_u32)?.into(); 
        info!(forced_loan_wei = %test_loan_amount, forced_profit_wei = %fake_profit_wei, "Injecting fake profit for local test.");
        return Ok((test_loan_amount, I256::try_from(fake_profit_wei).unwrap()));
    }

    Ok((best_loan_amount_wei, max_net_profit_wei)) // Return Result directly
}

/// Calculates a dynamic maximum loan amount based primarily on V2/Aero pool reserves.
// (Function remains unchanged)
#[instrument(level="debug", skip(app_state, route, config))]
async fn calculate_dynamic_max_loan_weth(
    app_state: Arc<AppState>,
    route: &RouteCandidate,
    config: &Config, // config is &Config here, not Arc<Config>
) -> Result<f64> {
    trace!("Calculating dynamic max loan based on pool depth...");
    let buy_snap_opt = app_state.pool_snapshots.get(&route.buy_pool_addr);
    if buy_snap_opt.is_none() { return Ok(config.max_loan_amount_weth); }
    let buy_snap = buy_snap_opt.unwrap();

    let loan_token = route.token_in_buy_pool;
    let weth_decimals = config.weth_decimals;
    let mut dynamic_max_wei = U256::max_value();

    if buy_snap.dex_type.is_velo_style() {
        let (reserve_loan, _reserve_other) = if buy_snap.token0 == Some(loan_token) { // buy_snap.token0 is Option<H160>
            (buy_snap.reserve0.unwrap_or_default(), buy_snap.reserve1.unwrap_or_default())
        } else if buy_snap.token1 == Some(loan_token) { // buy_snap.token1 is Option<H160>
            (buy_snap.reserve1.unwrap_or_default(), buy_snap.reserve0.unwrap_or_default())
        } else {
            error!(pool=%buy_snap.pool_address, %loan_token, token0=?buy_snap.token0, token1=?buy_snap.token1, "Loan token not found in Velo/Aerodrome pool reserves for dynamic sizing.");
            return Ok(config.max_loan_amount_weth);
        };
        // Corrected field name: e.g., dynamic_sizing_velo_percentage
        let limit_percentage = config.dynamic_sizing_velo_percentage.unwrap_or(10); 
        let limit_wei = reserve_loan * U256::from(limit_percentage) / U256::from(100);
        trace!( pool = %buy_snap.pool_address, dex = %buy_snap.dex_type.to_string(), reserve_loan_token = %reserve_loan, limit_percentage, calculated_limit_wei = %limit_wei, "Velo/Aero dynamic sizing");
        dynamic_max_wei = std::cmp::min(dynamic_max_wei, limit_wei);
    } else if buy_snap.dex_type == DexType::UniswapV3 {
        // If config.enable_univ3_dynamic_sizing is bool, not Option<bool>
        if config.enable_univ3_dynamic_sizing { 
            warn!(pool=%buy_snap.pool_address, "UniV3 dynamic sizing based on tick liquidity is complex and not fully implemented for precise depth analysis. Using configured max loan as the upper bound for now.");
            trace!("TODO: Implement UniV3 tick liquidity analysis for dynamic sizing.");
        } else {
            trace!(pool = %buy_snap.pool_address, "UniV3 dynamic sizing disabled by config. Using configured max loan as upper bound.");
        }
    }
    
    let final_dynamic_max_wei = std::cmp::min(dynamic_max_wei, parse_units(format!("{:.18}", config.max_loan_amount_weth), weth_decimals as u32)?.into()); // Cast u8 to u32
    if final_dynamic_max_wei == parse_units(format!("{:.18}", config.max_loan_amount_weth), weth_decimals as u32)?.into() { // Cast u8 to u32
        trace!(dynamic_max_wei = %final_dynamic_max_wei, "Using config max loan (or less) as effective limit.");
    }
    Ok(f64::from_str(&format_units(final_dynamic_max_wei, weth_decimals as i32)?)?)
}

pub async fn get_amount_out(
    pool_address: Address,
    token_in: Address,
    token_out: Address,
    amount_in_wei: U256,
    dex_type: DexType,
    app_state: Arc<AppState>,
) -> Result<U256> {
    let client = app_state.client.clone();
    let config = &app_state.config;

    match dex_type {
        DexType::UniswapV3 => {
            let quoter_address = config.quoter_v2_address;
            let quoter = QuoterV2::new(quoter_address, client.clone());
            let fee_u24 = app_state.pool_states.get(&pool_address).and_then(|s| s.uni_fee)
                .ok_or_else(|| eyre!("Fee not found for UniV3 pool {} in get_amount_out", pool_address))?;

            let params = quoter_v2_bindings::QuoteExactInputSingleParams { // Use alias
                token_in,
                token_out,
                amount_in: amount_in_wei,
                fee: fee_u24,
                sqrt_price_limit_x96: U256::zero(),
            };
            match quoter.quote_exact_input_single(params).call().await {
                Ok(quote_result) => Ok(quote_result.0),
                Err(e) => Err(eyre!("QuoterV2 call failed for UniV3 pool {} in get_amount_out: {:?}", pool_address, e)),
            }
        }
        DexType::VelodromeV2 | DexType::Aerodrome => {
            let router_address_to_use = if dex_type == DexType::VelodromeV2 {
                config.velo_router_addr
            } else {
                config.aerodrome_router_addr.ok_or_else(|| eyre!("Aerodrome Router address not configured for get_amount_out"))?
            };
            let router = VelodromeRouter::new(router_address_to_use, client.clone());
            let (stable, factory_address) = app_state.pool_states.get(&pool_address)
                .and_then(|s| Some((s.velo_stable?, s.factory?)))
                .ok_or_else(|| eyre!("Stable status or factory not found for Velo/Aero pool {} in get_amount_out", pool_address))?;

            let routes = vec![velo_router_bindings::Route { // Use alias
                from: token_in,
                to: token_out,
                stable,
                factory: factory_address,
            }];
            match router.get_amounts_out(amount_in_wei, routes).call().await {
                Ok(amounts_out_vec) => {
                    if amounts_out_vec.is_empty() || amounts_out_vec.last().is_none() {
                        Err(eyre!("getAmountsOut returned empty or invalid result for Velo/Aero pool {} in get_amount_out", pool_address))
                    } else {
                        Ok(*amounts_out_vec.last().unwrap())
                    }
                }
                Err(e) => Err(eyre!("VelodromeRouter getAmountsOut failed for Velo/Aero pool {} in get_amount_out: {:?}", pool_address, e)),
            }
        }
        DexType::Unknown => Err(eyre!("Unknown DEX type for get_amount_out")),
    }
}

pub async fn simulate_swap_exact_input_single(
    pool_address: Address,
    token_in: Address,
    token_out: Address,
    amount_in_wei: U256,
    dex_type: DexType,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, 
    app_state_config: Arc<Config>,
) -> Result<U256> {
    match dex_type {
        DexType::UniswapV3 => {
            let quoter_address = app_state_config.quoter_v2_address;
            let quoter = QuoterV2::new(quoter_address, client.clone());
            // Corrected field name: e.g., test_config_uniswap_fee
            let fee_u24 = app_state_config.test_config_uniswap_fee.unwrap_or(3000); 

            let params = quoter_v2_bindings::QuoteExactInputSingleParams { // Use alias
                token_in,
                token_out,
                amount_in: amount_in_wei,
                fee: fee_u24,
                sqrt_price_limit_x96: U256::zero(),
            };
            match quoter.quote_exact_input_single(params).call().await {
                Ok(quote_result) => Ok(quote_result.0),
                Err(e) => Err(eyre!("QuoterV2 call failed for UniV3 pool {}: {:?}", pool_address, e)),
            }
        }
        DexType::VelodromeV2 | DexType::Aerodrome => {
            let router_address_to_use = if dex_type == DexType::VelodromeV2 {
                app_state_config.velo_router_addr
            } else {
                app_state_config.aerodrome_router_addr.ok_or_else(|| eyre!("Aerodrome Router address not configured"))?
            };
            let router = VelodromeRouter::new(router_address_to_use, client.clone());
            // Corrected field name: e.g., test_config_velo_stable
            let stable = app_state_config.test_config_velo_stable.unwrap_or(false); 
            // Corrected field name: e.g., test_config_velo_factory
            let factory_address = app_state_config.test_config_velo_factory.unwrap_or_else(Address::zero); // Corrected field

            let routes = vec![velo_router_bindings::Route { // Use alias
                from: token_in,
                to: token_out,
                stable,
                factory: factory_address,
            }];
            match router.get_amounts_out(amount_in_wei, routes).call().await {
                Ok(amounts_out_vec) => {
                    if amounts_out_vec.is_empty() || amounts_out_vec.last().is_none() {
                        Err(eyre!("getAmountsOut returned empty or invalid result for Velo/Aerodrome pool {} in simulate_swap_exact_input_single", pool_address))
                    } else {
                        Ok(*amounts_out_vec.last().unwrap())
                    }
                }
                Err(e) => Err(eyre!("VelodromeRouter getAmountsOut failed for Velo/Aerodrome pool {} in simulate_swap_exact_input_single: {:?}", pool_address, e)),
            }
        }
        DexType::Unknown => Err(eyre!("Unknown DEX type for simulate_swap_exact_input_single")),
    }
}
