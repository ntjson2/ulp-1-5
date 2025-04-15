// src/main.rs

// --- Imports ---
use ethers::{
    prelude::*, // Import common ethers items
    types::{Address, U256}, // Specific types
    utils::{format_units}, // Formatting helper
};
use eyre::Result;
use std::{env, sync::Arc, str::FromStr}; // Standard library items

// --- Module Declarations ---
mod config;     // Loads configuration from .env
mod utils;      // Contains helper functions (price calcs, conversions)
mod simulation; // Contains swap simulation logic
mod bindings;   // Contains all contract bindings (abigen!)

// --- Use Statements ---
use crate::config::load_config; // Function to load config
use crate::utils::*;            // Utility functions and traits
use crate::simulation::simulate_swap; // Swap simulation function
// Import contract types from the bindings module
use crate::bindings::{
    UniswapV3Pool,
    VelodromeV2Pool,
    VelodromeRouter,
    BalancerVault,
    QuoterV2,
    // IERC20 is also available via bindings::i_erc20 if needed directly
};

// --- Constants ---
const ARBITRAGE_THRESHOLD_PERCENTAGE: f64 = 0.1;
const FLASH_LOAN_FEE_RATE: f64 = 0.0000;
const SIMULATION_AMOUNT_WETH: f64 = 1.0;

// --- Main Execution ---
#[tokio::main]
async fn main() -> Result<()> {
    // Load config using the function from the config module
    let config = load_config()?;

    // --- Setup Provider & Client ---
    println!("Setting up provider & client...");
    let provider = Provider::<Http>::try_from(config.local_rpc_url.clone())?;
    let provider = Arc::new(provider);
    let chain_id = provider.get_chainid().await?.as_u64();
    println!("RPC OK. Chain ID: {}", chain_id);
    let wallet = config.local_private_key.parse::<LocalWallet>()?.with_chain_id(chain_id);
    let client = SignerMiddleware::new(provider.clone(), wallet.clone());
    let client = Arc::new(client);
    println!("Provider & client setup complete.");

    // --- Use Addresses from Config ---
    println!("Using addresses from config.");
    let uni_v3_pool_address = config.uni_v3_pool_addr;
    let velo_v2_pool_address = config.velo_v2_pool_addr;
    let weth_address = config.weth_address;
    let usdc_address = config.usdc_address;
    let arb_executor_address = config.arb_executor_address;
    let velo_router_address = config.velo_router_addr;
    let balancer_vault_address = config.balancer_vault_address;
    let quoter_v2_address = config.quoter_v2_address;
    let weth_decimals = config.weth_decimals;
    let usdc_decimals = config.usdc_decimals;

    // --- Create Contract Instances ---
    // Use the types imported from the bindings module
    println!("Creating contract instances...");
    let uni_v3_pool = UniswapV3Pool::new(uni_v3_pool_address, provider.clone());
    let velo_v2_pool = VelodromeV2Pool::new(velo_v2_pool_address, provider.clone());
    let velo_router = VelodromeRouter::new(velo_router_address, provider.clone());
    let balancer_vault = BalancerVault::new(balancer_vault_address, client.clone()); // Keep unused for now
    let quoter = QuoterV2::new(quoter_v2_address, provider.clone());
    println!("Contract instances created.");

    // --- Determine Pool/Token Details ---
    println!("Fetching pool details...");
    let velo_token0 = velo_v2_pool.token_0().call().await?;
    let velo_token1 = velo_v2_pool.token_1().call().await?;
    let velo_is_stable = velo_v2_pool.stable().call().await?;
    println!("  Velo Pool Stable: {}", velo_is_stable);
    let (velo_decimals0, velo_decimals1, velo_t0_is_weth) = if velo_token0 == weth_address && velo_token1 == usdc_address {
        (weth_decimals, usdc_decimals, true)
    } else if velo_token0 == usdc_address && velo_token1 == weth_address {
        (usdc_decimals, weth_decimals, false)
    } else {
        eyre::bail!("Velo pool tokens ({:?}, {:?}) do not match WETH/USDC addresses in .env", velo_token0, velo_token1);
    };

    let uni_token0 = uni_v3_pool.token_0().call().await?;
    let uni_token1 = uni_v3_pool.token_1().call().await?;
    let uni_fee = uni_v3_pool.fee().call().await?;
    println!("  Uni Pool Fee: {}", uni_fee);
     if !(uni_token0 == weth_address && uni_token1 == usdc_address) && !(uni_token0 == usdc_address && uni_token1 == weth_address) {
         eyre::bail!("Uni pool tokens ({:?}, {:?}) do not match WETH/USDC addresses in .env", uni_token0, uni_token1);
     }
    let uni_decimals0 = weth_decimals;
    let uni_decimals1 = usdc_decimals;
    println!("Pool details fetched.");


    println!("\n--- Performing Single Test Run ---");

    let simulation_amount_weth_wei = f64_to_wei(SIMULATION_AMOUNT_WETH, weth_decimals as u32)?;
    println!("Simulating with {} WETH ({})", SIMULATION_AMOUNT_WETH, simulation_amount_weth_wei);


    // --- Fetch Prices (for initial detection) ---
    println!("Fetching prices...");
    let uni_price_result: Result<f64> = async {
        uni_v3_pool.slot_0().call().await
            .map_err(|e| eyre::eyre!("RPC Error fetching UniV3 slot0: {}", e))
            .and_then(|slot0_data| {
                let price_native = v3_price_from_sqrt(slot0_data.0, uni_decimals0, uni_decimals1)?;
                 if uni_token0 == weth_address { Ok(price_native) } else { if price_native.abs() < f64::EPSILON {Ok(0.0)} else {Ok(1.0 / price_native)} }
            })
    }.await;

    let velo_price_result: Result<f64> = async {
         velo_v2_pool.get_reserves().call().await
            .map_err(|e| eyre::eyre!("RPC Error fetching Velo reserves: {}", e))
            .and_then(|reserves| {
                let price = v2_price_from_reserves(reserves.0.into(), reserves.1.into(), velo_decimals0, velo_decimals1)?;
                Ok(if velo_t0_is_weth { price } else { if price.abs() < f64::EPSILON { 0.0 } else { 1.0 / price } })
         })
    }.await;
     println!("Prices fetched.");

    // --- Arbitrage Detection & Simulation ---
        match (uni_price_result, velo_price_result) {
            (Ok(p_uni), Ok(p_velo)) => {
                 println!("UniV3 Price (WETH/USDC): {:.6}", p_uni);
                 println!("VeloV2 Price (WETH/USDC): {:.6}", p_velo);
                 let price_diff = (p_uni - p_velo).abs();
                 let base_price = p_uni.min(p_velo);
                 let spread_percentage = if base_price > 1e-18 { (price_diff / base_price) * 100.0 } else { 0.0 };
                 println!("-> Spread (Abs): {:.4}%", spread_percentage);

                 if spread_percentage > ARBITRAGE_THRESHOLD_PERCENTAGE {
                    println!(">>> Arbitrage Opportunity DETECTED! Spread: {:.4}%", spread_percentage);

                    let amount_in_wei = simulation_amount_weth_wei;
                    let token_in = weth_address;
                    let token_out = usdc_address;

                    let (buy_dex, sell_dex, buy_dex_stable, sell_dex_stable, buy_dex_fee, sell_dex_fee) = if p_uni < p_velo {
                        ("UniV3", "VeloV2", false, velo_is_stable, uni_fee, 0u32)
                    } else {
                        ("VeloV2", "UniV3", velo_is_stable, false, 0u32, uni_fee)
                    };
                    println!("    Direction: Buy {} (Low), Sell {} (High)", buy_dex, sell_dex);

                    // --- Accurate Simulation ---
                    // Calls the simulate_swap function from the simulation module
                    let simulation_result: Result<U256> = async {

                        let amount_out_intermediate_wei = simulate_swap(
                            buy_dex,
                            token_in,
                            token_out,
                            amount_in_wei,
                            &velo_router, // Pass instance created in main
                            &quoter,      // Pass instance created in main
                            buy_dex_stable,
                            buy_dex_fee,
                        ).await?;
                        // Intermediate logging is now inside simulate_swap

                        if amount_out_intermediate_wei.is_zero() {
                            eyre::bail!("Simulation Swap 1 resulted in zero output.");
                        }

                        let amount_out_final_wei = simulate_swap(
                            sell_dex,
                            token_out,
                            token_in,
                            amount_out_intermediate_wei,
                            &velo_router, // Pass instance created in main
                            &quoter,      // Pass instance created in main
                            sell_dex_stable,
                            sell_dex_fee,
                         ).await?;
                        // Final logging is now inside simulate_swap

                        Ok(amount_out_final_wei)
                    }.await;


                    match simulation_result {
                        Ok(final_amount) => {
                            let gross_profit_wei = final_amount.saturating_sub(amount_in_wei);
                            println!("    Simulated Gross Profit (WETH): {}", format_units(gross_profit_wei, "ether")?);

                            // Estimate Gas Cost (Placeholder)
                            let gas_price = provider.get_gas_price().await?;
                            let estimated_gas_units = U256::from(500_000);
                            let gas_cost_wei = gas_price * estimated_gas_units;
                            println!("    Estimated Gas Cost (Placeholder): {} Wei ({:.8} ETH)", gas_cost_wei, format_units(gas_cost_wei, "ether")?);

                            // Calculate Flash Loan Fee
                            let fee_numerator = U256::from((FLASH_LOAN_FEE_RATE * 10000.0) as u128);
                            let fee_denominator = U256::from(10000);
                            let flash_loan_fee_wei = amount_in_wei * fee_numerator / fee_denominator;
                             println!("    Estimated Flash Loan Fee: {} Wei ({:.8} WETH)", flash_loan_fee_wei, format_units(flash_loan_fee_wei, "ether")?);

                            let total_cost_wei = gas_cost_wei + flash_loan_fee_wei;

                            // --- Decision ---
                            if final_amount > amount_in_wei && gross_profit_wei > total_cost_wei {
                                let net_profit_weth_wei = gross_profit_wei.saturating_sub(total_cost_wei);
                                println!("    Simulated NET Profit: {} Wei ({:.8} WETH)", net_profit_weth_wei, format_units(net_profit_weth_wei, "ether")?);
                                println!("    >>> Simulation SUCCESSFUL - Profit Expected <<<");
                                // TODO: Encode userData
                                // TODO: Estimate Gas Accurately (NEXT STEP!)
                                // TODO: Send flash loan tx
                            } else {
                                println!("    Simulated NET Loss/Insufficient Profit: Gross Profit {} <= Total Cost {}", gross_profit_wei, total_cost_wei);
                                println!("    >>> Simulation FAILED - Aborting Execution <<<");
                            }
                        },
                        Err(sim_err) => {
                            eprintln!("! Simulation Error: {}", sim_err);
                             println!("    >>> Simulation FAILED - Aborting Execution <<<");
                        }
                    } // End match simulation_result


                 } // End if spread > threshold
            }, // End Ok match prices
            (Err(e), _) => eprintln!("! Error Processing UniV3 Price: {}", e),
            (_, Err(e)) => eprintln!("! Error Processing VeloV2 Price: {}", e),
        } // End match prices

    println!("\n--- Run Complete ---");
    Ok(())
} // End main