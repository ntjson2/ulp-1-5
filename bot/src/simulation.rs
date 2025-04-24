// src/simulation.rs

// --- Imports ---
use ethers::{
    prelude::{Middleware, Provider, Http, SignerMiddleware, LocalWallet},
    utils::{format_units, parse_units}, // Import parse_units
    types::{Address, U256, I256},
};
use eyre::{Result, WrapErr}; // Import WrapErr
use std::sync::Arc;
use tracing::{debug, error, info, instrument, warn}; // Import tracing macros
// Keep f64_to_wei, ToF64Lossy might be used implicitly via dependencies
use crate::utils::{f64_to_wei, ToF64Lossy};

// --- Use statements ---
use crate::bindings::{
    VelodromeRouter, QuoterV2,
    quoter_v2 as quoter_v2_bindings,
    velodrome_router as velo_router_bindings,
};
use crate::gas::estimate_flash_loan_gas;
use crate::encoding::encode_user_data;


// --- simulate_swap function definition ---
#[allow(clippy::too_many_arguments)]
// Add tracing instrument to log entry/exit and arguments at debug level
#[instrument(skip(velo_router, quoter), level = "debug", fields(
    dex = dex_type,
    token_in = %token_in,
    token_out = %token_out,
    amount_in_eth = format_units(amount_in, "ether").unwrap_or_default().as_str(),
    stable = is_velo_route_stable,
    fee = uni_pool_fee
))]
pub async fn simulate_swap<M: Middleware>(
    dex_type: &str,
    token_in: Address,
    token_out: Address,
    amount_in: U256,
    velo_router: &VelodromeRouter<M>, // Can be SignerMiddleware or Provider
    quoter: &QuoterV2<M>,             // Can be SignerMiddleware or Provider
    is_velo_route_stable: bool, // Specific to Velodrome route segment
    uni_pool_fee: u32,          // Specific to Uniswap pool
) -> Result<U256> where M: Send + Sync, M::Error: 'static + Send + Sync { // Ensure Middleware constraints are met
    debug!("Simulating swap..."); // No need to repeat args already in instrument fields
    match dex_type {
        "UniV3" => {
            let params = quoter_v2_bindings::QuoteExactInputSingleParams {
                token_in, token_out, amount_in, fee: uni_pool_fee,
                sqrt_price_limit_x96: U256::zero(), // No price limit for simulation
            };
            // Consider adding a timeout to the call
            let quote_result = quoter.quote_exact_input_single(params).call().await;
            match quote_result {
                // Result tuple: (amountOut, sqrtPriceX96After, initializedTicksCrossed, gasEstimate)
                Ok(output) => {
                    debug!(amount_out = ?output.0, gas_estimate = ?output.3, "UniV3 Quoter simulation successful");
                    Ok(output.0) // Return only amountOut
                },
                Err(e) => {
                    let err_msg = format!("UniV3 Quoter simulation failed: {:?}", e);
                    error!("{}", err_msg);
                    // Wrap the ethers::contract::ContractError into eyre::Report
                    Err(eyre::Report::from(e).wrap_err(err_msg))
                },
            }
        }
        "VeloV2" => {
            // Construct the route for Velodrome
            let routes = vec![velo_router_bindings::Route {
                 from: token_in, to: token_out, stable: is_velo_route_stable,
                 // Factory address is typically ignored by getAmountsOut but required by struct
                 factory: Address::zero(), // Use zero address or a placeholder if needed
            }];
            // Consider adding a timeout
            match velo_router.get_amounts_out(amount_in, routes).call().await {
                Ok(amounts_out) => {
                    // amounts_out should be [amountIn, amountOut]
                    if amounts_out.len() >= 2 {
                        debug!(amounts_out = ?amounts_out, "VeloV2 getAmountsOut simulation successful");
                        Ok(amounts_out[1]) // Return the second element which is amountOut
                    } else {
                        let err_msg = format!("VeloV2 getAmountsOut returned unexpected vector length: {:?}", amounts_out);
                        error!("{}", err_msg);
                        Err(eyre::eyre!(err_msg))
                    }
                },
                Err(e) => {
                     let err_msg = format!("VeloV2 simulation failed: {:?}", e);
                     error!("{}", err_msg);
                     Err(eyre::Report::from(e).wrap_err(err_msg))
                },
            }
        }
        _ => {
             let err_msg = format!("Unsupported DEX type for simulation: {}", dex_type);
             error!("{}", err_msg);
             Err(eyre::eyre!(err_msg))
        }
    }
}

// --- Profit Calculation Helper ---
#[allow(clippy::too_many_arguments)]
// Instrument the entire function call
#[instrument(skip_all, level = "debug", fields(
    buy_dex = buy_dex,
    sell_dex = sell_dex,
    amount_in_eth = format_units(amount_in_wei, "ether").unwrap_or_default().as_str()
))]
pub async fn calculate_net_profit(
    // Use Arc for client as it might be cloned for gas estimation
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    amount_in_wei: U256,
    token_in: Address, // Loan token (e.g., WETH)
    token_out: Address, // Intermediate token (e.g., USDC)
    buy_dex: &str,    // DEX for TokenIn -> TokenOut (e.g., "UniV3")
    sell_dex: &str,   // DEX for TokenOut -> TokenIn (e.g., "VeloV2")
    // Pool specific details needed for simulation
    buy_dex_pool_addr: Address, // Needed for context, maybe fees/stability later
    sell_dex_pool_addr: Address, // Needed for context
    buy_dex_stable: bool, // Velo specific
    sell_dex_stable: bool, // Velo specific
    buy_dex_fee: u32,     // UniV3 specific
    sell_dex_fee: u32,    // UniV3 specific
    // Contract instances (borrow them, don't need ownership here)
    velo_router: &VelodromeRouter<SignerMiddleware<Provider<Http>, LocalWallet>>,
    uni_quoter: &QuoterV2<SignerMiddleware<Provider<Http>, LocalWallet>>,
    // Addresses for gas estimation and execution
    arb_executor_address: Address,
    balancer_vault_address: Address,
    velo_router_address: Address, // Separate from instance for encoding
    // Execution path details for userData and gas estimation
    pool_a_addr: Address, // Should match buy_dex_pool_addr if buy is first swap
    pool_b_addr: Address, // Should match sell_dex_pool_addr if sell is second swap
    zero_for_one_a: bool, // Direction of swap A (token_in -> token_out ?)
    is_a_velo: bool,      // Is pool A Velo?
    is_b_velo: bool,      // Is pool B Velo?
    // Config values
    flash_loan_fee_rate: f64,
    gas_price_gwei: f64, // Pass current gas price estimate (in GWEI)
    gas_limit_buffer_percentage: u64, // Pass buffer percentage
    min_flashloan_gas_limit: u64, // Pass minimum gas limit
 ) -> Result<I256> { // Return net profit in wei

    debug!("Calculating net profit for {} -> {} -> {}", token_in, token_out, token_in);

    // 1. Simulate Swap 1 (Buy): TokenIn -> TokenOut
    let amount_out_intermediate_wei = match simulate_swap(
        buy_dex, token_in, token_out, amount_in_wei,
        velo_router, uni_quoter, buy_dex_stable, buy_dex_fee,
    ).await {
        Ok(amount) => amount,
        Err(e) => {
            warn!("Swap 1 ({} -> {}) simulation failed: {}", token_in, token_out, e);
            // Return specific value indicating simulation failure, distinct from negative profit
            return Ok(I256::min_value()); // Use min_value to signal failure/unprofitability upstream
        }
    };

    if amount_out_intermediate_wei.is_zero() {
        debug!("Swap 1 ({} -> {}) simulation resulted in zero output.", token_in, token_out);
        return Ok(I256::min_value()); // Not profitable if first swap yields zero
    }
    debug!(intermediate_amount_wei = %amount_out_intermediate_wei, "Swap 1 simulated");

    // 2. Simulate Swap 2 (Sell): TokenOut -> TokenIn
    let final_amount_out_wei = match simulate_swap(
        sell_dex, token_out, token_in, amount_out_intermediate_wei,
        velo_router, uni_quoter, sell_dex_stable, sell_dex_fee,
    ).await {
        Ok(amount) => amount,
        Err(e) => {
            warn!("Swap 2 ({} -> {}) simulation failed: {}", token_out, token_in, e);
            return Ok(I256::min_value());
        }
    };
    debug!(final_amount_wei = %final_amount_out_wei, "Swap 2 simulated");


    // 3. Calculate Gross Profit (before costs)
    // Use I256 for potential negative profit
    let gross_profit_wei = I256::from_raw(final_amount_out_wei) - I256::from_raw(amount_in_wei);
    let gross_profit_eth = format_units(gross_profit_wei.abs().into_raw(), "ether").unwrap_or_default(); // Format absolute value
    debug!(gross_profit_wei = %gross_profit_wei, gross_profit_eth = %gross_profit_eth, "Gross profit calculated");

    if gross_profit_wei <= I256::zero() {
         debug!("Gross profit is not positive, skipping cost calculation.");
         return Ok(gross_profit_wei); // Not profitable even before costs
    }

    // 4. Estimate Gas Cost
    // Use the passed gas_price_gwei for calculation
    let gas_price_wei = match parse_units(gas_price_gwei, "gwei") {
        Ok(U256::Number(n)) => n, // Ethers returns ParseUnits::U256 which wraps the value
        Ok(_) => return Err(eyre::eyre!("Parsed gas price units resulted in non-U256 type")), // Should not happen
        Err(e) => return Err(eyre::eyre!("Failed to parse gas_price_gwei {} to wei: {}", gas_price_gwei, e)),
    };

    // Encode userData needed for the actual execution path
    let user_data = encode_user_data(
        pool_a_addr, pool_b_addr, token_out,
        zero_for_one_a, is_a_velo, is_b_velo, velo_router_address,
    ).wrap_err("Failed to encode user data for gas estimation")?;

    let estimated_gas_units = match estimate_flash_loan_gas(
        client.clone(), balancer_vault_address, arb_executor_address,
        token_in, amount_in_wei, user_data.clone(), // Clone user_data for potential reuse
    ).await {
        Ok(gas) => gas,
        Err(e) => {
            warn!("Gas estimation failed: {}. Assuming unprofitable.", e);
            // Decide how to handle: return error, or assume high cost?
            // For now, assume it makes the trade unprofitable by returning min_value
             return Ok(I256::min_value());
        }
    };

    // Apply buffer and minimum gas limit
    let gas_limit_with_buffer = estimated_gas_units * (100 + gas_limit_buffer_percentage) / 100;
    let final_gas_limit = std::cmp::max(gas_limit_with_buffer, U256::from(min_flashloan_gas_limit));

    let gas_cost_wei = gas_price_wei * final_gas_limit;
    debug!(
        estimated_gas = %estimated_gas_units,
        gas_limit_buffer = %gas_limit_buffer_percentage,
        min_gas_limit = %min_flashloan_gas_limit,
        final_gas_limit = %final_gas_limit,
        gas_price_gwei = %gas_price_gwei,
        gas_cost_wei = %gas_cost_wei,
        "Gas cost estimated"
    );

    // 5. Calculate Flash Loan Fee (Balancer V2 is currently 0)
    // Keep calculation for future flexibility or other providers
    let fee_numerator = U256::from((flash_loan_fee_rate * 10000.0).round() as u128);
    let fee_denominator = U256::from(10000);
    let flash_loan_fee_wei = if fee_denominator.is_zero() { U256::zero() } else { amount_in_wei * fee_numerator / fee_denominator };
    debug!(flash_loan_fee_wei = %flash_loan_fee_wei, "Flash loan fee calculated");

    // 6. Calculate Total Cost
    let total_cost_wei = gas_cost_wei + flash_loan_fee_wei;
    let total_cost_eth = format_units(total_cost_wei, "ether").unwrap_or_default();
    debug!(total_cost_wei = %total_cost_wei, total_cost_eth = %total_cost_eth, "Total cost calculated");

    // 7. Calculate Net Profit
    // Subtract total cost (as U256 converted to I256) from gross profit
    let net_profit_wei = gross_profit_wei - I256::from_raw(total_cost_wei); // Potential underflow if cost > gross profit
    let net_profit_eth = format_units(net_profit_wei.abs().into_raw(), "ether").unwrap_or_default();
    debug!(net_profit_wei = %net_profit_wei, net_profit_eth = %net_profit_eth, "Net profit calculated");

    Ok(net_profit_wei)
}


// --- Optimal Loan Amount Search Function ---
#[allow(clippy::too_many_arguments)]
// Instrument search function at info level
#[instrument(skip_all, level = "info", fields(
    min_loan_eth = min_loan_amount_weth,
    max_loan_eth = max_loan_amount_weth,
    iterations = iterations,
    buy_dex = buy_dex,
    sell_dex = sell_dex,
    token_in = %token_in,
    token_out = %token_out,
))]
pub async fn find_optimal_loan_amount(
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    min_loan_amount_weth: f64,
    max_loan_amount_weth: f64,
    iterations: u32,
    // Tokens & Decimals
    token_in: Address, token_out: Address, weth_decimals: u8,
    // Fees & Gas
    flash_loan_fee_rate: f64,
    gas_price_gwei: f64, // Pass current gas price estimate (GWEI)
    gas_limit_buffer_percentage: u64,
    min_flashloan_gas_limit: u64,
    // DEX Info
    buy_dex: &str, sell_dex: &str,
    buy_dex_pool_addr: Address, sell_dex_pool_addr: Address,
    buy_dex_stable: bool, sell_dex_stable: bool,
    buy_dex_fee: u32, sell_dex_fee: u32,
    // Contract Instances & Addresses (Use Arc for cloning into tasks)
    velo_router: Arc<VelodromeRouter<SignerMiddleware<Provider<Http>, LocalWallet>>>,
    uni_quoter: Arc<QuoterV2<SignerMiddleware<Provider<Http>, LocalWallet>>>,
    arb_executor_address: Address,
    balancer_vault_address: Address,
    velo_router_address: Address,
    // Execution path details
    pool_a_addr: Address, pool_b_addr: Address,
    zero_for_one_a: bool, is_a_velo: bool, is_b_velo: bool,
) -> Result<Option<(U256, I256)>> { // Returns (Optimal Amount Wei, Max Profit Wei)
    info!("Searching for optimal loan amount...");

    let mut best_amount_wei = U256::zero();
    // Initialize max_profit_wei to a very small negative number to ensure any positive profit is chosen
    // Or use I256::min_value() to clearly distinguish from zero or small negative profits
    let mut max_profit_wei = I256::min_value();

    // Convert loan amounts from WETH (f64) to wei (U256)
    let min_loan_wei = f64_to_wei(min_loan_amount_weth, weth_decimals as u32)
        .wrap_err("Failed to convert min_loan_amount_weth to wei")?;
    let max_loan_wei = f64_to_wei(max_loan_amount_weth, weth_decimals as u32)
        .wrap_err("Failed to convert max_loan_amount_weth to wei")?;

    // Basic validation of search range
    if min_loan_wei >= max_loan_wei || iterations < 1 || min_loan_amount_weth <= 0.0 {
        warn!(?min_loan_wei, ?max_loan_wei, iterations, "Invalid search range or iterations. Testing only Min amount if valid.");
        // Only test min amount if it's valid and less than max
        if min_loan_wei > U256::zero() && min_loan_wei < max_loan_wei {
            // Pass all necessary arguments to calculate_net_profit
            let profit_at_min = calculate_net_profit(
                client.clone(), min_loan_wei, token_in, token_out, buy_dex, sell_dex,
                buy_dex_pool_addr, sell_dex_pool_addr, buy_dex_stable, sell_dex_stable,
                buy_dex_fee, sell_dex_fee, &velo_router, &uni_quoter, arb_executor_address,
                balancer_vault_address, velo_router_address, pool_a_addr, pool_b_addr,
                zero_for_one_a, is_a_velo, is_b_velo, flash_loan_fee_rate, gas_price_gwei,
                gas_limit_buffer_percentage, min_flashloan_gas_limit,
            ).await?;
            // Check if the profit is strictly positive
            return if profit_at_min > I256::zero() { Ok(Some((min_loan_wei, profit_at_min))) } else { Ok(None) };
        } else {
            return Ok(None); // Min amount is invalid or range is zero/negative
        }
    }

    // --- Iterative Sampling (Parallelized) ---
    let mut tasks = vec![];
    info!(num_tasks = iterations, "Spawning parallel profit calculation tasks...");
    for i in 0..iterations {
        // Calculate the sample amount for this iteration using linear interpolation
        let ratio = if iterations <= 1 { 0.0 } else { i as f64 / (iterations - 1) as f64 };
        let sample_amount_f64 = min_loan_amount_weth + (max_loan_amount_weth - min_loan_amount_weth) * ratio;
        let current_amount_wei = match f64_to_wei(sample_amount_f64, weth_decimals as u32) {
            Ok(amount) => amount,
            Err(e) => {
                warn!(amount = sample_amount_f64, error = ?e, "Could not convert sample amount to wei, skipping iteration {}", i);
                continue; // Skip if conversion fails
            }
        };

        // Basic validation for the calculated wei amount
        if current_amount_wei < min_loan_wei || current_amount_wei > max_loan_wei || current_amount_wei.is_zero() {
             debug!(iteration = i, amount_wei = %current_amount_wei, "Skipping iteration: Amount out of bounds or zero");
             continue;
        }

        // --- Clone Arcs and necessary data for the spawned task ---
        let client_clone = client.clone();
        let velo_router_clone = velo_router.clone(); // Clone the Arc
        let uni_quoter_clone = uni_quoter.clone();   // Clone the Arc
        let buy_dex_str = buy_dex.to_string(); // Clone String for task
        let sell_dex_str = sell_dex.to_string(); // Clone String for task


        // Spawn a task to calculate profit for this amount
        tasks.push(tokio::spawn(async move {
            // Call calculate_net_profit with all required arguments
            let profit_result = calculate_net_profit(
                client_clone, current_amount_wei, token_in, token_out, &buy_dex_str, &sell_dex_str,
                buy_dex_pool_addr, sell_dex_pool_addr, buy_dex_stable, sell_dex_stable,
                buy_dex_fee, sell_dex_fee, &velo_router_clone, &uni_quoter_clone, arb_executor_address,
                balancer_vault_address, velo_router_address, pool_a_addr, pool_b_addr,
                zero_for_one_a, is_a_velo, is_b_velo, flash_loan_fee_rate, gas_price_gwei,
                gas_limit_buffer_percentage, min_flashloan_gas_limit,
            ).await;
            // Return the amount tested along with the result
            (current_amount_wei, profit_result)
        }));

    } // End loop spawning tasks

    // --- Collect results from tasks ---
    let results = futures_util::future::join_all(tasks).await;
    info!("Collected results from {} simulation tasks.", results.len());

    for (i, join_result) in results.into_iter().enumerate() { // Use enumerate for task index
        match join_result {
            Ok((amount_tested, profit_result)) => {
                match profit_result {
                    // Profit calculation succeeded
                    Ok(profit) => {
                         debug!(task = i, amount_wei = %amount_tested, profit_wei = %profit, "Task completed.");
                        // Check if this profit is better than the current best
                        if profit > max_profit_wei {
                            max_profit_wei = profit;
                            best_amount_wei = amount_tested;
                            debug!(task = i, amount_wei = %amount_tested, profit_wei = %profit, "New best profit found");
                        }
                    }
                    // Profit calculation returned an error (e.g., simulation failed, gas estimation failed)
                    Err(e) => {
                         warn!(task = i, amount_wei = %amount_tested, error = ?e, "Profit calculation failed within task");
                         // We don't update best profit if calculation fails
                    }
                }
            }
            // Task panicked or was cancelled
            Err(e) => {
                error!(task = i, error = ?e, "Task panicked or was cancelled during profit calculation");
            }
        }
    } // End processing results

    // --- Final Result ---
    // Check if the best profit found is strictly greater than zero
    if max_profit_wei > I256::zero() {
        // Format amounts for logging
        let best_amount_eth = format_units(best_amount_wei, "ether").unwrap_or_else(|_| "N/A".to_string());
        let max_profit_eth = format_units(max_profit_wei.into_raw(), "ether").unwrap_or_else(|_| "N/A".to_string());
        info!(
            optimal_amount_eth = %best_amount_eth,
            optimal_amount_wei = %best_amount_wei,
            estimated_profit_eth = %max_profit_eth,
            estimated_profit_wei = %max_profit_wei,
            "Optimal Amount Search Complete. Profitable opportunity found."
        );
        Ok(Some((best_amount_wei, max_profit_wei)))
    } else {
        info!("Optimal Amount Search Complete. No profitable amount found in the specified range.");
        Ok(None) // Indicate no profitable opportunity found
    }
}

// END OF FILE: bot/src/simulation.rs