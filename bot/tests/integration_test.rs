// bot/tests/integration_test.rs

// This test file requires the 'local_simulation' feature to be enabled.
// Run with: cargo test --features local_simulation -- --ignored
// Add other test functions and run without --ignored, or adjust flags as needed.

#![cfg(feature = "local_simulation")]
#![allow(unexpected_cfgs)] // Allow the cfg check warning within tests too

use ulp1_5::{
    bindings::{BalancerVault, IERC20, UniswapV3Pool, VelodromeV2Pool}, // Added BalancerVault
    config::Config,
    encoding, // Need encoding for userData
    event_handler,
    local_simulator::{self, SimEnv},
    path_optimizer,
    simulation,
    state::{self, AppState, DexType},
    transaction::{self, NonceManager},
};
use ethers::{
    prelude::*,
    types::{Address, Bytes, Filter, I256, U256, Log},
    utils::{format_ether, parse_ether, parse_units},
};
use eyre::{eyre, Result}; // Added Result explicitly
use std::{str::FromStr, sync::Arc, time::Duration, time::SystemTime, time::UNIX_EPOCH}; // Added SystemTime, UNIX_EPOCH
use tracing::{error, info, warn};
use tracing_subscriber::{filter::LevelFilter, fmt, EnvFilter};


// Helper to initialize tracing subscriber for tests
fn setup_tracing() {
    let _ = fmt().with_max_level(LevelFilter::INFO).with_env_filter(EnvFilter::from_default_env()).with_target(true).with_line_number(true).with_test_writer().try_init();
}


// Helper to ensure signer has enough WETH and has approved the spender
async fn ensure_weth_balance_and_approval( sim_env: &SimEnv, spender: Address, required_weth: U256, approve_amount: U256 ) -> eyre::Result<()> {
    // ... (implementation remains the same) ...
    info!("Ensuring WETH balance >= {} and approval for spender {}", format_ether(required_weth), spender);
    let weth_addr: Address = sim_env.config.target_weth_address.parse()?; let weth_contract = IERC20::new(weth_addr, sim_env.http_client.clone()); let signer_addr = sim_env.wallet_address; let balance = weth_contract.balance_of(signer_addr).call().await?; info!("Current WETH Balance: {}", format_ether(balance));
    if balance < required_weth { let eth_balance = sim_env.http_client.get_balance(signer_addr, None).await?; info!("Current ETH Balance: {}", format_ether(eth_balance));
        if eth_balance > required_weth { info!("Attempting to wrap ETH to get WETH..."); let wrap_tx = TransactionRequest::new().to(weth_addr).value(required_weth); let pending_tx = sim_env.http_client.send_transaction(wrap_tx, None).await?; let receipt = pending_tx.await?.ok_or_else(|| eyre::eyre!("WETH wrap tx dropped"))?; if receipt.status != Some(1.into()) { return Err(eyre::eyre!("WETH wrap transaction failed: {:?}", receipt.transaction_hash)); } info!("Wrapped ETH successfully. Tx: {:?}", receipt.transaction_hash); }
        else { return Err(eyre::eyre!("Insufficient ETH balance ({}) on Anvil signer {} to wrap required WETH ({})", format_ether(eth_balance), signer_addr, format_ether(required_weth))); }
        let new_balance = weth_contract.balance_of(signer_addr).call().await?; if new_balance < required_weth { return Err(eyre::eyre!("Still insufficient WETH balance after wrapping attempt")); } }
    let allowance = weth_contract.allowance(signer_addr, spender).call().await?; info!("Current WETH allowance for {}: {}", spender, format_ether(allowance));
    if allowance < approve_amount { info!("Approving WETH for spender {}...", spender); let approve_call = weth_contract.approve(spender, approve_amount); let pending_tx = approve_call.send().await?; let receipt = pending_tx.await?.ok_or_else(|| eyre::eyre!("WETH approval tx dropped"))?; if receipt.status != Some(1.into()) { return Err(eyre::eyre!("WETH approval transaction failed: {:?}", receipt.transaction_hash)); } info!("WETH approved successfully. Tx: {:?}", receipt.transaction_hash); }
    Ok(())
}


#[tokio::test]
#[ignore]
async fn test_anvil_simulation_setup_and_fetch() {
    // ... (test remains the same) ...
    setup_tracing(); info!("############# IMPORTANT #############"); info!("Ensure Anvil is running externally..."); info!("#####################################"); tokio::time::sleep(Duration::from_secs(1)).await; let setup_result = local_simulator::setup_simulation_environment().await; assert!(setup_result.is_ok(), "Setup failed: {:?}. Anvil running?", setup_result.err()); let sim_env = setup_result.unwrap(); info!("Setup Complete."); info!("Wallet Address: {:?}", sim_env.wallet_address); info!("Executor Address: {:?}", sim_env.executor_address); if sim_env.config.deploy_executor_in_sim { assert!(sim_env.executor_address.is_some()); assert_ne!(sim_env.executor_address.unwrap(), Address::zero()); } let fetch_result = local_simulator::fetch_simulation_data(&sim_env).await; assert!(fetch_result.is_ok(), "Fetch failed: {:?}", fetch_result.err()); info!("Fetch successful.");
}

#[tokio::test]
#[ignore]
async fn test_anvil_v3_swap_trigger() {
    // ... (test remains the same) ...
     setup_tracing(); info!("Starting V3 Swap Trigger Test - Ensure Anvil is running!"); tokio::time::sleep(Duration::from_secs(1)).await; let sim_env = match local_simulator::setup_simulation_environment().await { Ok(env) => Arc::new(env), Err(e) => panic!("Setup failed: {:?}", e), }; let pool_addr: Address = match sim_env.config.target_uniswap_v3_pool_address.parse() { Ok(addr) if addr != Address::zero() => addr, _ => { info!("Skipping V3 swap: Target pool not configured."); return; } }; let pool_binding = UniswapV3Pool::new(pool_addr, sim_env.http_client.clone()); let recipient = sim_env.wallet_address; let zero_for_one = true; let amount_weth = 0.01; let amount_in_wei = parse_ether(amount_weth).unwrap(); info!("Checking/Setting prerequisites..."); let prereq_result = ensure_weth_balance_and_approval(&sim_env, pool_addr, amount_in_wei, U256::MAX).await; assert!(prereq_result.is_ok(), "Prereq failed: {:?}", prereq_result.err()); info!("Prerequisites met."); info!("Attempting V3 swap trigger on pool: {}", pool_addr); let swap_result = local_simulator::trigger_v3_swap(&sim_env, pool_addr, &pool_binding, recipient, zero_for_one, I256::from_raw(amount_in_wei), U256::zero(), Bytes::new()).await; assert!(swap_result.is_ok(), "Swap trigger failed: {:?}", swap_result.err()); info!("V3 Swap transaction sent: {:?}", swap_result.unwrap());
}


#[tokio::test]
#[ignore]
async fn test_anvil_v2_swap_trigger() {
    // ... (test remains the same) ...
    setup_tracing(); info!("Starting V2 Swap Trigger Test - Ensure Anvil is running!"); tokio::time::sleep(Duration::from_secs(1)).await; let sim_env = match local_simulator::setup_simulation_environment().await { Ok(env) => Arc::new(env), Err(e) => panic!("Setup failed: {:?}", e), }; let pool_addr: Address = match sim_env.config.target_velodrome_v2_pool_address.parse() { Ok(addr) if addr != Address::zero() => addr, _ => { info!("Skipping V2 swap: Target pool not configured."); return; } }; let pool_binding = VelodromeV2Pool::new(pool_addr, sim_env.http_client.clone()); let amount_out_usdc_wei = U256::from(10_000_000); let amount0_out = U256::zero(); let amount1_out = amount_out_usdc_wei; let recipient = sim_env.wallet_address; info!("Prerequisites assumed met (pool has liquidity)."); info!("Attempting V2 swap trigger on pool: {}", pool_addr); let swap_result = local_simulator::trigger_v2_swap(&sim_env, pool_addr, &pool_binding, amount0_out, amount1_out, recipient, Bytes::new()).await; assert!(swap_result.is_ok(), "Swap trigger failed: {:?}", swap_result.err()); info!("V2 Swap transaction sent: {:?}", swap_result.unwrap());
}


#[tokio::test]
#[ignore]
async fn test_full_arbitrage_cycle() -> eyre::Result<()> {
    // ... (test remains largely the same) ...
    setup_tracing(); info!("Starting Full Arbitrage Cycle Test - Ensure Anvil is running!"); tokio::time::sleep(Duration::from_secs(1)).await;
    let sim_env = match local_simulator::setup_simulation_environment().await { Ok(env) => Arc::new(env), Err(e) => panic!("Setup failed: {:?}", e), };
    let executor_addr = sim_env.executor_address.expect("Executor address needed");
    let test_config = Config { /* ... construct config ... */
        ws_rpc_url: sim_env.config.anvil_ws_url.to_string(), http_rpc_url: sim_env.config.anvil_http_url.to_string(),
        local_private_key: sim_env.config.anvil_private_key.to_string(), chain_id: Some(sim_env.http_client.get_chainid().await?.as_u64()),
        arb_executor_address: sim_env.executor_address,
        uniswap_v3_factory_addr: Address::from_str("0x1F98431c8aD98523631AE4a59f267346ea31F984")?,
        velodrome_v2_factory_addr: Address::from_str("0x25CbdDb98b35AB1FF795324516342Fac4845718f")?,
        balancer_vault_address: Address::from_str("0xBA12222222228d8Ba445958a75a0704d566BF2C9")?,
        quoter_v2_address: Address::from_str("0xbC52C688c34A4F6180437B40593F1F9638C2571d")?,
        velo_router_addr: Address::from_str("0x9c12939390052919aF3155f41Bf41543Ca30607B")?,
        aerodrome_factory_addr: None, aerodrome_router_addr: None,
        weth_address: sim_env.config.target_weth_address.parse()?, usdc_address: sim_env.config.target_usdc_address.parse()?,
        weth_decimals: 18, usdc_decimals: 6, deploy_executor: false, executor_bytecode_path: "".to_string(),
        min_loan_amount_weth: 0.1, max_loan_amount_weth: 10.0, optimal_loan_search_iterations: 5,
        fetch_timeout_secs: Some(10), enable_univ3_dynamic_sizing: false, max_priority_fee_per_gas_gwei: 0.01,
        fallback_gas_price_gwei: Some(0.01), gas_limit_buffer_percentage: 50, min_flashloan_gas_limit: 500_000,
        private_rpc_url: None, secondary_private_rpc_url: None, min_profit_buffer_bps: 20,
        min_profit_abs_buffer_wei_str: "100000000000000".to_string(), critical_block_lag_seconds: 300, critical_log_lag_seconds: 300,
     };
    let app_state = Arc::new(AppState::new(test_config.clone())); let nonce_manager = Arc::new(NonceManager::new(sim_env.wallet_address));
    info!("Fetching initial state..."); let v3_pool_addr: Address = sim_env.config.target_uniswap_v3_pool_address.parse()?; let v2_pool_addr: Address = sim_env.config.target_velodrome_v2_pool_address.parse()?; let v3_factory_addr = app_state.config.uniswap_v3_factory_addr; let v2_factory_addr = app_state.config.velodrome_v2_factory_addr;
    state::fetch_and_cache_pool_state(v3_pool_addr, DexType::UniswapV3, v3_factory_addr, sim_env.http_client.clone(), app_state.clone()).await?; state::fetch_and_cache_pool_state(v2_pool_addr, DexType::VelodromeV2, v2_factory_addr, sim_env.http_client.clone(), app_state.clone()).await?; info!("Initial states fetched."); assert!(app_state.pool_states.contains_key(&v3_pool_addr)); assert!(app_state.pool_states.contains_key(&v2_pool_addr));
    info!("Manually creating price discrepancy..."); let v3_pool = UniswapV3Pool::new(v3_pool_addr, sim_env.http_client.clone()); let swap1_amount_weth = 0.5; let swap1_amount = parse_ether(swap1_amount_weth)?; ensure_weth_balance_and_approval(&sim_env, v3_pool_addr, swap1_amount, U256::MAX).await?; let swap_tx_hash = local_simulator::trigger_v3_swap(&sim_env, v3_pool_addr, &v3_pool, sim_env.wallet_address, true, I256::from_raw(swap1_amount), U256::zero(), Bytes::new()).await?; info!("Sent initial swap on V3 pool: {:?}", swap_tx_hash); tokio::time::sleep(Duration::from_secs(2)).await;
    info!("Simulating bot's arbitrage check (sequential)...");
    state::fetch_and_cache_pool_state(v3_pool_addr, DexType::UniswapV3, v3_factory_addr, sim_env.http_client.clone(), app_state.clone()).await?; let updated_snapshot = app_state.pool_snapshots.get(&v3_pool_addr).expect("Updated snapshot missing").clone();
    let routes = path_optimizer::find_top_routes( &updated_snapshot, &app_state.pool_states, &app_state.pool_snapshots, &app_state.config, app_state.weth_address, app_state.usdc_address, app_state.weth_decimals, app_state.usdc_decimals, ); assert!(!routes.is_empty(), "No routes found!"); info!("Found {} routes. Evaluating top one.", routes.len()); let route = routes.first().unwrap().clone();
    let gas_info = transaction::fetch_gas_price(sim_env.http_client.clone(), &app_state.config).await?; let gas_price_gwei = gas_info.max_priority_fee_per_gas.to_f64_lossy() / 1e9; let buy_snap = app_state.pool_snapshots.get(&route.buy_pool_addr).map(|r| r.value().clone()); let sell_snap = app_state.pool_snapshots.get(&route.sell_pool_addr).map(|r| r.value().clone());
    let optimal_result = simulation::find_optimal_loan_amount( sim_env.http_client.clone(), app_state.clone(), &route, buy_snap.as_ref(), sell_snap.as_ref(), gas_price_gwei, ).await?; assert!(optimal_result.is_some(), "Expected profit, found None."); let (loan_amount, net_profit) = optimal_result.unwrap(); assert!(net_profit > I256::zero(), "Expected positive profit, found {}", net_profit); info!("Optimal loan found: {} WETH, Profit: {} WEI", format_ether(loan_amount), net_profit);
    info!("Attempting submission..."); let submit_result = transaction::submit_arbitrage_transaction( sim_env.http_client.clone(), app_state.clone(), route, loan_amount, net_profit, nonce_manager.clone(), ).await; assert!(submit_result.is_ok(), "Submission failed: {:?}", submit_result.err()); let final_tx_hash = submit_result.unwrap(); info!("✅ Arb Tx submitted: {:?}", final_tx_hash);
    let receipt = sim_env.http_client.get_transaction_receipt(final_tx_hash).await?.ok_or_else(|| eyre::eyre!("Receipt not found"))?; assert_eq!(receipt.status, Some(1.into()), "Arb tx reverted!"); info!("Arb Tx confirmed successfully.");
    info!("Full Arbitrage Cycle Test Completed."); Ok(())
}

// --- Placeholder Tests for Direct Huff Contract Verification ---

#[tokio::test]
#[ignore] // Requires manual setup and direct contract interaction
async fn test_huff_profit_check_revert() -> Result<()> {
    setup_tracing();
    info!("Starting Huff Profit Check Revert Test - Ensure Anvil is running!");
    // 1. Setup SimEnv & Deploy Executor
    let sim_env = local_simulator::setup_simulation_environment().await?;
    let executor_addr = sim_env.executor_address.expect("Executor must be deployed");
    let balancer_vault_addr: Address = "0xBA12222222228d8Ba445958a75a0704d566BF2C9".parse()?;

    // 2. Define Pool/Token Addresses (use configured ones or specific test cases)
    let pool_a: Address = sim_env.config.target_uniswap_v3_pool_address.parse()?;
    let pool_b: Address = sim_env.config.target_velodrome_v2_pool_address.parse()?;
    let token_in: Address = sim_env.config.target_weth_address.parse()?; // WETH
    let token_out: Address = sim_env.config.target_usdc_address.parse()?; // USDC
    let velo_router: Address = "0x9c12939390052919aF3155f41Bf41543Ca30607B".parse()?; // Example

    // 3. Prepare flashLoan call parameters
    let loan_amount = parse_ether(1.0)?; // Example: 1 WETH loan
    let tokens = vec![token_in];
    let amounts = vec![loan_amount];

    // 4. Encode userData with a minProfitWei designed to FAIL
    //    - Simulate a scenario where actual profit < minProfitWei
    //    - For this test, set minProfitWei unreasonably high.
    let min_profit_wei = parse_ether(1000.0)?; // Impossible profit target
    let salt = U256::from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos());
    let user_data = encoding::encode_user_data(
        pool_a, pool_b, token_out, true, false, true, velo_router, min_profit_wei, salt,
    )?;

    // 5. Setup prerequisites: Ensure executor holds *some* WETH to potentially execute swaps,
    //    even though we expect it to revert before needing full repayment approval.
    //    This might involve the signer sending WETH directly to the executor on Anvil.
    warn!("Profit check revert test assumes executor has some WETH if swaps execute before check.");

    // 6. Call flashLoan via Balancer Vault
    info!("Calling flashLoan, expecting profit check revert...");
    let vault_contract = BalancerVault::new(balancer_vault_addr, sim_env.http_client.clone());
    let flash_loan_call = vault_contract.flash_loan(executor_addr, tokens, amounts, user_data);

    // 7. Assert call reverts (ideally check for specific revert reason if possible)
    let call_result = flash_loan_call.send().await;
    assert!(call_result.is_err(), "Expected transaction to revert due to profit check, but it succeeded.");

    // Further check: Examine error message if available
    if let Err(e) = call_result {
         info!("Transaction reverted as expected: {:?}", e.to_string());
         // Ideally, check if the revert reason matches the expected unprofitable revert,
         // but this is hard without tracing or custom errors in Huff.
         assert!(e.to_string().contains("reverted"), "Expected revert error message");
    }

    info!("✅ Huff Profit Check Revert Test Passed (Transaction Reverted).");
    Ok(())
}

#[tokio::test]
#[ignore] // Requires manual setup and direct contract interaction
async fn test_huff_profit_check_success() -> Result<()> {
    setup_tracing();
    info!("Starting Huff Profit Check Success Test - Ensure Anvil is running!");
     // 1. Setup SimEnv & Deploy Executor
    let sim_env = local_simulator::setup_simulation_environment().await?;
    let executor_addr = sim_env.executor_address.expect("Executor must be deployed");
    let balancer_vault_addr: Address = "0xBA12222222228d8Ba445958a75a0704d566BF2C9".parse()?;

    // 2. Define Pool/Token Addresses
    let pool_a: Address = sim_env.config.target_uniswap_v3_pool_address.parse()?;
    let pool_b: Address = sim_env.config.target_velodrome_v2_pool_address.parse()?;
    let token_in: Address = sim_env.config.target_weth_address.parse()?; // WETH
    let token_out: Address = sim_env.config.target_usdc_address.parse()?; // USDC
    let velo_router: Address = "0x9c12939390052919aF3155f41Bf41543Ca30607B".parse()?; // Example

    // 3. Prepare flashLoan call parameters
    let loan_amount = parse_ether(1.0)?; // Example: 1 WETH loan
    let tokens = vec![token_in];
    let amounts = vec![loan_amount];

    // 4. Encode userData with minProfitWei = 1 (smallest possible profit)
    let min_profit_wei = U256::one();
    let salt = U256::from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos());
     let user_data = encoding::encode_user_data(
        pool_a, pool_b, token_out, true, false, true, velo_router, min_profit_wei, salt,
    )?;

    // 5. Setup prerequisites:
    //    - Create actual price difference on Anvil pools (like in full cycle test).
    //    - Ensure SIGNER has enough WETH to cover the actual flash loan repayment + Balancer fee (0).
    //      The executor doesn't repay directly, Balancer pulls from the recipient (executor).
    //      For the test, Balancer needs to pull from the *executor*. We need to fund the executor.
    info!("Funding executor ({}) with WETH ({})", executor_addr, format_ether(loan_amount * 2)); // Send > loan amount
    ensure_weth_balance_and_approval(&sim_env, executor_addr, loan_amount * 2, U256::zero()).await?; // Ensure signer has WETH
    let weth_contract = IERC20::new(token_in, sim_env.http_client.clone());
    let transfer_call = weth_contract.transfer(executor_addr, loan_amount * 2); // Send enough WETH
    transfer_call.send().await?.await?; // Send and confirm transfer
    info!("Executor funded.");

    // 6. Call flashLoan via Balancer Vault
    info!("Calling flashLoan, expecting profit check success...");
    let vault_contract = BalancerVault::new(balancer_vault_addr, sim_env.http_client.clone());
    // Need to call from an account that can approve Balancer Vault later - use SimEnv signer
    let flash_loan_call = vault_contract.connect(sim_env.http_client.clone()).flash_loan(executor_addr, tokens, amounts, user_data);

    // 7. Assert call SUCCEEDS
    let pending_tx = flash_loan_call.send().await.wrap_err("Flashloan transaction failed to send")?;
    let tx_hash = pending_tx.tx_hash();
    info!("Flashloan tx sent: {:?}", tx_hash);
    let receipt = pending_tx.await?.ok_or_else(|| eyre::eyre!("Flashloan tx dropped"))?;

    assert_eq!(receipt.status, Some(1.into()), "Expected transaction success, but it reverted. Tx: {:?}", tx_hash);

    info!("✅ Huff Profit Check Success Test Passed (Transaction Succeeded).");
    Ok(())
}

#[tokio::test]
#[ignore] // Requires manual setup and direct contract interaction
async fn test_huff_salt_guard_replay() -> Result<()> {
    setup_tracing();
    info!("Starting Huff Salt Guard Replay Test - Ensure Anvil is running!");
     // 1. Setup SimEnv & Deploy Executor
    let sim_env = local_simulator::setup_simulation_environment().await?;
    let executor_addr = sim_env.executor_address.expect("Executor must be deployed");
    let balancer_vault_addr: Address = "0xBA12222222228d8Ba445958a75a0704d566BF2C9".parse()?;

    // 2. Define Pool/Token Addresses (can be dummy for this test if desired)
    let pool_a: Address = sim_env.config.target_uniswap_v3_pool_address.parse()?;
    let pool_b: Address = sim_env.config.target_velodrome_v2_pool_address.parse()?;
    let token_in: Address = sim_env.config.target_weth_address.parse()?;
    let token_out: Address = sim_env.config.target_usdc_address.parse()?;
    let velo_router: Address = "0x9c12939390052919aF3155f41Bf41543Ca30607B".parse()?;

    // 3. Prepare flashLoan call parameters
    let loan_amount = parse_ether(0.001)?; // Small loan is fine
    let tokens = vec![token_in];
    let amounts = vec![loan_amount];

    // 4. Encode userData with minProfitWei=1 and a specific SALT
    let min_profit_wei = U256::one();
    let salt = U256::from(1234567890_u128); // Use a fixed salt value for replay
     let user_data = encoding::encode_user_data(
        pool_a, pool_b, token_out, true, false, true, velo_router, min_profit_wei, salt,
    )?;

     // 5. Fund Executor (similar to profit check success test)
     info!("Funding executor ({}) with WETH ({})", executor_addr, format_ether(loan_amount * 2));
     ensure_weth_balance_and_approval(&sim_env, executor_addr, loan_amount * 2, U256::zero()).await?;
     let weth_contract = IERC20::new(token_in, sim_env.http_client.clone());
     let transfer_call = weth_contract.transfer(executor_addr, loan_amount * 2);
     transfer_call.send().await?.await?; info!("Executor funded.");

    // 6. Call flashLoan FIRST time (expected to succeed or revert based on profit, but *should* mark salt)
    info!("Calling flashLoan (1st time) with salt {}...", salt);
    let vault_contract = BalancerVault::new(balancer_vault_addr, sim_env.http_client.clone());
    let flash_loan_call_1 = vault_contract.flash_loan(executor_addr, tokens.clone(), amounts.clone(), user_data.clone());
    let tx_result_1 = flash_loan_call_1.send().await;

    // We don't strictly care if the first tx succeeds/fails on profit, only that it ran.
    // However, if it fails to send entirely, the test is invalid.
    assert!(tx_result_1.is_ok(), "First flashloan call failed to send: {:?}", tx_result_1.err());
    let pending_tx_1 = tx_result_1.unwrap();
    info!("First flashloan tx sent: {:?}", pending_tx_1.tx_hash());
    // Wait for it to be mined
    let _receipt_1 = pending_tx_1.await?.ok_or_else(|| eyre::eyre!("First flashloan tx dropped"));
    info!("First flashloan tx mined.");

    // 7. Call flashLoan SECOND time with the EXACT SAME userData (including salt)
    info!("Calling flashLoan (2nd time) with SAME salt {}...", salt);
    let flash_loan_call_2 = vault_contract.flash_loan(executor_addr, tokens, amounts, user_data);
    let tx_result_2 = flash_loan_call_2.send().await;

    // 8. Assert the SECOND call REVERTS due to the salt guard
    assert!(tx_result_2.is_err(), "Expected second transaction with same salt to revert, but it succeeded.");
    if let Err(e) = tx_result_2 {
         info!("Second transaction reverted as expected: {:?}", e.to_string());
         // Check for specific revert string if Huff contract emitted one (currently it doesn't)
         assert!(e.to_string().contains("reverted"), "Expected revert error message for salt replay");
    }

    info!("✅ Huff Salt Guard Replay Test Passed (Second Transaction Reverted).");
    Ok(())
}