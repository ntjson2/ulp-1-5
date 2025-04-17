// src/main.rs

// --- Imports ---
use ethers::{
    // abi::{Token}, // Removed unused
    prelude::*,
    // Removed unused Bytes, TransactionReceipt
    types::{Address, Eip1559TransactionRequest, U256},
    utils::{format_units},
};
use eyre::Result;
use std::{sync::Arc}; // Removed unused env, FromStr
use tokio::time::{interval, Duration};
use chrono::Utc;

// --- Module Declarations ---
mod config;
mod utils;
mod simulation;
mod bindings;
mod encoding;
mod deploy;
mod gas;

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
    IERC20,
};
use crate::encoding::encode_user_data;
use crate::deploy::deploy_contract_from_bytecode;
use crate::gas::estimate_flash_loan_gas;

// --- Constants ---
const ARBITRAGE_THRESHOLD_PERCENTAGE: f64 = 0.1;
const FLASH_LOAN_FEE_RATE: f64 = 0.0000;
const SIMULATION_AMOUNT_WETH: f64 = 1.0; // Will be replaced by optimization logic
const POLLING_INTERVAL_SECONDS: u64 = 5;
const MAX_TRADE_SIZE_VS_RESERVE_PERCENT: f64 = 5.0;

// --- Main Execution ---
#[tokio::main]
async fn main() -> Result<()> {
    let config = load_config()?; // Removed `mut`

    // --- Setup Provider & Client ---
    println!("Setting up provider & client...");
    let provider = Provider::<Http>::try_from(config.local_rpc_url.clone())?;
    let chain_id = provider.get_chainid().await?.as_u64();
    println!("RPC OK. Chain ID: {}", chain_id);
    let wallet = config.local_private_key.parse::<LocalWallet>()?.with_chain_id(chain_id);
    let client = SignerMiddleware::new(provider, wallet.clone()); // Pass provider directly
    let client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>> = Arc::new(client);
    println!("Provider & client setup complete.");

     // --- Deploy Executor Contract (Conditional) ---
    let arb_executor_address: Address;
    if config.deploy_executor {
        println!(">>> Auto-deployment enabled. Deploying executor contract...");
        let deployed_address = deploy_contract_from_bytecode(
            client.clone(),
            &config.executor_bytecode_path,
        ).await?;
        arb_executor_address = deployed_address;
        println!(">>> Executor deployed to: {:?}", arb_executor_address);
    } else {
        println!(">>> Using existing executor address from config.");
        arb_executor_address = config.arb_executor_address.expect("ARBITRAGE_EXECUTOR_ADDRESS must be set in .env if DEPLOY_EXECUTOR is not true");
        println!(">>> Using executor at: {:?}", arb_executor_address);
    }

    // --- Use Addresses (rest are from config) ---
    println!("Using addresses from config.");
    let uni_v3_pool_address = config.uni_v3_pool_addr;
    let velo_v2_pool_address = config.velo_v2_pool_addr;
    let weth_address = config.weth_address;
    let usdc_address = config.usdc_address;
    let velo_router_address = config.velo_router_addr;
    let balancer_vault_address = config.balancer_vault_address;
    let quoter_v2_address = config.quoter_v2_address;
    let weth_decimals = config.weth_decimals;
    let usdc_decimals = config.usdc_decimals;

    // --- Create Contract Instances ---
    println!("Creating contract instances...");
    let uni_v3_pool = UniswapV3Pool::new(uni_v3_pool_address, client.clone());
    let velo_v2_pool = VelodromeV2Pool::new(velo_v2_pool_address, client.clone());
    let velo_router = VelodromeRouter::new(velo_router_address, client.clone());
    let balancer_vault = BalancerVault::new(balancer_vault_address, client.clone()); // Keep instance for calldata gen
    let uni_quoter = QuoterV2::new(quoter_v2_address, client.clone());
    println!("Contract instances created.");

    // --- Determine Pool/Token Details (Fetch once initially) ---
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
        poll_interval.tick().await;
        println!("\n==== Polling Cycle Start ({}) ====", Utc::now());

        let client_clone = client.clone();
        // REMOVED unused provider variable: let provider = client.inner();

        let cycle_result = async { // Wrap cycle logic in async block

            // --- Fetch Prices ---
            println!("Fetching prices...");
            let slot0_call_builder = uni_v3_pool.slot_0();
            let reserves_call_builder = velo_v2_pool.get_reserves();
            let slot0_future = slot0_call_builder.call();
            let reserves_future = reserves_call_builder.call();
            let (slot0_data, reserves) = tokio::try_join!(
                slot0_future,
                reserves_future
            ).map_err(|e| eyre::eyre!("RPC Error fetching prices: {}", e))?;
            println!("Prices fetched.");

            // --- Calculate Prices ---
            let p_uni_res = v3_price_from_sqrt(slot0_data.0, uni_decimals0, uni_decimals1)
                .map(|price_native| if uni_token0 == weth_address { price_native } else { if price_native.abs() < f64::EPSILON {0.0} else {1.0 / price_native} });
            let (velo_calc_dec0, velo_calc_dec1) = if velo_t0_is_weth { (weth_decimals, usdc_decimals) } else { (usdc_decimals, weth_decimals) };
            let p_velo_res = v2_price_from_reserves(reserves.0.into(), reserves.1.into(), velo_calc_dec0, velo_calc_dec1)
                .map(|price| if velo_t0_is_weth { price } else { if price.abs() < f64::EPSILON { 0.0 } else { 1.0 / price } });

            let (p_uni, p_velo) = match (p_uni_res, p_velo_res) {
                (Ok(p_u), Ok(p_v)) => (p_u, p_v),
                (Err(e), _) => return Err(eyre::eyre!("Error processing UniV3 price: {}", e)),
                (_, Err(e)) => return Err(eyre::eyre!("Error processing VeloV2 price: {}", e)),
            };
            println!("  UniV3 Price: {:.6} | VeloV2 Price: {:.6}", p_uni, p_velo);

            // --- Check Spread ---
            let price_diff = (p_uni - p_velo).abs();
            let base_price = p_uni.min(p_velo);
            let spread_percentage = if base_price > 1e-18 { (price_diff / base_price) * 100.0 } else { 0.0 };
            println!("  -> Spread: {:.4}% (Threshold: {}%)", spread_percentage, ARBITRAGE_THRESHOLD_PERCENTAGE);

            // --- Arbitrage Logic ---
            if spread_percentage > ARBITRAGE_THRESHOLD_PERCENTAGE {
                println!("  >>> Opportunity DETECTED!");

                // --- TODO: Replace this fixed amount with optimal amount search ---
                let amount_in_wei = f64_to_wei(SIMULATION_AMOUNT_WETH, weth_decimals as u32)?;
                let token_in = weth_address;
                let token_out = usdc_address;

                let (buy_dex, sell_dex, buy_dex_stable, sell_dex_stable, buy_dex_fee, sell_dex_fee) = if p_uni < p_velo {
                    ("UniV3", "VeloV2", false, velo_is_stable, uni_fee, 0u32)
                } else {
                    ("VeloV2", "UniV3", velo_is_stable, false, 0u32, uni_fee)
                };
                println!("      Direction: Buy {} -> Sell {}", buy_dex, sell_dex);

                // --- LIQUIDITY PRE-CHECK ---
                println!("      Performing liquidity pre-check...");
                let pool_a_addr = if buy_dex == "UniV3" { uni_v3_pool_address } else { velo_v2_pool_address };
                let pool_a_token_in_contract = IERC20::new(token_in, client_clone.clone());
                let pool_a_token_out_contract = IERC20::new(token_out, client_clone.clone());
                let balance_in_call = pool_a_token_in_contract.balance_of(pool_a_addr);
                let balance_out_call = pool_a_token_out_contract.balance_of(pool_a_addr);
                let balance_in_future = balance_in_call.call();
                let balance_out_future = balance_out_call.call();
                let balance_check_result = async { tokio::try_join!( balance_in_future, balance_out_future ) }.await;
                match balance_check_result {
                    Ok((balance_in, balance_out)) => {
                        println!("      Pool A Balances: TokenIn ({:?}): {}, TokenOut ({:?}): {}", token_in, balance_in, token_out, balance_out);
                        let reserve_token_in = balance_in;
                        let reserve_f64 = reserve_token_in.to_f64_lossy();
                        if reserve_f64 < 1e-9 {
                             println!("      ⚠️ LIQUIDITY WARNING: Pool A's relevant reserve ({}) is near zero. Skipping.", reserve_token_in);
                             return Ok(());
                        }
                        let max_allowed_trade_f64 = reserve_f64 * (MAX_TRADE_SIZE_VS_RESERVE_PERCENT / 100.0);
                        let amount_in_f64 = amount_in_wei.to_f64_lossy();
                        if amount_in_f64 > max_allowed_trade_f64 {
                             println!("      ⚠️ LIQUIDITY WARNING: Trade size ({:.4}%) exceeds threshold ({:.2}%) of pool A's relevant reserve ({}). Skipping.",
                                (amount_in_f64 / reserve_f64) * 100.0, MAX_TRADE_SIZE_VS_RESERVE_PERCENT, reserve_token_in );
                             return Ok(());
                        } else { println!("      ✅ Liquidity sufficient."); }
                    },
                    Err(e) => { eprintln!("      ❌ Failed to fetch pool balances for liquidity check: {}. Continuing without check.", e); }
                }
                // --- End Liquidity Pre-Check ---

                // --- Accurate Simulation ---
                let simulation_result: Result<U256> = async {
                    let amount_out_intermediate_wei = simulate_swap(
                        buy_dex, token_in, token_out, amount_in_wei,
                        &velo_router, &uni_quoter, buy_dex_stable, buy_dex_fee,
                    ).await?;
                    if amount_out_intermediate_wei.is_zero() { eyre::bail!("Simulation Swap 1 resulted in zero output."); }
                    let amount_out_final_wei = simulate_swap(
                        sell_dex, token_out, token_in, amount_out_intermediate_wei,
                        &velo_router, &uni_quoter, sell_dex_stable, sell_dex_fee,
                    ).await?;
                    Ok(amount_out_final_wei)
                }.await;

                // --- Process Simulation Result ---
                match simulation_result {
                    Ok(final_amount) => {
                        let gross_profit_wei = final_amount.saturating_sub(amount_in_wei);
                        println!("      Sim Gross Profit: {}", format_units(gross_profit_wei, "ether")?);

                        let gas_price = client_clone.inner().get_gas_price().await?;
                        println!("      Current Gas Price: {} Wei", gas_price);

                        // --- Accurate Gas Estimation ---
                        let zero_for_one_a: bool;
                        let pool_a_addr: Address; let pool_b_addr: Address;
                        let is_a_velo: bool; let is_b_velo: bool;
                        if buy_dex == "UniV3" {
                            pool_a_addr = uni_v3_pool_address; pool_b_addr = velo_v2_pool_address;
                            is_a_velo = false; is_b_velo = true;
                            zero_for_one_a = uni_token0 == weth_address; // Parentheses removed
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

                        let estimated_gas_units = estimate_flash_loan_gas(
                            client_clone.clone(),
                            balancer_vault_address,
                            arb_executor_address,
                            token_in,
                            amount_in_wei,
                            user_data.clone(),
                        ).await?;

                        let gas_cost_wei = gas_price * estimated_gas_units;
                        println!("      Est. Gas Cost: {} ETH", format_units(gas_cost_wei, "ether")?);

                        // --- Profit Calculation ---
                        let fee_numerator = U256::from((FLASH_LOAN_FEE_RATE * 10000.0) as u128);
                        let fee_denominator = U256::from(10000);
                        let flash_loan_fee_wei = amount_in_wei * fee_numerator / fee_denominator;
                        let total_cost_wei = gas_cost_wei + flash_loan_fee_wei;

                        // --- Decision & Execution ---
                        if final_amount > amount_in_wei && gross_profit_wei > total_cost_wei {
                            let net_profit_weth_wei = gross_profit_wei.saturating_sub(total_cost_wei);
                            println!("      Simulated NET Profit: {} WETH", format_units(net_profit_weth_wei, "ether")?);
                            println!("      >>> EXECUTION: Sending TX <<<");

                            // --- Send Transaction ---
                            let final_flash_loan_calldata = BalancerVault::new(balancer_vault_address, client_clone.clone())
                                .flash_loan(arb_executor_address, vec![token_in], vec![amount_in_wei], user_data)
                                .calldata().ok_or_else(|| eyre::eyre!("Failed to get final flashLoan calldata"))?;

                            let final_tx_request = Eip1559TransactionRequest::new()
                                .to(balancer_vault_address)
                                .data(final_flash_loan_calldata);

                            match client_clone.send_transaction(final_tx_request.clone(), None).await {
                                Ok(pending_tx) => {
                                    let tx_hash = pending_tx.tx_hash();
                                    println!("      >>> TX Sent: {:?}", tx_hash);
                                    println!("          Waiting for receipt...");
                                    // Wait with timeout
                                    match tokio::time::timeout(Duration::from_secs(120), pending_tx).await {
                                        Ok(Ok(Some(receipt))) => { // Timeout ok, pending_tx.await ok, receipt is Some
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
                                        Ok(Ok(None)) => eprintln!("          ⚠️ Receipt not found (dropped/replaced?). Hash: {:?}", tx_hash), // pending_tx.await ok, receipt is None
                                        Ok(Err(e)) => eprintln!("          ❌ Error waiting for receipt provider error: {}. Hash: {:?}", e, tx_hash), // Error from pending_tx.await itself
                                        Err(_) => eprintln!("          ⏳ Timeout waiting for transaction receipt (120s). Hash: {:?}", tx_hash), // Timeout elapsed
                                    }
                                }
                                Err(e) => eprintln!("      ❌ Error Sending TX: {}", e),
                            } // End send_transaction match

                        } else { // Simulation predicted loss
                            println!("      Simulated NET Loss/Insufficient: Gross {} <= Cost {}", gross_profit_wei, total_cost_wei);
                            println!("      >>> Aborting Execution <<<");
                        }
                    } // End simulation_result match Ok
                    Err(sim_err) => { // Error during swap simulation calls
                        eprintln!("  ! Simulation Error: {}", sim_err);
                    }
                } // End simulation_result match
            } // End if spread > threshold
            else {
                println!("  Spread below threshold.");
            }

            Ok(()) // Indicate success for this cycle's attempt
        }.await; // End of async block for cycle logic

        // Log if the cycle encountered an error that didn't stop the loop
        if let Err(e) = cycle_result {
             eprintln!("!! Cycle Error: {} !!", e);
        }

        println!("==== Polling Cycle End ({}) ====", Utc::now());
    } // End loop
} // End main