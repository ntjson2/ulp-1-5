// bot/src/main.rs

// --- Imports ---
use ethers::{
    prelude::*,
    types::{Address, BlockId, BlockNumber, Eip1559TransactionRequest, I256, U256},
    utils::{format_units, parse_units},
};
use eyre::Result;
use std::{sync::Arc, cmp::max};
use tokio::time::{interval, Duration};
use chrono::Utc;
use clap::Parser; // Import clap

// --- Module Declarations ---
// Assumes these files are in the same directory (bot/src/)
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
use crate::bindings::{ // Import contract types from bindings
    UniswapV3Pool, VelodromeV2Pool, VelodromeRouter, BalancerVault, QuoterV2, IERC20,
    ArbitrageExecutor, // Import the executor binding
};
use crate::encoding::encode_user_data;
use crate::deploy::deploy_contract_from_bytecode;
use crate::gas::estimate_flash_loan_gas;

// --- Constants ---
const ARBITRAGE_THRESHOLD_PERCENTAGE: f64 = 0.1;
const FLASH_LOAN_FEE_RATE: f64 = 0.0000;
const POLLING_INTERVAL_SECONDS: u64 = 5;
const MAX_TRADE_SIZE_VS_RESERVE_PERCENT: f64 = 5.0;

// --- CLI Argument Parsing ---
#[derive(Parser, Debug)]
#[command(author, version, about = "ULP 1.5 Cross-DEX Arbitrage Bot", long_about = None)]
struct Cli {
    /// Optional: Token address to withdraw from the executor contract.
    #[arg(long = "withdraw-token", value_name = "TOKEN_ADDRESS")]
    withdraw_token: Option<String>,

    /// Optional: Address to send withdrawn tokens to. Requires --withdraw-token.
    #[arg(long = "withdraw-recipient", value_name = "RECIPIENT_ADDRESS", requires = "withdraw_token")]
    withdraw_recipient: Option<String>,
}

// --- Main Execution ---
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = load_config()?;

    // --- Mode Selection ---
    if let (Some(token_str), Some(recipient_str)) = (cli.withdraw_token, cli.withdraw_recipient) {
        // --- WITHDRAWAL MODE ---
        println!("--- Running in Withdrawal Mode ---");

        let token_address = token_str.parse::<Address>()?;
        let recipient_address = recipient_str.parse::<Address>()?;
        println!("Attempting to withdraw token {:?} to recipient {:?}", token_address, recipient_address);

        let provider = Provider::<Http>::try_from(config.local_rpc_url)?;
        let chain_id = provider.get_chainid().await?.as_u64();
        let wallet = config.local_private_key.parse::<LocalWallet>()?.with_chain_id(chain_id);
        let client = Arc::new(SignerMiddleware::new(provider, wallet));

        let executor_address = config.arb_executor_address
             .ok_or_else(|| eyre::eyre!("ARBITRAGE_EXECUTOR_ADDRESS must be set in .env for withdrawal mode"))?;
        println!("Using executor contract at: {:?}", executor_address);

        let executor_contract = ArbitrageExecutor::new(executor_address, client.clone());

        println!("Preparing withdrawal transaction...");
        let tx_call = executor_contract.withdraw_token(token_address, recipient_address);
        let tx: Eip1559TransactionRequest = tx_call.tx.into(); // Convert CallBuilder Tx to concrete type

        println!("Sending withdrawal transaction...");
        match client.send_transaction(tx, None).await {
             Ok(pending_tx) => {
                 let tx_hash = pending_tx.tx_hash();
                 println!("  Withdrawal TX Sent: {:?}", tx_hash);
                 println!("  Waiting for receipt...");
                 match tokio::time::timeout(Duration::from_secs(120), pending_tx).await {
                     Ok(Ok(Some(receipt))) => {
                         println!("  >>> TX Confirmed: Block #{} Gas Used: {}", receipt.block_number.unwrap_or_default(), receipt.gas_used.unwrap_or_default() );
                         if receipt.status == Some(1.into()) { println!("  ✅ Withdrawal Successful!"); }
                         else { eprintln!("  ❌ Withdrawal TX Reverted! Status: {:?}, Hash: {:?}", receipt.status, tx_hash); }
                     }
                     Ok(Ok(None)) => eprintln!("  ⚠️ Receipt not found (dropped?). Hash: {:?}", tx_hash),
                     Ok(Err(e)) => eprintln!("  ❌ Error waiting for withdrawal receipt: {}. Hash: {:?}", e, tx_hash),
                     Err(_) => eprintln!("  ⏳ Timeout waiting for withdrawal receipt (120s). Hash: {:?}", tx_hash),
                 }
             }
             Err(e) => eprintln!("  ❌ Error Sending Withdrawal TX: {}", e),
        }
        println!("--- Withdrawal Attempt Complete ---");

    } else {
        // --- POLLING MODE ---
        println!("--- Running in Polling Mode ---");
        // Setup Provider & Client
        let provider = Provider::<Http>::try_from(config.local_rpc_url.clone())?;
        let chain_id = provider.get_chainid().await?.as_u64();
        println!("RPC OK. Chain ID: {}", chain_id);
        let wallet = config.local_private_key.parse::<LocalWallet>()?.with_chain_id(chain_id);
        let client = SignerMiddleware::new(provider, wallet.clone());
        let client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>> = Arc::new(client);
        println!("Provider & client setup complete.");

         // Deploy Executor Contract (Conditional)
        let arb_executor_address: Address;
        if config.deploy_executor {
            println!(">>> Auto-deployment enabled. Deploying executor contract...");
            arb_executor_address = deploy_contract_from_bytecode(client.clone(), &config.executor_bytecode_path).await?;
            println!(">>> Executor deployed to: {:?}", arb_executor_address);
        } else {
            println!(">>> Using existing executor address from config.");
            arb_executor_address = config.arb_executor_address.expect("ARBITRAGE_EXECUTOR_ADDRESS must be set for polling if DEPLOY_EXECUTOR=false");
            println!(">>> Using executor at: {:?}", arb_executor_address);
        }

        // Use Addresses & Create Instances
        let uni_v3_pool_address = config.uni_v3_pool_addr;
        let velo_v2_pool_address = config.velo_v2_pool_addr; // FIX: Correct variable name was used here
        let weth_address = config.weth_address; let usdc_address = config.usdc_address;
        let velo_router_address = config.velo_router_addr;
        let balancer_vault_address = config.balancer_vault_address;
        let quoter_v2_address = config.quoter_v2_address;
        let weth_decimals = config.weth_decimals; let usdc_decimals = config.usdc_decimals;
        println!("Creating contract instances...");
        let uni_v3_pool = UniswapV3Pool::new(uni_v3_pool_address, client.clone());
        let velo_v2_pool = VelodromeV2Pool::new(velo_v2_pool_address, client.clone()); // Use correct variable
        let velo_router = VelodromeRouter::new(velo_router_address, client.clone());
        let uni_quoter = QuoterV2::new(quoter_v2_address, client.clone());
        println!("Contract instances created.");

        // Fetch Initial Pool Details
        println!("Fetching initial pool details...");
        let velo_token0 = velo_v2_pool.token_0().call().await?;
        let velo_token1 = velo_v2_pool.token_1().call().await?;
        let velo_is_stable = velo_v2_pool.stable().call().await?;
        println!("  Velo Pool Stable: {}", velo_is_stable);
        // FIX: Restore full logic block
        let (_velo_decimals0, _velo_decimals1, velo_t0_is_weth) = if velo_token0 == weth_address && velo_token1 == usdc_address {
            (weth_decimals, usdc_decimals, true)
        } else if velo_token0 == usdc_address && velo_token1 == weth_address {
            (usdc_decimals, weth_decimals, false)
        } else {
            eyre::bail!("Velo pool tokens ({:?}, {:?}) do not match WETH/USDC addresses in .env", velo_token0, velo_token1);
        };
        let uni_token0 = uni_v3_pool.token_0().call().await?;
        let uni_token1 = uni_v3_pool.token_1().call().await?; // FIX: Restore variable
        let uni_fee = uni_v3_pool.fee().call().await?;
        println!("  Uni Pool Fee: {}", uni_fee);
        // FIX: Restore full logic block
         if !(uni_token0 == weth_address && uni_token1 == usdc_address) && !(uni_token0 == usdc_address && uni_token1 == weth_address) {
             eyre::bail!("Uni pool tokens ({:?}, {:?}) do not match WETH/USDC addresses in .env", uni_token0, uni_token1);
         }
        let uni_decimals0 = weth_decimals;
        let uni_decimals1 = usdc_decimals;
        println!("Initial pool details fetched."); // FIX: Restore print


        // Initialize Polling Timer & Start Loop
        let mut poll_interval = interval(Duration::from_secs(POLLING_INTERVAL_SECONDS));
        println!("\n--- Starting Continuous Polling (Interval: {}s) ---", POLLING_INTERVAL_SECONDS);

        loop {
            poll_interval.tick().await; println!("\n==== Polling Cycle Start ({}) ====", Utc::now());
            let client_clone = client.clone(); let uni_v3_pool_clone = uni_v3_pool.clone(); let velo_v2_pool_clone = velo_v2_pool.clone(); let velo_router_clone = velo_router.clone(); let uni_quoter_clone = uni_quoter.clone(); let arb_executor_addr_clone = arb_executor_address; let balancer_vault_addr_clone = balancer_vault_address; let config_clone = config.clone();

            let cycle_result = async {
                // Fetch Prices
                println!("Fetching prices...");
                let slot0_call_builder = uni_v3_pool_clone.slot_0(); let reserves_call_builder = velo_v2_pool_clone.get_reserves();
                let slot0_future = slot0_call_builder.call(); let reserves_future = reserves_call_builder.call();
                let (slot0_data, reserves) = tokio::try_join!(slot0_future, reserves_future).map_err(|e| eyre::eyre!("RPC Error fetching prices: {}", e))?;
                println!("Prices fetched.");

                // Calculate Prices
                let p_uni_res = v3_price_from_sqrt(slot0_data.0, uni_decimals0, uni_decimals1).map(|p| if uni_token0 == weth_address { p } else { if p.abs() < f64::EPSILON {0.0} else {1.0 / p} });
                let (d0, d1) = if velo_t0_is_weth {(weth_decimals, usdc_decimals)} else {(usdc_decimals, weth_decimals)};
                let p_velo_res = v2_price_from_reserves(reserves.0.into(), reserves.1.into(), d0, d1).map(|p| if velo_t0_is_weth {p} else { if p.abs() < f64::EPSILON {0.0} else {1.0 / p} });
                let (p_uni, p_velo) = match (p_uni_res, p_velo_res) { (Ok(u), Ok(v)) => (u, v), (Err(e), _) => return Err(e.wrap_err("Uni price error")), (_, Err(e)) => return Err(e.wrap_err("Velo price error")), };
                println!("  UniV3 Price: {:.6} | VeloV2 Price: {:.6}", p_uni, p_velo);

                // Check Spread
                let price_diff = (p_uni - p_velo).abs(); let base_price = p_uni.min(p_velo);
                let spread_percentage = if base_price > 1e-18 { (price_diff / base_price) * 100.0 } else { 0.0 };
                println!("  -> Spread: {:.4}% (Threshold: {}%)", spread_percentage, ARBITRAGE_THRESHOLD_PERCENTAGE);

                // Arbitrage Logic
                if spread_percentage > ARBITRAGE_THRESHOLD_PERCENTAGE {
                    println!("  >>> Opportunity DETECTED!");
                    let token_in = weth_address; let token_out = usdc_address;
                    let (buy_dex, sell_dex, buy_dex_stable, sell_dex_stable, buy_dex_fee, sell_dex_fee) = if p_uni < p_velo { ("UniV3", "VeloV2", false, velo_is_stable, uni_fee, 0u32) } else { ("VeloV2", "UniV3", velo_is_stable, false, 0u32, uni_fee) };
                    println!("      Direction: Buy {} -> Sell {}", buy_dex, sell_dex);
                    let zero_for_one_a: bool; let pool_a_addr: Address; let pool_b_addr: Address; let is_a_velo: bool; let is_b_velo: bool;
                    if buy_dex == "UniV3" { pool_a_addr = uni_v3_pool_address; pool_b_addr = velo_v2_pool_address; is_a_velo = false; is_b_velo = true; zero_for_one_a = uni_token0 == weth_address; }
                    else { pool_a_addr = velo_v2_pool_address; pool_b_addr = uni_v3_pool_address; is_a_velo = true; is_b_velo = false; zero_for_one_a = velo_t0_is_weth; }
                    let token1_addr = token_out;

                    // LIQUIDITY PRE-CHECK
                    let min_check_amount = f64_to_wei(config_clone.min_loan_amount_weth, weth_decimals as u32)?;
                    println!("      Performing liquidity pre-check (based on min loan {:.4} WETH)...", config_clone.min_loan_amount_weth);
                    let pool_a_token_in_contract = IERC20::new(token_in, client_clone.clone());
                    let balance_in_call = pool_a_token_in_contract.balance_of(pool_a_addr); let balance_in_future = balance_in_call.call();
                    match balance_in_future.await {
                        Ok(balance_in) => {
                            let reserve_token_in = balance_in; let reserve_f64 = reserve_token_in.to_f64_lossy();
                            if reserve_f64 < 1e-9 { println!("      ⚠️ LIQUIDITY WARNING: Pool A reserve near zero. Skipping."); return Ok(()); }
                            let max_allowed_trade_f64 = reserve_f64 * (MAX_TRADE_SIZE_VS_RESERVE_PERCENT / 100.0); let check_amount_f64 = min_check_amount.to_f64_lossy();
                            if check_amount_f64 > max_allowed_trade_f64 { println!("      ⚠️ LIQUIDITY WARNING: Min loan amount exceeds threshold. Skipping."); return Ok(()); }
                            else { println!("      ✅ Initial liquidity sufficient."); }
                        },
                        Err(e) => { eprintln!("      ❌ Failed to fetch pool balance for liquidity check: {}. Continuing without check.", e); }
                    }

                    // Find Optimal Loan Amount
                    let optimal_result = find_optimal_loan_amount( client_clone.clone(), config_clone.min_loan_amount_weth, config_clone.max_loan_amount_weth,
                        config_clone.optimal_loan_search_iterations, token_in, token_out, weth_decimals, FLASH_LOAN_FEE_RATE, buy_dex, sell_dex,
                        buy_dex_stable, sell_dex_stable, buy_dex_fee, sell_dex_fee, &velo_router_clone, &uni_quoter_clone, arb_executor_addr_clone,
                        balancer_vault_addr_clone, pool_a_addr, pool_b_addr, zero_for_one_a, is_a_velo, is_b_velo, velo_router_address, ).await?;

                    // Process Optimization Result & Execute
                    if let Some((optimal_amount_wei, max_profit_wei)) = optimal_result {
                        if max_profit_wei <= I256::zero() { println!("      Optimal search found no profitable amount. Aborting execution."); return Ok(()); }
                        println!("      Optimal Loan Amount Found: {} WETH", format_units(optimal_amount_wei, "ether")?);
                        println!("      Estimated Max Net Profit: {} WETH", format_units(max_profit_wei.into_raw(), "ether")?);

                        // FINAL Gas Estimation & EIP-1559 Setup
                        println!("      Setting up EIP-1559 fees and final gas estimate...");
                        let base_fee = match client_clone.inner().get_block(BlockId::Number(BlockNumber::Latest)).await? { Some(b) => b.base_fee_per_gas.ok_or_else(|| eyre::eyre!("Block missing base_fee"))?, None => eyre::bail!("Failed to get latest block") };
                        let max_priority_fee_wei: U256 = parse_units(config_clone.max_priority_fee_per_gas_gwei, "gwei")?.into();
                        let max_fee_wei = (base_fee * 2) + max_priority_fee_wei; println!("      Base Fee: {}, Priority Fee: {}, Max Fee: {}", base_fee, max_priority_fee_wei, max_fee_wei);
                        let final_user_data = encode_user_data( pool_a_addr, pool_b_addr, token1_addr, zero_for_one_a, is_a_velo, is_b_velo, velo_router_address )?;
                        let estimated_gas_units = estimate_flash_loan_gas( client_clone.clone(), balancer_vault_address, arb_executor_address, token_in, optimal_amount_wei, final_user_data.clone() ).await?;
                        let gas_limit_buffer_mult = U256::from(100 + config_clone.gas_limit_buffer_percentage); let gas_limit_buffered = estimated_gas_units.saturating_mul(gas_limit_buffer_mult).checked_div(U256::from(100)).unwrap_or(estimated_gas_units); let min_limit = U256::from(config_clone.min_flashloan_gas_limit);
                        let final_gas_limit = max(gas_limit_buffered, min_limit); println!("      Est. Gas Units: {}, Buffered Limit: {}", estimated_gas_units, final_gas_limit);
                        let estimated_cost_per_gas = base_fee + max_priority_fee_wei; let final_gas_cost_wei = estimated_cost_per_gas * estimated_gas_units;
                        println!("      Final Est. Gas Cost (EIP-1559): {} ETH", format_units(final_gas_cost_wei, "ether")?);
                        let fee_numerator = U256::from((FLASH_LOAN_FEE_RATE * 10000.0) as u128); let fee_denominator = U256::from(10000); let final_flash_loan_fee_wei = optimal_amount_wei * fee_numerator / fee_denominator;
                        let final_total_cost_wei = final_gas_cost_wei + final_flash_loan_fee_wei;

                        // Final Check
                        if max_profit_wei > I256::from_raw(final_total_cost_wei) {
                            println!("      >>> Final Check Passed. EXECUTION: Sending TX <<<");
                            // Send Transaction
                            // FIX: Restore full logic for calldata generation
                            let final_flash_loan_calldata = BalancerVault::new(balancer_vault_address, client_clone.clone())
                                .flash_loan(arb_executor_address, vec![token_in], vec![optimal_amount_wei], final_user_data)
                                .calldata().ok_or_else(|| eyre::eyre!("Failed to get final flashLoan calldata"))?;
                            let final_tx_request = Eip1559TransactionRequest::new().to(balancer_vault_address).data(final_flash_loan_calldata).max_priority_fee_per_gas(max_priority_fee_wei).max_fee_per_gas(max_fee_wei).gas(final_gas_limit);
                            match client_clone.send_transaction(final_tx_request.clone(), None).await {
                                Ok(pending_tx) => {
                                    let tx_hash = pending_tx.tx_hash(); println!("      >>> TX Sent: {:?}", tx_hash); println!("          Waiting for receipt...");
                                    match tokio::time::timeout(Duration::from_secs(120), pending_tx).await {
                                        Ok(Ok(Some(receipt))) => {
                                             println!("          >>> TX Confirmed: Block #{} Gas Used: {}", receipt.block_number.unwrap_or_default(), receipt.gas_used.unwrap_or_default() );
                                             let effective_gas_price = receipt.effective_gas_price.unwrap_or_default(); let actual_cost = receipt.gas_used.unwrap_or_default() * effective_gas_price;
                                             println!("          Actual TX Cost: {} ETH (Effective Gas Price: {} Gwei)", format_units(actual_cost, "ether")?, format_units(effective_gas_price, "gwei")? );
                                             if receipt.status == Some(1.into()) { println!("          ✅ Success on-chain!"); }
                                             else { eprintln!("          ❌ TX Reverted On-Chain! Status: {:?}, Hash: {:?}", receipt.status, tx_hash); }
                                        }
                                        Ok(Ok(None)) => { eprintln!("          ⚠️ Receipt not found (dropped/replaced?). Hash: {:?}", tx_hash); }
                                        Ok(Err(e)) => { eprintln!("          ❌ Error waiting for receipt provider error: {}. Hash: {:?}", e, tx_hash); }
                                        Err(_) => { eprintln!("          ⏳ Timeout waiting for receipt (120s). Hash: {:?}", tx_hash); }
                                    }
                                }
                                Err(e) => eprintln!("      ❌ Error Sending TX: {}", e),
                            } // End send_transaction match
                        } else { println!("      >>> Final Check FAILED: Re-estimated cost {} exceeds max profit {}. Aborting Execution <<<", format_units(final_total_cost_wei, "ether")?, format_units(max_profit_wei.into_raw(), "ether")? ); }
                    } else { println!("      No profitable loan amount found by search. Aborting Execution."); }

                } else { println!("  Spread below threshold."); } // End if spread > threshold
                Ok(())
            }.await; // End of async block for cycle logic

            if let Err(e) = cycle_result { eprintln!("!! Cycle Error: {} !!", e); }
            println!("==== Polling Cycle End ({}) ====", Utc::now());
        } // End loop
    } // End else (Polling Mode)

    Ok(())
} // End main


// END OF FILE: bot/src/main.rs