// src/main.rs

// --- Imports ---
use ethers::{
    abi::{Token}, // Keep Token if used by encoding indirectly
    prelude::*,
    types::{Address, Bytes, Eip1559TransactionRequest, TransactionReceipt, U256},
    utils::{format_units},
};
use eyre::Result;
use std::{sync::Arc}; // Removed env, FromStr
use tokio::time::{interval, Duration}; // Added for interval polling

// --- Module Declarations ---
mod config;
mod utils;
mod simulation;
mod bindings;
mod encoding;

// --- Use Statements ---
use crate::config::load_config;
use crate::utils::*;
use crate::simulation::simulate_swap;
use crate::bindings::{
    UniswapV3Pool,
    VelodromeV2Pool,
    VelodromeRouter,
    BalancerVault,
    QuoterV2,
};
use crate::encoding::encode_user_data;

// --- Constants ---
const ARBITRAGE_THRESHOLD_PERCENTAGE: f64 = 0.00001; // Keep low threshold for testing execution path
const FLASH_LOAN_FEE_RATE: f64 = 0.0000;
const SIMULATION_AMOUNT_WETH: f64 = 1.0;
const POLLING_INTERVAL_SECONDS: u64 = 5; // Poll every 5 seconds (adjust as needed)

// --- Main Execution ---
#[tokio::main]
async fn main() -> Result<()> {
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
    println!("Creating contract instances...");
    let uni_v3_pool = UniswapV3Pool::new(uni_v3_pool_address, provider.clone());
    let velo_v2_pool = VelodromeV2Pool::new(velo_v2_pool_address, provider.clone());
    let velo_router = VelodromeRouter::new(velo_router_address, provider.clone());
    let balancer_vault = BalancerVault::new(balancer_vault_address, client.clone());
    let uni_quoter = QuoterV2::new(quoter_v2_address, provider.clone()); // Renamed variable used
    println!("Contract instances created.");

    // --- Determine Pool/Token Details (Fetch once initially) ---
    // It might be better to refresh these periodically too, but for now, fetch once.
    println!("Fetching initial pool details...");
    let velo_token0 = velo_v2_pool.token_0().call().await?;
    let velo_token1 = velo_v2_pool.token_1().call().await?;
    let velo_is_stable = velo_v2_pool.stable().call().await?;
    println!("  Velo Pool Stable: {}", velo_is_stable);
    let (_velo_decimals0, _velo_decimals1, velo_t0_is_weth) = if velo_token0 == weth_address && velo_token1 == usdc_address {
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
    println!("Initial pool details fetched.");

    // --- Initialize Polling Timer ---
    let mut poll_interval = interval(Duration::from_secs(POLLING_INTERVAL_SECONDS));
    println!("\n--- Starting Continuous Polling (Interval: {}s) ---", POLLING_INTERVAL_SECONDS);

    // --- Main Polling Loop ---
    loop {
        poll_interval.tick().await; // Wait for the next interval tick
        println!("\n==== Polling Cycle Start ====");

        // Use already created instances and config values within the loop
        let simulation_amount_weth_wei = f64_to_wei(SIMULATION_AMOUNT_WETH, weth_decimals as u32)?;
        // println!("Simulating with {} WETH ({})", SIMULATION_AMOUNT_WETH, simulation_amount_weth_wei); // Less verbose logging in loop

        // --- Fetch Prices ---
        // Encapsulate price fetching in an inner async block or function for better error isolation?
        // For now, continue with current structure.
        println!("Fetching prices...");
        let fetch_result = async {
            let uni_slot0_call = uni_v3_pool.slot_0().call();
            let velo_reserves_call = velo_v2_pool.get_reserves().call();

            // Execute calls concurrently
            tokio::try_join!(uni_slot0_call, velo_reserves_call)
        }.await;

        match fetch_result {
            Ok((slot0_data, reserves)) => {
                // Process prices (using helpers from utils)
                let p_uni_res = v3_price_from_sqrt(slot0_data.0, uni_decimals0, uni_decimals1)
                    .map(|price_native| if uni_token0 == weth_address { price_native } else { if price_native.abs() < f64::EPSILON {0.0} else {1.0 / price_native} });
                let p_velo_res = v2_price_from_reserves(reserves.0.into(), reserves.1.into(), weth_decimals, usdc_decimals) // Assuming t0=WETH for calc
                    .map(|price| if velo_t0_is_weth { price } else { if price.abs() < f64::EPSILON { 0.0 } else { 1.0 / price } });

                match (p_uni_res, p_velo_res) {
                     (Ok(p_uni), Ok(p_velo)) => {
                         println!("  UniV3 Price: {:.6} | VeloV2 Price: {:.6}", p_uni, p_velo);
                         let price_diff = (p_uni - p_velo).abs();
                         let base_price = p_uni.min(p_velo);
                         let spread_percentage = if base_price > 1e-18 { (price_diff / base_price) * 100.0 } else { 0.0 };
                         println!("  -> Spread: {:.4}% (Threshold: {}%)", spread_percentage, ARBITRAGE_THRESHOLD_PERCENTAGE);

                         if spread_percentage > ARBITRAGE_THRESHOLD_PERCENTAGE {
                            println!("  >>> Opportunity DETECTED!");

                            let amount_in_wei = simulation_amount_weth_wei;
                            let token_in = weth_address;
                            let token_out = usdc_address;

                            let (buy_dex, sell_dex, buy_dex_stable, sell_dex_stable, buy_dex_fee, sell_dex_fee) = if p_uni < p_velo {
                                ("UniV3", "VeloV2", false, velo_is_stable, uni_fee, 0u32)
                            } else {
                                ("VeloV2", "UniV3", velo_is_stable, false, 0u32, uni_fee)
                            };
                            println!("      Direction: Buy {} -> Sell {}", buy_dex, sell_dex);

                            // --- Accurate Simulation ---
                            let simulation_result: Result<U256> = async {
                                let amount_out_intermediate_wei = simulate_swap(
                                    buy_dex, token_in, token_out, amount_in_wei,
                                    &velo_router, &uni_quoter, buy_dex_stable, buy_dex_fee,
                                ).await?;
                                if amount_out_intermediate_wei.is_zero() {
                                    eyre::bail!("Simulation Swap 1 resulted in zero output.");
                                }
                                let amount_out_final_wei = simulate_swap(
                                    sell_dex, token_out, token_in, amount_out_intermediate_wei,
                                    &velo_router, &uni_quoter, sell_dex_stable, sell_dex_fee,
                                 ).await?;
                                Ok(amount_out_final_wei)
                            }.await;

                            match simulation_result {
                                Ok(final_amount) => {
                                    let gross_profit_wei = final_amount.saturating_sub(amount_in_wei);
                                    println!("      Sim Gross Profit: {}", format_units(gross_profit_wei, "ether")?);

                                    let gas_price = provider.get_gas_price().await?;
                                    // println!("      Current Gas Price: {} Wei", gas_price); // Less verbose

                                    // --- Accurate Gas Estimation ---
                                    // println!("      Preparing for gas estimation..."); // Less verbose
                                    let receiver = arb_executor_address;
                                    let tokens = vec![token_in];
                                    let amounts = vec![amount_in_wei];
                                    let zero_for_one_a: bool;
                                    let pool_a_addr: Address;
                                    let pool_b_addr: Address;
                                    let is_a_velo: bool;
                                    let is_b_velo: bool;
                                    if buy_dex == "UniV3" {
                                        pool_a_addr = uni_v3_pool_address; pool_b_addr = velo_v2_pool_address;
                                        is_a_velo = false; is_b_velo = true;
                                        zero_for_one_a = (uni_token0 == weth_address);
                                    } else {
                                        pool_a_addr = velo_v2_pool_address; pool_b_addr = uni_v3_pool_address;
                                        is_a_velo = true; is_b_velo = false;
                                        zero_for_one_a = velo_t0_is_weth;
                                    }
                                    let token1_addr = token_out;
                                    let user_data = encode_user_data(
                                        pool_a_addr, pool_b_addr, token1_addr, zero_for_one_a,
                                        is_a_velo, is_b_velo, velo_router_address,
                                    )?;
                                    // println!("      Encoded User Data: {}", user_data); // Less verbose

                                    let flash_loan_calldata = balancer_vault.flash_loan(
                                        receiver, tokens.clone(), amounts.clone(), user_data
                                    ).calldata().ok_or_else(|| eyre::eyre!("Failed to get flashLoan calldata"))?;

                                    let tx_request = Eip1559TransactionRequest::new()
                                        .to(balancer_vault_address)
                                        .data(flash_loan_calldata);

                                    // println!("      Estimating gas..."); // Less verbose
                                    let estimated_gas_units = client.estimate_gas(&tx_request.clone().into(), None).await
                                        .map_err(|e| eyre::eyre!("Gas estimation failed: {}", e))?;
                                    println!("      Est. Gas Units: {}", estimated_gas_units);

                                    let gas_cost_wei = gas_price * estimated_gas_units;
                                    println!("      Est. Gas Cost: {} ETH", format_units(gas_cost_wei, "ether")?);

                                    // --- Profit Calculation ---
                                    let fee_numerator = U256::from((FLASH_LOAN_FEE_RATE * 10000.0) as u128);
                                    let fee_denominator = U256::from(10000);
                                    let flash_loan_fee_wei = amount_in_wei * fee_numerator / fee_denominator;
                                    // println!("      Est. Flash Fee: {} WETH", format_units(flash_loan_fee_wei, "ether")?); // Less verbose
                                    let total_cost_wei = gas_cost_wei + flash_loan_fee_wei;

                                    // --- Decision & Execution ---
                                    if final_amount > amount_in_wei && gross_profit_wei > total_cost_wei {
                                        let net_profit_weth_wei = gross_profit_wei.saturating_sub(total_cost_wei);
                                        println!("      Simulated NET Profit: {} WETH", format_units(net_profit_weth_wei, "ether")?);
                                        println!("      >>> EXECUTION: Sending TX <<<");

                                        // --- Send Transaction ---
                                        match client.send_transaction(tx_request.clone(), None).await {
                                            Ok(pending_tx) => {
                                                let tx_hash = pending_tx.tx_hash();
                                                println!("      >>> TX Sent: {:?}", tx_hash);

                                                println!("          Waiting for receipt...");
                                                match pending_tx.await {
                                                    Ok(Some(receipt)) => {
                                                        println!("          >>> TX Confirmed: Block #{} Gas Used: {}",
                                                                 receipt.block_number.unwrap_or_default(),
                                                                 receipt.gas_used.unwrap_or_default()
                                                        );
                                                        if receipt.status == Some(1.into()) {
                                                            println!("          ✅ Success on-chain!");
                                                        } else {
                                                            eprintln!("          ❌ TX Reverted On-Chain! Status: {:?}, Hash: {:?}", receipt.status, tx_hash);
                                                        }
                                                    }
                                                    Ok(None) => {
                                                        eprintln!("          ⚠️ Receipt not found (dropped/replaced?). Hash: {:?}", tx_hash);
                                                    }
                                                    Err(e) => {
                                                        eprintln!("          ❌ Error waiting for receipt: {}. Hash: {:?}", e, tx_hash);
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                eprintln!("      ❌ Error Sending TX: {}", e);
                                            }
                                        }

                                    } else { // Simulation predicted loss
                                        println!("      Simulated NET Loss/Insufficient: Gross {} <= Cost {}", gross_profit_wei, total_cost_wei);
                                        println!("      >>> Aborting Execution <<<");
                                    }
                                },
                                Err(sim_err) => { // Error during swap simulation calls
                                    eprintln!("  ! Simulation Error: {}", sim_err);
                                }
                            } // End match simulation_result

                         } else { // Spread not > threshold
                            // No opportunity found in this cycle
                         }
                     }
                     (Err(e), _) => eprintln!("! Error processing UniV3 price: {}", e),
                     (_, Err(e)) => eprintln!("! Error processing VeloV2 price: {}", e),
                 } // End match price results
            }
            Err(e) => {
                // Handle errors during the concurrent price fetching (e.g., RPC error)
                eprintln!("! Error fetching prices concurrently: {}", e);
                // Optionally, wait before retrying to avoid spamming RPC on persistent errors
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        } // End match fetch_result

        println!("==== Polling Cycle End ====");
    } // End loop

    // Note: Code execution will likely never reach here in the current loop structure
    // Ok(())
} // End main