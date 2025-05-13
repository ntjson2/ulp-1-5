// bot/src/simulation.rs
//! Handles off-chain simulation of arbitrage routes to determine profitability
//! and optimal loan amounts before attempting on-chain execution.

use crate::bindings::{
    quoter_v2 as quoter_v2_bindings,
    velodrome_router as velo_router_bindings,
    QuoterV2,
    VelodromeRouter,
};
use crate::config::Config;
use crate::gas::estimate_flash_loan_gas;
use crate::state::{AppState, DexType, PoolSnapshot};
use crate::path_optimizer::RouteCandidate;
use crate::utils::{f64_to_wei, ToF64Lossy};
use ethers::{
    contract::ContractError,
    prelude::{Http, LocalWallet, Provider, SignerMiddleware},
    types::{Address, I256, U256, Selector}, // Removed unused Bytes
    utils::{format_units, parse_units},
};
use eyre::{eyre, Result, WrapErr};
use hex;
use std::sync::Arc;
use tokio::time::{timeout, Duration};
use tracing::{debug, error, info, instrument, trace, warn};
use std::str::FromStr;

// Configuration Constants for Simulation
const V2_RESERVE_PERCENTAGE_LIMIT: u64 = 5;

#[cfg(feature = "local_simulation")]
const VELO_ROUTER_IMPL_ADDR_FOR_SIM: &str = "0xa062aE8A9c5e11aaA026fc2670B0D65cCc8B2858";
#[cfg(feature = "local_simulation")]
const PAIR_DOES_NOT_EXIST_SELECTOR_STR: &str = "9a73ab46";


/// Simulates a single swap on a DEX using appropriate on-chain query methods.
// (Function remains unchanged)
#[allow(clippy::too_many_arguments)]
#[instrument(skip(app_state, client), level = "trace", fields(dex = %dex_type, token_in = %token_in, token_out = %token_out, amount_in = %amount_in_wei))]
pub async fn simulate_swap(
    app_state: Arc<AppState>,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    dex_type: DexType,
    token_in: Address,
    token_out: Address,
    amount_in_wei: U256,
    is_stable_route: Option<bool>,
    uni_pool_fee: Option<u32>,
    factory_addr: Option<Address>,
) -> Result<U256> {
    trace!("Simulating single swap...");
    match dex_type {
        DexType::UniswapV3 => {
            let quoter_address = app_state.config.quoter_v2_address;
            let quoter = QuoterV2::new(quoter_address, client);
            let fee = uni_pool_fee.ok_or_else(|| eyre!("Missing UniV3 pool fee for simulation"))?;
            let params = quoter_v2_bindings::QuoteExactInputSingleParams { token_in, token_out, amount_in: amount_in_wei, fee, sqrt_price_limit_x96: U256::zero(), };
            trace!(?params, "Calling QuoterV2 quoteExactInputSingle");
            let quote_result = quoter.quote_exact_input_single(params).call().await
                .wrap_err_with(|| format!("QuoterV2 simulation failed for pair {token_in:?} -> {token_out:?}"))?;
            debug!(amount_out = %quote_result.0, "QuoterV2 simulation successful");
            Ok(quote_result.0)
        }
        DexType::VelodromeV2 | DexType::Aerodrome => {
            let mut router_address_to_use = if dex_type == DexType::VelodromeV2 {
                app_state.config.velo_router_addr
            } else {
                app_state.config.aerodrome_router_addr.ok_or_else(|| eyre!("Aerodrome router address missing for simulation"))?
            };
            let mut attempted_impl_call = false;

            #[cfg(feature = "local_simulation")]
            {
                if dex_type == DexType::VelodromeV2 {
                    warn!("LOCAL SIMULATION: Attempting VelodromeV2 simulate_swap with IMPL address ({}) due to Anvil proxy issues.", VELO_ROUTER_IMPL_ADDR_FOR_SIM);
                    router_address_to_use = Address::from_str(VELO_ROUTER_IMPL_ADDR_FOR_SIM)?;
                    attempted_impl_call = true;
                }
            }

            let factory_address_for_call = factory_addr.ok_or_else(|| eyre!("Missing Factory address for Velo/Aero route simulation"))?;
            let stable_for_call = is_stable_route.ok_or_else(|| eyre!("Missing stability flag for Velo/Aero simulation"))?;

            let router = VelodromeRouter::new(router_address_to_use, client);
            let routes = vec![velo_router_bindings::Route {
                from: token_in,
                to: token_out,
                stable: stable_for_call,
                factory: factory_address_for_call,
            }];
            trace!(?routes, amount_in = %amount_in_wei, "Calling VelodromeRouter ({}) getAmountsOut", router_address_to_use);

            match router.get_amounts_out(amount_in_wei, routes.clone()).call().await {
                Ok(amounts) if amounts.len() >= 2 => {
                    debug!(amounts_out = ?amounts, "Velo/Aero getAmountsOut simulation successful on address {}", router_address_to_use);
                    Ok(amounts[1])
                }
                Ok(amounts) => Err(eyre!("Invalid amounts array length returned from getAmountsOut: {}", amounts.len())),
                Err(e) => {
                    #[cfg(feature = "local_simulation")]
                    if attempted_impl_call {
                        let selector_bytes = hex::decode(PAIR_DOES_NOT_EXIST_SELECTOR_STR)?;
                        let pair_does_not_exist_selector: Selector = selector_bytes.try_into()
                            .map_err(|_| eyre!("Failed to convert decoded hex to Selector bytes"))?;

                        if let ContractError::Revert(data) = &e {
                            if data.0.starts_with(&pair_does_not_exist_selector) {
                                warn!("LOCAL SIMULATION FALLBACK: Velodrome IMPL call reverted with PairDoesNotExist. Estimating output.");
                            } else {
                                warn!("LOCAL SIMULATION FALLBACK: Velodrome IMPL call reverted (data: {:?}). Estimating output.", data.0);
                            }
                        } else if e.to_string().contains("failed to decode empty bytes") {
                             warn!("LOCAL SIMULATION FALLBACK: Velodrome IMPL call failed to decode (empty bytes). Estimating output.");
                        } else {
                            return Err(eyre!(e).wrap_err(format!("Velo/Aero getAmountsOut RPC call FAILED UNEXPECTEDLY on IMPL address {}, factory {}, stable {}", router_address_to_use, factory_address_for_call, stable_for_call)));
                        }

                        let estimated_out = if stable_for_call {
                             if token_in == app_state.usdc_address && token_out == app_state.weth_address { amount_in_wei / 2000u64 }
                             else if token_in == app_state.weth_address && token_out == app_state.usdc_address { amount_in_wei * 2000u64 }
                             else { amount_in_wei }
                        } else {
                             if token_in == app_state.weth_address && token_out == app_state.usdc_address { amount_in_wei * 2000u64 }
                             else if token_in == app_state.usdc_address && token_out == app_state.weth_address { amount_in_wei / 2000u64 }
                             else { amount_in_wei }
                        };
                        let simulated_out = estimated_out * 999u64 / 1000u64;
                        warn!(amount_in = %amount_in_wei, simulated_out = %simulated_out, "Using ESTIMATED output for local sim due to IMPL call revert/failure.");
                        return Ok(simulated_out);
                    }
                    Err(eyre!(e).wrap_err(format!("Velo/Aero getAmountsOut RPC call failed for router {}, factory {}, stable {}", router_address_to_use, factory_address_for_call, stable_for_call)))
                }
            }
        }
        DexType::Unknown => Err(eyre!("Cannot simulate swap for Unknown DEX type")),
    }
}


/// Calculates the estimated net profit for a given route and loan amount.
#[allow(clippy::too_many_arguments)]
#[instrument(skip(app_state, client, route), level = "debug", fields( loan_amount_wei = %amount_in_wei ))]
pub async fn calculate_net_profit(
    app_state: Arc<AppState>,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    route: &RouteCandidate,
    amount_in_wei: U256,
    gas_price_gwei: f64,
    gas_limit_buffer_percentage: u64,
    min_flashloan_gas_limit: u64,
) -> Result<I256> {
    // ... (function body unchanged) ...
    let config = &app_state.config;
    let loan_token = route.token_in; let intermediate_token = route.token_out;
    trace!("Calculating net profit for route: {:?} -> {:?}", route.buy_dex_type, route.sell_dex_type);
    let amount_out_intermediate = match simulate_swap( app_state.clone(), client.clone(), route.buy_dex_type, loan_token, intermediate_token, amount_in_wei, route.buy_pool_stable, route.buy_pool_fee, Some(route.buy_pool_factory), ).await { Ok(amount) => amount, Err(e) => { warn!(error=?e, "Swap A simulation failed, assuming unprofitable."); return Ok(I256::min_value()); } };
    if amount_out_intermediate.is_zero() { debug!("Swap A simulation returned zero output. Route unprofitable."); return Ok(I256::min_value()); }
    trace!(amount_out_intermediate = %amount_out_intermediate, "Swap A simulation successful.");
    let final_amount_out_loan_token = match simulate_swap( app_state.clone(), client.clone(), route.sell_dex_type, intermediate_token, loan_token, amount_out_intermediate, route.sell_pool_stable, route.sell_pool_fee, Some(route.sell_pool_factory), ).await { Ok(amount) => amount, Err(e) => { warn!(error=?e, "Swap B simulation failed, assuming unprofitable."); return Ok(I256::min_value()); } };
    trace!(final_amount_out_loan_token = %final_amount_out_loan_token, "Swap B simulation successful.");
    let gross_profit_wei = I256::from_raw(final_amount_out_loan_token) - I256::from_raw(amount_in_wei);
    debug!(gross_profit_wei = %gross_profit_wei, "Gross profit calculated.");
    if gross_profit_wei <= I256::zero() { return Ok(gross_profit_wei); }
    trace!("Estimating gas cost for net profit calculation...");
    let gas_price_wei_str = format!("{:.18}", gas_price_gwei); let gas_price_wei: U256 = parse_units(&gas_price_wei_str, "gwei")?.into();
    trace!(gas_price_gwei=%gas_price_gwei, gas_price_wei=%gas_price_wei, "Converted gas price");
    let effective_router_addr = if route.buy_dex_type.is_velo_style() || route.sell_dex_type.is_velo_style() { if route.buy_dex_type == DexType::Aerodrome || route.sell_dex_type == DexType::Aerodrome { config.aerodrome_router_addr.ok_or_else(|| eyre!("Aero router needed for encoding"))? } else { config.velo_router_addr } } else { config.velo_router_addr };
    use crate::encoding::encode_user_data;
    let user_data_for_gas_est = encode_user_data( route.buy_pool_addr, route.sell_pool_addr, intermediate_token, route.zero_for_one_a, route.buy_dex_type.is_velo_style(), route.sell_dex_type.is_velo_style(), effective_router_addr, U256::zero(), U256::zero(), )?;
    trace!("User data for gas estimate encoded.");
    let gas_est_timeout = Duration::from_secs(10);
    let gas_estimate_result = timeout(
        gas_est_timeout,
        estimate_flash_loan_gas(
            client.clone(),
            config.balancer_vault_address,
            config.arb_executor_address.ok_or_else(|| eyre!("Executor address missing for gas estimate"))?,
            loan_token,
            amount_in_wei,
            user_data_for_gas_est,
        )
    ).await;
    let gas_estimate_units = match gas_estimate_result {
        Ok(Ok(est)) => est,
        Ok(Err(e)) => {
            warn!(error=?e, "Gas estimation failed within net profit calc, assuming high cost.");
            return Ok(I256::min_value());
        }
        Err(_) => {
            warn!(timeout_secs = gas_est_timeout.as_secs(), "Gas estimation timed out within net profit calc, assuming high cost.");
            return Ok(I256::min_value());
        }
    };
    trace!(gas_estimate_units = %gas_estimate_units, "Initial gas estimate received.");
    let buffered_gas_limit = gas_estimate_units * (100 + gas_limit_buffer_percentage) / 100; let final_gas_limit = std::cmp::max(buffered_gas_limit, U256::from(min_flashloan_gas_limit));
    trace!(buffered_gas_limit = %buffered_gas_limit, min_flashloan_gas_limit = %min_flashloan_gas_limit, final_gas_limit = %final_gas_limit, "Calculated final gas limit");
    let gas_cost_wei = gas_price_wei * final_gas_limit;
    trace!(gas_cost_wei = %gas_cost_wei, "Total gas cost calculated.");
    let net_profit_wei = gross_profit_wei - I256::from_raw(gas_cost_wei);
    debug!(net_profit_wei = %net_profit_wei, "Net profit calculated.");
    Ok(net_profit_wei)
}


/// Searches for the optimal flash loan amount for a given route candidate.
// (Function remains unchanged)
#[allow(clippy::too_many_arguments)]
#[instrument(skip_all, level = "info", fields( route = ?route ))]
pub async fn find_optimal_loan_amount(
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    app_state: Arc<AppState>,
    route: &RouteCandidate,
    buy_pool_snapshot: Option<&PoolSnapshot>,
    sell_pool_snapshot: Option<&PoolSnapshot>,
    gas_price_gwei: f64,
) -> Result<Option<(U256, I256)>> {
    info!("Searching optimal loan amount...");
    let config = &app_state.config; let mut best_loan_amount_wei = U256::zero(); let mut max_net_profit_wei = I256::min_value();
    let min_loan_weth = config.min_loan_amount_weth; let config_max_loan_weth = config.max_loan_amount_weth;
    let min_loan_wei = f64_to_wei(min_loan_weth, config.weth_decimals as u32)?; let config_max_loan_wei = f64_to_wei(config_max_loan_weth, config.weth_decimals as u32)?;
    let dynamic_max_loan_wei = calculate_dynamic_max_loan( config_max_loan_wei, buy_pool_snapshot, sell_pool_snapshot, route.token_in, config, );
    let dynamic_max_loan_weth = dynamic_max_loan_wei.to_f64_lossy() / 10f64.powi(config.weth_decimals as i32);
    info!( config_max_weth = config_max_loan_weth, dynamic_max_weth = format!("{:.4}", dynamic_max_loan_weth), "Max loan amount limits (WETH)" );
    let effective_max_loan_wei = std::cmp::min(config_max_loan_wei, dynamic_max_loan_wei); let effective_max_loan_weth = dynamic_max_loan_weth.min(config_max_loan_weth);
    let search_min_weth = min_loan_weth; let search_max_weth = effective_max_loan_weth; let iterations = config.optimal_loan_search_iterations;
    if min_loan_wei >= effective_max_loan_wei || iterations < 1 || search_min_weth <= 0.0 || search_max_weth <= search_min_weth { warn!( min_weth = search_min_weth, eff_max_weth = search_max_weth, iterations, "Invalid or zero-width search range for optimal loan. Skipping search." ); return Ok(None); }
    info!( search_range_weth = format!("{:.4} - {:.4}", search_min_weth, search_max_weth), iterations, "Starting optimal loan search..." );
    let mut simulation_tasks = vec![];
    for i in 0..iterations {
        let ratio = if iterations <= 1 { 0.5 } else { i as f64 / (iterations - 1) as f64 }; let current_loan_amount_weth = search_min_weth + (search_max_weth - search_min_weth) * ratio;
        let current_loan_amount_wei = match f64_to_wei(current_loan_amount_weth, config.weth_decimals as u32) { Ok(amount) => amount, Err(e) => { warn!(amount_f64=%current_loan_amount_weth, error=?e, "Failed f64_to_wei conversion, skipping amount"); continue; } };
        if current_loan_amount_wei < min_loan_wei || current_loan_amount_wei > effective_max_loan_wei || current_loan_amount_wei.is_zero() { trace!(%current_loan_amount_wei, "Skipping amount outside effective range."); continue; }
        let task_client = client.clone(); let task_app_state = app_state.clone(); let task_route = route.clone(); let task_gas_limit_buffer = config.gas_limit_buffer_percentage; let task_min_gas_limit = config.min_flashloan_gas_limit;
        simulation_tasks.push(tokio::spawn(async move { let profit_result = calculate_net_profit( task_app_state, task_client, &task_route, current_loan_amount_wei, gas_price_gwei, task_gas_limit_buffer, task_min_gas_limit, ).await; (current_loan_amount_wei, profit_result) }));
    }
    let results = futures_util::future::join_all(simulation_tasks).await; debug!("Collected {} simulation results.", results.len());
    for join_result in results { match join_result { Ok((amount_wei, Ok(profit_wei))) => { trace!(loan_amount_wei=%amount_wei, net_profit_wei=%profit_wei, "Profit calculated for amount."); if profit_wei > max_net_profit_wei { max_net_profit_wei = profit_wei; best_loan_amount_wei = amount_wei; } } Ok((amount_wei, Err(e))) => { warn!(loan_amount_wei=%amount_wei, error=?e, "Error calculating profit for specific loan amount"); } Err(e) => { error!(error=?e, "Simulation task failed"); } } }
    if max_net_profit_wei > I256::zero() {
        let best_loan_weth_str = format_units(best_loan_amount_wei, config.weth_decimals as i32)?;
        let profit_weth_str = format_units(max_net_profit_wei.into_raw(), config.weth_decimals as i32)?;
        info!( optimal_loan_weth = %best_loan_weth_str, max_net_profit_weth = %profit_weth_str, "🎉 Optimal loan amount found!" );
        Ok(Some((best_loan_amount_wei, max_net_profit_wei)))
    } else {
        info!("No profitable loan amount found within the search range.");
        #[cfg(feature = "local_simulation")]
        {
            warn!("LOCAL SIMULATION: No real profit found. Forcing a small positive profit to test submission flow.");
            let test_loan_amount = if best_loan_amount_wei > U256::zero() { best_loan_amount_wei } else { min_loan_wei };
            let fake_profit_wei = I256::from(10000);
             info!(forced_loan_wei = %test_loan_amount, forced_profit_wei = %fake_profit_wei, "Injecting fake profit for local test.");
            Ok(Some((test_loan_amount, fake_profit_wei)))
        }
        #[cfg(not(feature = "local_simulation"))]
        {
             Ok(None)
        }
    }
}

/// Calculates a dynamic maximum loan amount based primarily on V2/Aero pool reserves.
// (Function remains unchanged)
#[instrument(level="debug", skip(buy_pool_snapshot))]
fn calculate_dynamic_max_loan(
    config_max_loan_wei: U256,
    buy_pool_snapshot: Option<&PoolSnapshot>,
    _sell_pool_snapshot: Option<&PoolSnapshot>, // Mark unused
    loan_token: Address,
    config: &Config,
) -> U256 {
    trace!("Calculating dynamic max loan based on pool depth...");
    let mut dynamic_max_wei = config_max_loan_wei;
    if let Some(buy_snap) = buy_pool_snapshot {
        match buy_snap.dex_type {
            DexType::VelodromeV2 | DexType::Aerodrome => {
                let reserve_option = if buy_snap.token0 == loan_token { buy_snap.reserve0 } else if buy_snap.token1 == loan_token { buy_snap.reserve1 } else { None };
                if let Some(reserve) = reserve_option {
                    if !reserve.is_zero() { let limit_wei = reserve * U256::from(V2_RESERVE_PERCENTAGE_LIMIT) / U256::from(100); dynamic_max_wei = std::cmp::min(dynamic_max_wei, limit_wei); trace!( pool = %buy_snap.pool_address, dex = %buy_snap.dex_type.to_string(), reserve = %reserve, limit_pct = V2_RESERVE_PERCENTAGE_LIMIT, limit_wei = %limit_wei, "Applied V2/Aero depth limit based on loan token reserve." ); }
                    else { warn!(pool=%buy_snap.pool_address, dex=%buy_snap.dex_type.to_string(), "Loan token reserve is zero, cannot borrow."); dynamic_max_wei = U256::zero(); }
                } else { error!(pool=%buy_snap.pool_address, %loan_token, token0=%buy_snap.token0, token1=%buy_snap.token1, "Loan token not found in V2 pool snapshot reserves!"); dynamic_max_wei = U256::zero(); }
            }
            DexType::UniswapV3 => {
                if config.enable_univ3_dynamic_sizing { warn!(pool = %buy_snap.pool_address, "UniV3 dynamic loan sizing ENABLED but NOT IMPLEMENTED. Accurate sizing requires tick liquidity analysis. Using configured max loan as the upper bound for now."); trace!("TODO: Implement UniV3 tick liquidity analysis for dynamic sizing."); }
                else { trace!(pool = %buy_snap.pool_address, "UniV3 dynamic sizing disabled by config. Using configured max loan as upper bound."); }
            }
            DexType::Unknown => { warn!(pool = %buy_snap.pool_address, "Cannot apply dynamic sizing for Unknown DEX type."); }
        }
    } else { warn!("Buy pool snapshot missing, cannot apply dynamic sizing based on pool depth. Using configured max loan."); }
    let final_dynamic_max_wei = std::cmp::min(dynamic_max_wei, config_max_loan_wei);
    if final_dynamic_max_wei < config_max_loan_wei { debug!(dynamic_max_wei = %final_dynamic_max_wei, "Dynamic depth limit applied."); }
    else { trace!(dynamic_max_wei = %final_dynamic_max_wei, "Using config max loan (or less) as effective limit."); }
    final_dynamic_max_wei
}
