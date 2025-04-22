// src/simulation.rs

// --- Imports ---
use ethers::{
    prelude::{Middleware, Provider, Http, SignerMiddleware, LocalWallet},
    utils::format_units,
    types::{Address, U256, I256},
};
use eyre::Result;
use std::sync::Arc;
// Remove unused ToF64Lossy, keep f64_to_wei
use crate::utils::f64_to_wei;

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
pub async fn simulate_swap<M: Middleware>(
    dex_type: &str,
    token_in: Address,
    token_out: Address,
    amount_in: U256,
    velo_router: &VelodromeRouter<M>,
    quoter: &QuoterV2<M>,
    is_velo_route_stable: bool,
    uni_pool_fee: u32,
) -> Result<U256> where M::Error: 'static + Send + Sync {
     println!(
        "    Simulating Swap: {} -> {} Amount: {} on {}",
        token_in, token_out, amount_in, dex_type
    );
    match dex_type {
        "UniV3" => {
            let params = quoter_v2_bindings::QuoteExactInputSingleParams {
                token_in, token_out, amount_in, fee: uni_pool_fee,
                sqrt_price_limit_x96: U256::zero(),
            };
            let quote_result = quoter.quote_exact_input_single(params).call().await;
            match quote_result {
                Ok(output) => Ok(output.0),
                Err(e) => Err(eyre::eyre!("UniV3 Quoter simulation failed: {:?}", e)),
            }
        }
        "VeloV2" => {
            let routes = vec![velo_router_bindings::Route {
                 from: token_in, to: token_out, stable: is_velo_route_stable,
                 factory: Address::zero(),
             }];
            match velo_router.get_amounts_out(amount_in, routes).call().await {
                Ok(amounts_out) => {
                     if amounts_out.len() >= 2 { Ok(amounts_out[1]) }
                     else { Err(eyre::eyre!("VeloV2 getAmountsOut returned unexpected vector length")) }
                },
                Err(e) => Err(eyre::eyre!("VeloV2 simulation failed: {:?}", e)),
            }
        }
        _ => Err(eyre::eyre!("Unsupported DEX type for simulation: {}", dex_type)),
    }
}

// --- Profit Calculation Helper ---
#[allow(clippy::too_many_arguments)]
pub async fn calculate_net_profit(
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    amount_in_wei: U256,
    token_in: Address,
    token_out: Address,
    buy_dex: &str,
    sell_dex: &str,
    buy_dex_stable: bool,
    sell_dex_stable: bool,
    buy_dex_fee: u32,
    sell_dex_fee: u32,
    velo_router: &VelodromeRouter<SignerMiddleware<Provider<Http>, LocalWallet>>,
    uni_quoter: &QuoterV2<SignerMiddleware<Provider<Http>, LocalWallet>>,
    arb_executor_address: Address,
    balancer_vault_address: Address,
    pool_a_addr: Address,
    pool_b_addr: Address,
    zero_for_one_a: bool,
    is_a_velo: bool,
    is_b_velo: bool,
    velo_router_address: Address,
    flash_loan_fee_rate: f64,
 ) -> Result<I256> {

    // 1. Simulate Swaps
    let amount_out_intermediate_wei = simulate_swap(
        buy_dex, token_in, token_out, amount_in_wei,
        velo_router, uni_quoter, buy_dex_stable, buy_dex_fee,
    ).await?;
    if amount_out_intermediate_wei.is_zero() {
        return Ok(I256::min_value());
    }
    let final_amount = simulate_swap(
        sell_dex, token_out, token_in, amount_out_intermediate_wei,
        velo_router, uni_quoter, sell_dex_stable, sell_dex_fee,
    ).await?;

    // 2. Calculate Gross Profit
    let gross_profit_wei = I256::from_raw(final_amount) - I256::from_raw(amount_in_wei);

    // 3. Estimate Gas Cost
    let gas_price = client.get_gas_price().await?;
    let user_data = encode_user_data(
        pool_a_addr, pool_b_addr, token_out,
        zero_for_one_a, is_a_velo, is_b_velo, velo_router_address,
    )?;
    let estimated_gas_units = estimate_flash_loan_gas(
        client.clone(), balancer_vault_address, arb_executor_address,
        token_in, amount_in_wei, user_data,
    ).await?;
    let gas_cost_wei = gas_price * estimated_gas_units;

    // 4. Calculate Flash Loan Fee
    let fee_numerator = U256::from((flash_loan_fee_rate * 10000.0) as u128);
    let fee_denominator = U256::from(10000);
    if fee_denominator.is_zero() { return Err(eyre::eyre!("Fee denominator is zero")); }
    let flash_loan_fee_wei = amount_in_wei * fee_numerator / fee_denominator;

    // 5. Calculate Total Cost
    let total_cost_wei = gas_cost_wei + flash_loan_fee_wei;

    // 6. Calculate Net Profit
    let net_profit_wei = gross_profit_wei - I256::from_raw(total_cost_wei);

    Ok(net_profit_wei)
}


// --- Optimal Loan Amount Search Function ---
#[allow(clippy::too_many_arguments)]
pub async fn find_optimal_loan_amount(
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    min_loan_amount_weth: f64,
    max_loan_amount_weth: f64,
    iterations: u32,
    token_in: Address, token_out: Address, weth_decimals: u8, flash_loan_fee_rate: f64,
    buy_dex: &str, sell_dex: &str, buy_dex_stable: bool, sell_dex_stable: bool,
    buy_dex_fee: u32, sell_dex_fee: u32,
    velo_router: &VelodromeRouter<SignerMiddleware<Provider<Http>, LocalWallet>>,
    uni_quoter: &QuoterV2<SignerMiddleware<Provider<Http>, LocalWallet>>,
    arb_executor_address: Address, balancer_vault_address: Address, pool_a_addr: Address,
    pool_b_addr: Address, zero_for_one_a: bool, is_a_velo: bool, is_b_velo: bool,
    velo_router_address: Address,
) -> Result<Option<(U256, I256)>> {
    println!("      Searching for optimal loan amount (Min: {} WETH, Max: {} WETH, Iterations: {})...",
        min_loan_amount_weth, max_loan_amount_weth, iterations);

    let mut best_amount_wei = U256::zero();
    let mut max_profit_wei = I256::min_value();

    let min_loan_wei = f64_to_wei(min_loan_amount_weth, weth_decimals as u32)?;
    let max_loan_wei = f64_to_wei(max_loan_amount_weth, weth_decimals as u32)?;

    if min_loan_wei >= max_loan_wei || iterations < 1 {
        println!("      WARN: Invalid search range or iterations. Testing only Min amount.");
        let profit_at_min = calculate_net_profit(
            client.clone(), min_loan_wei, token_in, token_out, buy_dex, sell_dex,
            buy_dex_stable, sell_dex_stable, buy_dex_fee, sell_dex_fee,
            velo_router, uni_quoter, arb_executor_address, balancer_vault_address,
            pool_a_addr, pool_b_addr, zero_for_one_a, is_a_velo, is_b_velo,
            velo_router_address, flash_loan_fee_rate ).await?;
        return if profit_at_min > I256::zero() { Ok(Some((min_loan_wei, profit_at_min))) } else { Ok(None) };
    }

    // Iterative Sampling
    for i in 0..iterations {
        let ratio = if iterations <= 1 { 0.0 } else { i as f64 / (iterations - 1) as f64 };
        let sample_amount_f64 = min_loan_amount_weth + (max_loan_amount_weth - min_loan_amount_weth) * ratio;
        let current_amount_wei = f64_to_wei(sample_amount_f64, weth_decimals as u32)?;

        if current_amount_wei < min_loan_wei || current_amount_wei > max_loan_wei || current_amount_wei.is_zero() { continue; }

        let current_profit_result = calculate_net_profit(
            client.clone(), current_amount_wei, token_in, token_out, buy_dex, sell_dex,
            buy_dex_stable, sell_dex_stable, buy_dex_fee, sell_dex_fee, velo_router,
            uni_quoter, arb_executor_address, balancer_vault_address, pool_a_addr,
            pool_b_addr, zero_for_one_a, is_a_velo, is_b_velo, velo_router_address,
            flash_loan_fee_rate ).await;

        match current_profit_result {
            Ok(profit) => {
                if profit > max_profit_wei {
                    max_profit_wei = profit;
                    best_amount_wei = current_amount_wei;
                }
            }
            Err(e) => { eprintln!("        Error calculating profit for amount {}: {}", current_amount_wei, e); }
        }
    } // End loop

    if max_profit_wei > I256::zero() {
        println!("      Optimal Amount Search Complete. Best Amount: {} WETH ({}), Est. Max Profit: {} WETH",
            format_units(best_amount_wei, "ether").unwrap_or_default(),
            best_amount_wei,
            format_units(max_profit_wei.into_raw(), "ether").unwrap_or_default()
        );
        Ok(Some((best_amount_wei, max_profit_wei)))
    } else {
        println!("      Optimal Amount Search Complete. No profitable amount found in range.");
        Ok(None)
    }
}