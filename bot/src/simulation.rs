// bot/src/simulation.rs
//! Handles off-chain simulation of arbitrage routes to determine profitability
//! and optimal loan amounts before attempting on-chain execution.

use crate::{
    bindings::{
        // Contract structs will be in their respective modules, e.g.,
        // crate::bindings::quoter_v2::QuoterV2,
        // crate::bindings::velodrome_router::VelodromeRouter,
        // crate::bindings::arbitrage_executor::ArbitrageExecutor,
        // crate::bindings::balancer_vault::BalancerVault,
    },
    config::Config,
    state::{AppState, DexType, PoolSnapshot, /*PoolInfo, RouteDetails*/}, // PoolInfo, RouteDetails commented out if not defined/used
    // path_optimizer::RouteCandidate, // If RouteDetails is actually RouteCandidate
};
use ethers::{
    core::types::{Address, Bytes, I256, U256, H160, H256}, // Removed U64 as it's aliased or unused directly
    middleware::SignerMiddleware,
    providers::{Http, Middleware, Provider},
    signers::{LocalWallet, Signer},
    utils::{format_units, parse_units, ConversionError},
};
use eyre::{eyre, Result, Report};
use std::sync::Arc;
use tracing::{debug, info, instrument, trace, warn};

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
    token_in: Address,
    token_out: Address,
    amount_in: U256,
    dex_type: DexType,
    sqrt_price_limit_x96: Option<U256>, // For UniV3
    // config: &Config, // Added config if needed for router/quoter addresses directly
) -> Result<U256> {
    let config = &app_state.config;
    let quoter_or_router_address = get_quoter_router_address(config, dex_type)?;

    let actual_token_in = token_in;
    let actual_token_out = token_out;

    if config.use_local_anvil_node.unwrap_or(false) && dex_type == DexType::VelodromeV2 {
        // Attempt to call hardcoded Velodrome V2 Router implementation for Anvil
        // Fallback to estimation if it fails
        // This logic might need adjustment based on actual Velodrome V2 simulation needs
        let velo_router_impl_addr: Address = "0xYOUR_VELO_ROUTER_IMPL_ADDR_HERE".parse()
            .map_err(|_| eyre!("Invalid Velodrome Router implementation address"))?; // Replace with actual address
        
        let router_contract = crate::bindings::velodrome_router::VelodromeRouter::new(velo_router_impl_addr, client.clone());
        let routes_array = [crate::bindings::velodrome_router::Route {
            from: actual_token_in,
            to: actual_token_out,
            stable: false, // Assuming non-stable for simplicity, adjust as needed
            factory: config.test_config_velo_factory.unwrap_or_default(), // Adjust as needed
        }];
        match router_contract.get_amounts_out(amount_in, routes_array.to_vec()).call().await {
            Ok(amounts_out_vec) => {
                if let Some(amount_out) = amounts_out_vec.last() {
                    return Ok(*amount_out);
                } else {
                    return Err(eyre!("Velodrome getAmountsOut returned empty vector on Anvil"));
                }
            }
            Err(e) => {
                warn!("Failed to call Velodrome Router impl on Anvil ({}). Falling back to rough estimation.", e);
                // Fallback rough estimation for Velodrome V2 on Anvil
                // This is a placeholder: replace with a more accurate model if possible
                return Ok(amount_in * U256::from(99) / U256::from(100)); // e.g., 1% slippage
            }
        }
    }


    match dex_type {
        DexType::UniswapV3 => {
            let quoter_addr = quoter_or_router_address; 
            let quoter_contract = crate::bindings::quoter_v2::QuoterV2::new(quoter_addr, client.clone());
            let params = crate::bindings::quoter_v2::QuoteExactInputSingleParams { 
                token_in: actual_token_in, 
                token_out: actual_token_out, 
                amount_in,
                fee: config.test_config_uniswap_fee.unwrap_or(3000), // Example fee
                sqrt_price_limit_x96: sqrt_price_limit_x96.unwrap_or_default(),
            };
            // The call to quote_exact_input_single returns a tuple. We need the first element (amount_out).
            let quote_result = quoter_contract.quote_exact_input_single(params).call().await?;
            Ok(quote_result.0) // Assuming amount_out is the first element of the tuple
        }
        DexType::VelodromeV2 | DexType::Aerodrome => {
            let router_addr = quoter_or_router_address; 
            let router_contract = crate::bindings::velodrome_router::VelodromeRouter::new(router_addr, client.clone());
            let routes_array = [crate::bindings::velodrome_router::Route { 
                from: actual_token_in, 
                to: actual_token_out, 
                stable: config.test_config_velo_stable.unwrap_or(false), // Example: use config
                factory: config.test_config_velo_factory.unwrap_or_default(), // Example: use config
            }];
            let amounts_out_vec = router_contract.get_amounts_out(amount_in, routes_array.to_vec()).call().await?;
            amounts_out_vec.last().cloned().ok_or_else(|| eyre!("getAmountsOut returned empty vector"))
        }
        DexType::Balancer => Err(eyre!("Balancer simulation not directly implemented in simulate_swap")),
        DexType::Unknown => Err(eyre!("Unknown DEX type for simulation")),
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
    wallet_address: Address,
    executor_address: Address,
) -> Result<I256> {
    let config = &app_state.config;
    let amount_out_token0_wei = simulate_swap(
        app_state.clone(),
        client.clone(),
        route.sell_pool_addr,
        route.token0,
        route.token1,
        amount_in_wei,
        route.sell_dex_type,
        None,
    )
    .await
    .map_err(|e| eyre!("Simulate sell swap failed: {}", e))?;

    let final_amount_weth_wei = simulate_swap(
        app_state.clone(),
        client.clone(),
        route.buy_pool_addr,
        route.token1,
        route.token0,
        amount_out_token0_wei,
        route.buy_dex_type,
        None,
    )
    .await
    .map_err(|e| eyre!("Simulate buy swap failed: {}", e))?;
    
    let gross_profit_wei = I256::try_from(final_amount_weth_wei).map_err(|_| eyre!("Gross profit conversion error"))? 
                         - I256::try_from(amount_in_wei).map_err(|_| eyre!("Amount in conversion error"))?;

    if gross_profit_wei <= I256::zero() {
        return Ok(gross_profit_wei);
    }

    let gas_cost_wei_u256 = calculate_gas_cost_for_flash_loan(
        app_state.clone(),
        client.clone(),
        route,
        amount_in_wei,
        wallet_address,
        executor_address,
        config.simulation_gas_price_gwei,
    )
    .await?;
    
    let gas_cost_wei = I256::try_from(gas_cost_wei_u256).map_err(|_| eyre!("Gas cost U256 to I256 conversion error"))?;
    let net_profit_wei = gross_profit_wei - gas_cost_wei;
    
    debug!(
        route_id = %route.id(),
        loan_amount_eth = %format_units(amount_in_wei, "ether").unwrap_or_default(),
        gross_profit_eth = %format_units(gross_profit_wei.abs_diff(U256::zero()), "ether").unwrap_or_default(),
        gas_cost_eth = %format_units(gas_cost_wei.abs_diff(U256::zero()), "ether").unwrap_or_default(),
        net_profit_eth = %format_units(net_profit_wei.abs_diff(U256::zero()), "ether").unwrap_or_default(),
        "Calculated net profit"
    );

    Ok(net_profit_wei)
}


/// Searches for the optimal flash loan amount for a given route candidate.
// (Function remains unchanged)
#[allow(clippy::too_many_arguments)]
#[instrument(skip_all, level = "info", fields( route_id = %route.id() ))] // Corrected: route.id()
pub async fn find_optimal_loan_amount(
    app_state: Arc<AppState>,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    route: &RouteCandidate,
    wallet_address: Address,
    executor_address: Address,
) -> Result<(U256, I256)> {
    let config = &app_state.config;
    let iterations = config.optimal_loan_search_iterations.unwrap_or(20);
    let weth_decimals = config.weth_decimals.unwrap_or(18);

    let min_loan_weth = config.min_loan_amount_weth.unwrap_or(0.01);
    let max_loan_weth = dynamic_max_loan_weth(app_state.clone(), route, config).await?;
    
    let min_loan_wei: U256 = parse_units(min_loan_weth, weth_decimals as u32)?.into();
    let max_loan_wei: U256 = parse_units(max_loan_weth, weth_decimals as u32)?.into();

    let mut optimal_loan_wei = U256::zero(); // Initialize
    let mut max_profit_wei = I256::min_value(); // Initialize

    if min_loan_wei >= max_loan_wei {
        warn!(route_id = %route.id(), min_loan_wei = %min_loan_wei, max_loan_wei = %max_loan_wei, "Min loan is >= max loan, skipping search.");
        return Ok((optimal_loan_wei, max_profit_wei));
    }

    let step_size = if iterations > 0 {(max_loan_wei - min_loan_wei) / U256::from(iterations)} else {U256::zero()};

    if step_size == U256::zero() && iterations > 1 {
        for test_loan_wei_val in [min_loan_wei, max_loan_wei].iter() {
            let test_loan_wei = *test_loan_wei_val;
            if test_loan_wei.is_zero() { continue; }
            match simulate_calculate_net_profit_wei(app_state.clone(), client.clone(), route, test_loan_wei, wallet_address, executor_address).await {
                Ok(profit) => {
                    trace!(route_id = %route.id(), loan_amount_wei = %test_loan_wei, profit_wei = %profit, "Simulated profit for loan amount");
                    if profit > max_profit_wei {
                        max_profit_wei = profit;
                        optimal_loan_wei = test_loan_wei;
                        trace!(route_id = %route.id(), new_optimal_loan_wei = %optimal_loan_wei, new_max_profit_wei = %max_profit_wei, "New optimal loan found");
                    }
                }
                Err(e) => {
                    warn!(route_id = %route.id(), loan_amount_wei = %test_loan_wei, "Simulation error during optimal loan search: {:?}", e);
                }
            }
        }
    } else if iterations > 0 { // Ensure iterations is positive to avoid infinite loop if step_size is 0
        for i in 0..=iterations {
            let test_loan_wei = min_loan_wei + step_size * U256::from(i);
            let current_test_loan = if test_loan_wei > max_loan_wei { max_loan_wei } else { test_loan_wei };
            if current_test_loan.is_zero() && i > 0 { continue; }

            match simulate_calculate_net_profit_wei(app_state.clone(), client.clone(), route, current_test_loan, wallet_address, executor_address).await {
                Ok(profit) => {
                    trace!(route_id = %route.id(), loan_amount_wei = %current_test_loan, profit_wei = %profit, "Simulated profit for loan amount");
                    if profit > max_profit_wei {
                        max_profit_wei = profit;
                        optimal_loan_wei = current_test_loan;
                        trace!(route_id = %route.id(), new_optimal_loan_wei = %optimal_loan_wei, new_max_profit_wei = %max_profit_wei, "New optimal loan found");
                    }
                }
                Err(e) => {
                    warn!(route_id = %route.id(), loan_amount_wei = %current_test_loan, "Simulation error during optimal loan search: {:?}", e);
                }
            }
        }
    }


    if max_profit_wei <= I256::zero() && config.local_tests_inject_fake_profit.unwrap_or(false) {
        info!(route_id = %route.id(), "Injecting fake profit for local test as no actual profit was found.");
        max_profit_wei = I256::from_dec_str("100000000000000").unwrap_or_default();
        optimal_loan_wei = parse_units(0.1, weth_decimals as u32)?.into();
    }
    
    info!(route_id = %route.id(), optimal_loan_eth = %format_units(optimal_loan_wei, "ether").unwrap_or_default(), max_profit_eth = %format_units(max_profit_wei.abs_diff(U256::zero()), "ether").unwrap_or_default(), "Optimal loan search complete");
    Ok((optimal_loan_wei, max_profit_wei))
}

/// Calculates the gas cost for executing a flash loan and arbitrage on-chain.
/// This function is used to estimate whether a given arbitrage opportunity is profitable
/// after accounting for gas costs.
async fn calculate_gas_cost_for_flash_loan(
    app_state: Arc<AppState>,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    route: &RouteCandidate,
    loan_amount_wei: U256, // Corrected name
    wallet_address: Address,
    executor_address: Address,
    gas_price_gwei_override: Option<f64>,
) -> Result<U256> { // Return U256 for gas cost
    let config = &app_state.config;
    let gas_price_gwei_str = gas_price_gwei_override
        .map(|p| p.to_string())
        .unwrap_or_else(|| config.simulation_gas_price_gwei.unwrap_or(1.0).to_string());

    let gas_price_wei: U256 = parse_units(gas_price_gwei_str, "gwei")
        .map_err(|e: ConversionError| eyre!("Failed to parse gas_price_gwei_override: {}", e))? 
        .into(); // Added .into()

    let config_ref = &app_state.config;
    let executor_contract = crate::bindings::arbitrage_executor::ArbitrageExecutor::new(executor_address, client.clone());
    
    let salt_val = U256::from(rand::random::<u64>());

    let call_data_result = if route.buy_dex_type == DexType::Balancer || route.sell_dex_type == DexType::Balancer {
        let balancer_vault_addr = config_ref.balancer_vault_address.ok_or_else(|| eyre!("Balancer vault address not configured for gas calculation"))?;
        executor_contract.execute_flash_arbitrage_balancer( 
            route.buy_pool_addr,
            route.sell_pool_addr,
            route.token0,
            route.token1,
            loan_amount_wei, // Use corrected name
            balancer_vault_addr,
            route.buy_dex_type as u8, 
            route.sell_dex_type as u8, 
            salt_val
        ).calldata()
    } else {
        executor_contract.execute_flash_arbitrage(
            route.buy_pool_addr,
            route.sell_pool_addr,
            route.token0,
            route.token1,
            loan_amount_wei, // Use corrected name
            route.buy_dex_type as u8, 
            route.sell_dex_type as u8, 
            salt_val
        ).calldata()
    };

    let call_data = call_data_result.ok_or_else(|| eyre!("Failed to get calldata for arbitrage execution"))?;

    let gas_limit_estimation = client
        .estimate_gas(
            &ethers::types::transaction::eip2718::TypedTransaction::Eip1559(
                ethers::types::Eip1559TransactionRequest {
                    to: Some(executor_address.into()),
                    from: Some(wallet_address),
                    data: Some(call_data),
                    value: Some(U256::zero()),
                    chain_id: Some(config.chain_id.unwrap_or(1).into()),
                    max_priority_fee_per_gas: Some(parse_units(config.max_priority_fee_per_gas_gwei.unwrap_or(1.0), "gwei")?.into()),
                    max_fee_per_gas: Some(gas_price_wei),
                    gas: None,
                    nonce: None,
                    access_list: Default::default(),
                },
            ),
            None,
        )
        .await
        .map_err(|e| eyre!("Gas estimation failed: {}", e))?;

    let final_gas_limit_u256 = gas_limit_estimation * U256::from(config.gas_limit_buffer_percentage + 100) / U256::from(100);
    let gas_cost = final_gas_limit_u256 * gas_price_wei; 
    Ok(gas_cost) // Return U256
}

/// Simulates the profit from a swap on a DEX, used to determine the viability of an arbitrage route.
#[instrument(level="trace", skip(app_state, client, route), fields(route_id = %route.id(), loan_amount_wei = %loan_amount_wei))] // Corrected: route.id()
pub async fn simulate_swap_profit(
    app_state: Arc<AppState>,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    route: &RouteCandidate,
    loan_amount_wei: U256,
    wallet_address: Address,
    executor_address: Address,
) -> Result<I256> {
    simulate_calculate_net_profit_wei(
        app_state,
        client,
        route,
        loan_amount_wei,
        wallet_address,
        executor_address,
    ).await
}


// Helper function to get Balancer vault instance
#[allow(dead_code)] 
fn get_balancer_vault(
    config: &Config, 
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
) -> Result<crate::bindings::balancer_vault::BalancerVault<SignerMiddleware<Provider<Http>, LocalWallet>>> { 
    let balancer_vault_addr = config.balancer_vault_address.ok_or_else(|| eyre!("Balancer vault address not configured"))?; 
    Ok(crate::bindings::balancer_vault::BalancerVault::new(balancer_vault_addr, client.clone()))
}

// Definition for get_quoter_router_address (restored and corrected)
fn get_quoter_router_address(
    config: &Config, 
    dex_type: DexType,
) -> Result<Address> {
    match dex_type {
        DexType::UniswapV3 => config.uniswap_v3_quoter_v2_address.ok_or_else(|| eyre!("Uniswap V3 Quoter V2 address not configured")),
        DexType::VelodromeV2 => config.velodrome_router_address.ok_or_else(|| eyre!("Velodrome Router address not configured")),
        DexType::Aerodrome => config.aerodrome_router_addr.ok_or_else(|| eyre!("Aerodrome Router address not configured")),
        DexType::Balancer => Err(eyre!("Balancer does not use a quoter/router in this context for get_quoter_router_address")),
        DexType::Unknown => Err(eyre!("Unknown DEX type for quoter/router address")),
    }
}

// All subsequent duplicate definitions of the functions above should be REMOVED from this file.
// For example, the simulate_swap starting at line 711, simulate_calculate_net_profit_wei at 795, etc.
// This addresses all E0428 errors.