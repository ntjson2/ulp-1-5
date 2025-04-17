// src/main.rs

// --- Imports ---
use ethers::{
    prelude::*,
    types::{Address, BlockId, BlockNumber, Eip1559TransactionRequest, I256, U256},
    utils::{format_units, parse_units}, // Keep parse_units
};
use eyre::Result;
use std::{sync::Arc};
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
use crate::simulation::find_optimal_loan_amount;
use crate::bindings::{
    UniswapV3Pool, VelodromeV2Pool, VelodromeRouter, BalancerVault, QuoterV2, IERC20,
};
use crate::encoding::encode_user_data;
use crate::deploy::deploy_contract_from_bytecode;
use crate::gas::estimate_flash_loan_gas;

// --- Constants ---
const ARBITRAGE_THRESHOLD_PERCENTAGE: f64 = 0.1;
const FLASH_LOAN_FEE_RATE: f64 = 0.0000;
const POLLING_INTERVAL_SECONDS: u64 = 5;
const MAX_TRADE_SIZE_VS_RESERVE_PERCENT: f64 = 5.0;

// --- Main Execution ---
#[tokio::main]
async fn main() -> Result<()> {
    let config = load_config()?;

    // --- Setup Provider & Client ---
    println!("Setting up provider & client...");
    let provider = Provider::<Http>::try_from(config.local_rpc_url.clone())?;
    let chain_id = provider.get_chainid().await?.as_u64();
    println!("RPC OK. Chain ID: {}", chain_id);
    let wallet = config.local_private_key.parse::<LocalWallet>()?.with_chain_id(chain_id);
    let client = SignerMiddleware::new(provider, wallet.clone());
    let client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>> = Arc::new(client);
    println!("Provider & client setup complete.");

     // --- Deploy Executor Contract (Conditional) ---
    let arb_executor_address: Address;
    if config.deploy_executor {
        println!(">>> Auto-deployment enabled. Deploying executor contract...");
        let deployed_address = deploy_contract_from_bytecode(client.clone(), &config.executor_bytecode_path).await?;
        arb_executor_address = deployed_address;
        println!(">>> Executor deployed to: {:?}", arb_executor_address);
    } else {
        println!(">>> Using existing executor address from config.");
        arb_executor_address = config.arb_executor_address.expect("ARBITRAGE_EXECUTOR_ADDRESS must be set in .env if DEPLOY_EXECUTOR is false");
        println!(">>> Using executor at: {:?}", arb_executor_address);
    }

    // --- Use Addresses ---
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
    // let balancer_vault = BalancerVault::new(balancer_vault_address, client.clone()); // Keep instance var commented out
    let uni_quoter = QuoterV2::new(quoter_v2_address, client.clone());
    println!("Contract instances created.");

    // --- Determine Pool/Token Details (Fetch once initially) ---
    println!("Fetching initial pool details...");
    let velo_token0 = velo_v2_pool.token_0().call().await?;
    let velo_token1 = velo_v2_pool.token_1().call().await?;
    let velo_is_stable = velo_v2_pool.stable().call().await?;
    println!("  Velo Pool Stable: {}", velo_is_stable);
    let (_velo_decimals0, _velo_decimals1, velo_t0_is_weth) = if velo_token0 == weth_address && velo_token1 == usdc_address { (weth_decimals, usdc_decimals, true) }
        else if velo_token0 == usdc_address && velo_token1 == weth_address { (usdc_decimals, weth_decimals, false) }
        else { eyre::bail!("Velo pool tokens ({:?}, {:?}) do not match WETH/USDC addresses in .env", velo_token0, velo_token1); };

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
        let uni_v3_pool_clone = uni_v3_pool.clone();
        let velo_v2_pool_clone = velo_v2_pool.clone();
        let velo_router_clone = velo_router.clone();
        let uni_quoter_clone = uni_quoter.clone();
        let arb_executor_addr_clone = arb_executor_address;
        let balancer_vault_addr_clone = balancer_vault_address;
        let config_clone = config.clone();


        let cycle_result = async {

            // --- Fetch Prices ---
            println!("Fetching prices...");
            let slot0_call_builder = uni_v3_pool_clone.slot_0();
            let reserves_call_builder = velo_v2_pool_clone.get_reserves();
            let slot0_future = slot0_call_builder.call();
            let reserves_future = reserves_call_builder.call();
            let (slot0_data, reserves) = tokio::try_join!(slot0_future, reserves_future)
                .map_err(|e| eyre::eyre!("RPC Error fetching prices: {}", e))?;
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
                let token_in = weth_address; let token_out = usdc_address;
                let (buy_dex, sell_dex, buy_dex_stable, sell_dex_stable, buy_dex_fee, sell_dex_fee) = if p_uni < p_velo {
                    ("UniV3", "VeloV2", false, velo_is_stable, uni_fee, 0u32)
                } else { ("VeloV2", "UniV3", velo_is_stable, false, 0u32, uni_fee) };
                println!("      Direction: Buy {} -> Sell {}", buy_dex, sell_dex);
                let zero_for_one_a: bool;
                let pool_a_addr: Address; let pool_b_addr: Address;
                let is_a_velo: bool; let is_b_velo: bool;
                if buy_dex == "UniV3" {
                    pool_a_addr = uni_v3_pool_address; pool_b_addr = velo_v2_pool_address;
                    is_a_velo = false; is_b_velo = true; zero_for_one_a = uni_token0 == weth_address;
                } else {
                    pool_a_addr = velo_v2_pool_address; pool_b_addr = uni_v3_pool_address;
                    is_a_velo = true; is_b_velo = false; zero_for_one_a = velo_t0_is_weth;
                }
                let token1_addr = token_out;

                // --- LIQUIDITY PRE-CHECK ---
                let min_check_amount = f64_to_wei(config_clone.min_loan_amount_weth, weth_decimals as u32)?;
                println!("      Performing liquidity pre-check (based on min loan {:.4} WETH)...", config_clone.min_loan_amount_weth);
                let pool_a_token_in_contract = IERC20::new(token_in, client_clone.clone());
                let balance_in_call = pool_a_token_in_contract.balance_of(pool_a_addr);
                let balance_in_future = balance_in_call.call();
                match balance_in_future.await {
                    Ok(balance_in) => {
                        let reserve_token_in = balance_in; let reserve_f64 = reserve_token_in.to_f64_lossy();
                        if reserve_f64 < 1e-9 { println!("      ⚠️ LIQUIDITY WARNING: Pool A reserve near zero. Skipping."); return Ok(()); }
                        let max_allowed_trade_f64 = reserve_f64 * (MAX_TRADE_SIZE_VS_RESERVE_PERCENT / 100.0);
                        let check_amount_f64 = min_check_amount.to_f64_lossy();
                        if check_amount_f64 > max_allowed_trade_f64 { println!("      ⚠️ LIQUIDITY WARNING: Min loan amount exceeds threshold. Skipping."); return Ok(()); }
                        else { println!("      ✅ Initial liquidity sufficient."); }
                    },
                    Err(e) => { eprintln!("      ❌ Failed to fetch pool balance for liquidity check: {}. Continuing without check.", e); }
                } // --- End Liquidity Pre-Check ---

                // --- Find Optimal Loan Amount ---
                let optimal_result = find_optimal_loan_amount(
                    client_clone.clone(), config_clone.min_loan_amount_weth, config_clone.max_loan_amount_weth,
                    config_clone.optimal_loan_search_iterations, token_in, token_out, weth_decimals,
                    FLASH_LOAN_FEE_RATE, buy_dex, sell_dex, buy_dex_stable, sell_dex_stable,
                    buy_dex_fee, sell_dex_fee, &velo_router_clone, &uni_quoter_clone, arb_executor_addr_clone,
                    balancer_vault_addr_clone, pool_a_addr, pool_b_addr, zero_for_one_a, is_a_velo,
                    is_b_velo, velo_router_address,
                ).await?;

                // --- Process Optimization Result & Execute ---
                if let Some((optimal_amount_wei, max_profit_wei)) = optimal_result {
                    if max_profit_wei <= I256::zero() { println!("      Optimal search found no profitable amount. Aborting execution."); return Ok(()); }
                    println!("      Optimal Loan Amount Found: {} WETH", format_units(optimal_amount_wei, "ether")?);
                    println!("      Estimated Max Net Profit: {} WETH", format_units(max_profit_wei.into_raw(), "ether")?);

                    // --- FINAL Gas Estimation & EIP-1559 Setup ---
                    println!("      Setting up EIP-1559 fees and final gas estimate...");
                    let base_fee = match client_clone.inner().get_block(BlockId::Number(BlockNumber::Latest)).await? {
                         Some(block) => block.base_fee_per_gas.ok_or_else(|| eyre::eyre!("Latest block missing base_fee_per_gas"))?,
                         None => eyre::bail!("Failed to get latest block for base fee"),
                    };
                    // FIX: Use parse_units and handle Result properly
                    let max_priority_fee_wei = parse_units(config_clone.max_priority_fee_per_gas_gwei, "gwei")
                        .map_err(|e| eyre::eyre!("Failed to parse max_priority_fee_gwei: {}", e))?; // Handle error
                    // Calculate max fee
                    let max_fee_wei = (base_fee * 2) + max_priority_fee_wei;
                    println!("      Base Fee: {}, Priority Fee: {}, Max Fee: {}", base_fee, max_priority_fee_wei, max_fee_wei);

                    // Estimate gas for the optimal amount
                    let final_user_data = encode_user_data( pool_a_addr, pool_b_addr, token1_addr, zero_for_one_a, is_a_velo, is_b_velo, velo_router_address )?;
                    let estimated_gas_units = estimate_flash_loan_gas( client_clone.clone(), balancer_vault_address, arb_executor_address, token_in, optimal_amount_wei, final_user_data.clone() ).await?;

                    // Calculate estimated cost using EIP-1559 approach
                    let estimated_cost_per_gas = base_fee + max_priority_fee_wei;
                    let final_gas_cost_wei = estimated_cost_per_gas * estimated_gas_units;
                    println!("      Final Est. Gas Cost (EIP-1559): {} ETH", format_units(final_gas_cost_wei, "ether")?);

                    let fee_numerator = U256::from((FLASH_LOAN_FEE_RATE * 10000.0) as u128);
                    let fee_denominator = U256::from(10000);
                    let final_flash_loan_fee_wei = optimal_amount_wei * fee_numerator / fee_denominator;
                    let final_total_cost_wei = final_gas_cost_wei + final_flash_loan_fee_wei;

                    // Final check using EIP-1559 estimated cost
                    if max_profit_wei > I256::from_raw(final_total_cost_wei) {
                        println!("      >>> Final Check Passed. EXECUTION: Sending TX <<<");

                        // --- Send Transaction ---
                        let final_flash_loan_calldata = BalancerVault::new(balancer_vault_address, client_clone.clone())
                            .flash_loan(arb_executor_address, vec![token_in], vec![optimal_amount_wei], final_user_data)
                            .calldata().ok_or_else(|| eyre::eyre!("Failed to get final flashLoan calldata"))?;

                        let final_tx_request = Eip1559TransactionRequest::new()
                            .to(balancer_vault_address)
                            .data(final_flash_loan_calldata)
                            .max_priority_fee_per_gas(max_priority_fee_wei)
                            .max_fee_per_gas(max_fee_wei);

                        match client_clone.send_transaction(final_tx_request.clone(), None).await {
                            Ok(pending_tx) => {
                                let tx_hash = pending_tx.tx_hash();
                                println!("      >>> TX Sent: {:?}", tx_hash);
                                println!("          Waiting for receipt...");
                                match tokio::time::timeout(Duration::from_secs(120), pending_tx).await {
                                    Ok(Ok(Some(receipt))) => {
                                         println!("          >>> TX Confirmed: Block #{} Gas Used: {}", receipt.block_number.unwrap_or_default(), receipt.gas_used.unwrap_or_default() );
                                         let effective_gas_price = receipt.effective_gas_price.unwrap_or_default();
                                         let actual_cost = receipt.gas_used.unwrap_or_default() * effective_gas_price;
                                         println!("          Actual TX Cost: {} ETH (Effective Gas Price: {} Gwei)", format_units(actual_cost, "ether")?, format_units(effective_gas_price, "gwei")? );
                                         if receipt.status == Some(1.into()) { println!("          ✅ Success on-chain!"); }
                                         else { eprintln!("          ❌ TX Reverted On-Chain! Status: {:?}, Hash: {:?}", receipt.status, tx_hash); }
                                    }
                                    Ok(Ok(None)) => eprintln!("          ⚠️ Receipt not found (dropped?). Hash: {:?}", tx_hash),
                                    Ok(Err(e)) => eprintln!("          ❌ Error waiting for receipt provider error: {}. Hash: {:?}", e, tx_hash),
                                    Err(_) => eprintln!("          ⏳ Timeout waiting for receipt (120s). Hash: {:?}", tx_hash),
                                }
                            }
                            Err(e) => eprintln!("      ❌ Error Sending TX: {}", e),
                        } // End send_transaction match

                    } else {
                        println!("      >>> Final Check FAILED: Re-estimated cost {} exceeds max profit {}. Aborting Execution <<<",
                            format_units(final_total_cost_wei, "ether")?,
                            format_units(max_profit_wei.into_raw(), "ether")?
                        );
                    }
                } else { println!("      No profitable loan amount found by search. Aborting Execution."); }

            } else { println!("  Spread below threshold."); } // End if spread > threshold

            Ok(())
        }.await;

        if let Err(e) = cycle_result { eprintln!("!! Cycle Error: {} !!", e); }
        println!("==== Polling Cycle End ({}) ====", Utc::now());
    } // End loop
} // End main

// END OF FILE: src/main.rs