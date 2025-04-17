// src/simulation.rs

// --- Imports ---
use ethers::{
    prelude::{Middleware, Provider, Http, SignerMiddleware, LocalWallet}, // Added specific types
    utils::format_units, // Added for logging profit/loss if needed
    types::{Address, U256, I256, Bytes}, // Keep needed types
};
use eyre::Result;
use std::sync::Arc; // Keep Arc for client type

// --- Use statements for types from the shared bindings module ---
use crate::bindings::{
    VelodromeRouter,
    QuoterV2,
    quoter_v2 as quoter_v2_bindings,
    velodrome_router as velo_router_bindings,
    // BalancerVault needed by gas estimator
};
// --- Use statements for other modules ---
use crate::gas::estimate_flash_loan_gas;
use crate::encoding::encode_user_data;

// --- simulate_swap function definition ---
// Make this generic as well to accept instances created with SignerMiddleware
pub async fn simulate_swap<M: Middleware>( // Still generic here is fine
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

/// Calculates the estimated net profit (or loss) for a given arbitrage attempt amount.
pub async fn calculate_net_profit(
    // Expect the concrete client type needed by estimate_flash_loan_gas
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
    // Pass concrete contract instances tied to the client
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
    flash_loan_fee_rate: f64, // Use the constant name from main scope
 ) -> Result<I256> {

    // 1. Simulate Swaps (Pass concrete instances)
    // Need to pass the client itself now, as simulate_swap expects Middleware M
    // Create temp instances if necessary, or make simulate_swap take specific types?
    // Let's adjust simulate_swap signature to be specific as well for consistency here.
    // Reverting: Keep simulate_swap generic. We call it with instances created
    // via the concrete `client` type, so M = SignerMiddleware<...>

    let amount_out_intermediate_wei = simulate_swap(
        buy_dex, token_in, token_out, amount_in_wei,
        velo_router, uni_quoter, buy_dex_stable, buy_dex_fee,
    ).await?;
    if amount_out_intermediate_wei.is_zero() {
        println!("      WARN: Swap 1 simulation yielded zero output for amount {}", amount_in_wei);
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

    // Call gas estimator, passing the concrete client type
    let estimated_gas_units = estimate_flash_loan_gas(
        client.clone(), // Pass the concrete client Arc
        balancer_vault_address,
        arb_executor_address,
        token_in,
        amount_in_wei,
        user_data,
    ).await?;

    let gas_cost_wei = gas_price * estimated_gas_units;

    // 4. Calculate Flash Loan Fee (Use passed-in rate)
    let fee_numerator = U256::from((flash_loan_fee_rate * 10000.0) as u128);
    let fee_denominator = U256::from(10000);
    let flash_loan_fee_wei = amount_in_wei * fee_numerator / fee_denominator;

    // 5. Calculate Total Cost
    let total_cost_wei = gas_cost_wei + flash_loan_fee_wei;

    // 6. Calculate Net Profit
    let net_profit_wei = gross_profit_wei - I256::from_raw(total_cost_wei);

    Ok(net_profit_wei)
}