// bot/tests/integration_test.rs

// This test file requires the 'local_simulation' feature to be enabled.
// Run with: cargo test --features local_simulation -- --ignored
// Add other test functions and run without --ignored, or adjust flags as needed.

#![cfg(feature = "local_simulation")]
#![allow(unexpected_cfgs)] // Allow the cfg check warning within tests too

use ulp1_5::{
    bindings::{IERC20, UniswapV3Pool, VelodromeV2Pool},
    config::Config, // Need Config for AppState
    event_handler, // May need some functions if simulating event triggers
    local_simulator::{self, SimEnv},
    path_optimizer, // Need path_optimizer functions
    simulation,    // Need simulation functions
    state::{self, AppState, DexType}, // Need state items
    transaction::{self, NonceManager}, // Need transaction items
};
use ethers::{
    prelude::*,
    types::{Address, Bytes, Filter, I256, U256}, // Added Filter
    utils::{format_ether, parse_ether, parse_units}, // Added parse_units
};
use std::{str::FromStr, sync::Arc, time::Duration};
use tracing::{error, info, warn}; // Added error, warn
use tracing_subscriber::{filter::LevelFilter, fmt, EnvFilter};


// Helper to initialize tracing subscriber for tests
fn setup_tracing() {
    // Ensure tracing is only initialized once using a static flag or similar mechanism
    // For simplicity in this example, we rely on try_init's behavior.
    let _ = fmt()
        .with_max_level(LevelFilter::INFO) // Default to INFO level for tests
        .with_env_filter(EnvFilter::from_default_env()) // Allow overriding with RUST_LOG
        .with_target(true)
        .with_line_number(true)
        .with_test_writer() // Write to test output
        .try_init();
}


// Helper to ensure signer has enough WETH and has approved the spender
async fn ensure_weth_balance_and_approval(
    sim_env: &SimEnv,
    spender: Address,
    required_weth: U256,
    approve_amount: U256,
) -> eyre::Result<()> {
    // ... (implementation remains the same) ...
    info!(
        "Ensuring WETH balance >= {} and approval for spender {}",
        format_ether(required_weth),
        spender
    );
    let weth_addr: Address = sim_env.config.target_weth_address.parse()?;
    let weth_contract = IERC20::new(weth_addr, sim_env.http_client.clone());
    let signer_addr = sim_env.wallet_address;
    let balance = weth_contract.balance_of(signer_addr).call().await?;
    info!("Current WETH Balance: {}", format_ether(balance));
    if balance < required_weth {
        let eth_balance = sim_env.http_client.get_balance(signer_addr, None).await?;
        info!("Current ETH Balance: {}", format_ether(eth_balance));
        if eth_balance > required_weth {
            info!("Attempting to wrap ETH to get WETH...");
            let wrap_tx = TransactionRequest::new().to(weth_addr).value(required_weth);
            let pending_tx = sim_env.http_client.send_transaction(wrap_tx, None).await?;
            let receipt = pending_tx.await?.ok_or_else(|| eyre::eyre!("WETH wrap tx dropped"))?;
             if receipt.status != Some(1.into()) {
                 return Err(eyre::eyre!("WETH wrap transaction failed: {:?}", receipt.transaction_hash));
             }
             info!("Wrapped ETH successfully. Tx: {:?}", receipt.transaction_hash);
        } else {
             return Err(eyre::eyre!(
                "Insufficient ETH balance ({}) on Anvil signer {} to wrap required WETH ({})",
                format_ether(eth_balance), signer_addr, format_ether(required_weth)
            ));
        }
        let new_balance = weth_contract.balance_of(signer_addr).call().await?;
        if new_balance < required_weth {
             return Err(eyre::eyre!("Still insufficient WETH balance after wrapping attempt"));
        }
    }
    let allowance = weth_contract.allowance(signer_addr, spender).call().await?;
    info!("Current WETH allowance for {}: {}", spender, format_ether(allowance));
    if allowance < approve_amount {
        info!("Approving WETH for spender {}...", spender);
        let approve_call = weth_contract.approve(spender, approve_amount);
        let pending_tx = approve_call.send().await?;
        let receipt = pending_tx.await?.ok_or_else(|| eyre::eyre!("WETH approval tx dropped"))?;
        if receipt.status != Some(1.into()) {
            return Err(eyre::eyre!("WETH approval transaction failed: {:?}", receipt.transaction_hash));
        }
        info!("WETH approved successfully. Tx: {:?}", receipt.transaction_hash);
    }
    Ok(())
}


#[tokio::test]
#[ignore] // Ignore by default, requires manual Anvil setup.
async fn test_anvil_simulation_setup_and_fetch() {
    // ... (test remains the same) ...
    setup_tracing();
    info!("############# IMPORTANT #############");
    info!("Ensure Anvil is running externally and matches SIMULATION_CONFIG in local_simulator.rs:");
    info!("HTTP: http://127.0.0.1:8545"); info!("WS:   ws://127.0.0.1:8545");
    info!("Forking target network (e.g., Optimism) with relevant contracts deployed.");
    info!("#####################################");
    tokio::time::sleep(Duration::from_secs(3)).await;
    let setup_result = local_simulator::setup_simulation_environment().await;
    assert!(setup_result.is_ok(), "Setup failed: {:?}. Anvil running?", setup_result.err());
    let sim_env = setup_result.unwrap(); info!("Setup Complete.");
    info!("Wallet Address: {:?}", sim_env.wallet_address); info!("Executor Address: {:?}", sim_env.executor_address);
    if sim_env.config.deploy_executor_in_sim {
         assert!(sim_env.executor_address.is_some()); assert_ne!(sim_env.executor_address.unwrap(), Address::zero());
    }
    let fetch_result = local_simulator::fetch_simulation_data(&sim_env).await;
    assert!(fetch_result.is_ok(), "Fetch failed: {:?}", fetch_result.err()); info!("Fetch successful.");
}

#[tokio::test]
#[ignore] // Ignore by default, requires Anvil + interaction simulation.
async fn test_anvil_v3_swap_trigger() {
    // ... (test remains the same) ...
     setup_tracing(); info!("Starting V3 Swap Trigger Test - Ensure Anvil is running!");
     tokio::time::sleep(Duration::from_secs(1)).await;
     let sim_env = match local_simulator::setup_simulation_environment().await {
         Ok(env) => Arc::new(env), Err(e) => panic!("Setup failed: {:?}", e),
     };
     let pool_addr: Address = match sim_env.config.target_uniswap_v3_pool_address.parse() {
         Ok(addr) if addr != Address::zero() => addr,
         _ => { info!("Skipping V3 swap: Target pool not configured."); return; }
     };
     let pool_binding = UniswapV3Pool::new(pool_addr, sim_env.http_client.clone());
     let recipient = sim_env.wallet_address; let zero_for_one = true;
     let amount_weth = 0.01; let amount_in_wei = parse_ether(amount_weth).unwrap();
     info!("Checking/Setting prerequisites...");
     let prereq_result = ensure_weth_balance_and_approval(&sim_env, pool_addr, amount_in_wei, U256::MAX).await;
     assert!(prereq_result.is_ok(), "Prereq failed: {:?}", prereq_result.err()); info!("Prerequisites met.");
     info!("Attempting V3 swap trigger on pool: {}", pool_addr);
     let swap_result = local_simulator::trigger_v3_swap(&sim_env, pool_addr, &pool_binding, recipient, zero_for_one, I256::from_raw(amount_in_wei), U256::zero(), Bytes::new()).await;
     assert!(swap_result.is_ok(), "Swap trigger failed: {:?}", swap_result.err());
     info!("V3 Swap transaction sent: {:?}", swap_result.unwrap());
}


#[tokio::test]
#[ignore] // Ignore by default, requires Anvil + interaction simulation.
async fn test_anvil_v2_swap_trigger() {
    // ... (test remains the same) ...
    setup_tracing(); info!("Starting V2 Swap Trigger Test - Ensure Anvil is running!");
    tokio::time::sleep(Duration::from_secs(1)).await;
    let sim_env = match local_simulator::setup_simulation_environment().await {
        Ok(env) => Arc::new(env), Err(e) => panic!("Setup failed: {:?}", e),
    };
    let pool_addr: Address = match sim_env.config.target_velodrome_v2_pool_address.parse() {
       Ok(addr) if addr != Address::zero() => addr,
       _ => { info!("Skipping V2 swap: Target pool not configured."); return; }
    };
    let pool_binding = VelodromeV2Pool::new(pool_addr, sim_env.http_client.clone());
    let amount_out_usdc_wei = U256::from(10_000_000);
    let amount0_out = U256::zero(); let amount1_out = amount_out_usdc_wei;
    let recipient = sim_env.wallet_address;
    info!("Prerequisites assumed met (pool has liquidity).");
    info!("Attempting V2 swap trigger on pool: {}", pool_addr);
    let swap_result = local_simulator::trigger_v2_swap(&sim_env, pool_addr, &pool_binding, amount0_out, amount1_out, recipient, Bytes::new()).await;
    assert!(swap_result.is_ok(), "Swap trigger failed: {:?}", swap_result.err());
    info!("V2 Swap transaction sent: {:?}", swap_result.unwrap());
}


#[tokio::test]
#[ignore] // Most complex test - requires careful setup and simulation
async fn test_full_arbitrage_cycle() -> eyre::Result<()> {
    setup_tracing();
    info!("Starting Full Arbitrage Cycle Test - Ensure Anvil is running!");
    tokio::time::sleep(Duration::from_secs(1)).await;

    // --- Test Setup ---
    let sim_env = match local_simulator::setup_simulation_environment().await {
         Ok(env) => Arc::new(env),
         Err(e) => panic!("Failed to setup simulation environment: {:?}", e),
    };
    let executor_addr = sim_env.executor_address.expect("Executor address needed for full cycle test");

    // Create a dummy Config based on SimEnv (adjust as needed)
    // Ideally, load from a test .env file or construct manually
    let test_config = Config {
        ws_rpc_url: sim_env.config.anvil_ws_url.to_string(),
        http_rpc_url: sim_env.config.anvil_http_url.to_string(),
        local_private_key: sim_env.config.anvil_private_key.to_string(),
        chain_id: Some(sim_env.http_client.get_chainid().await?.as_u64()),
        arb_executor_address: sim_env.executor_address,
        uniswap_v3_factory_addr: Address::from_str("0x1F98431c8aD98523631AE4a59f267346ea31F984").unwrap(), // Example OP Address
        velodrome_v2_factory_addr: Address::from_str("0x25CbdDb98b35AB1FF795324516342Fac4845718f").unwrap(), // Example OP Address
        balancer_vault_address: Address::from_str("0xBA12222222228d8Ba445958a75a0704d566BF2C9").unwrap(),
        quoter_v2_address: Address::from_str("0xbC52C688c34A4F6180437B40593F1F9638C2571d").unwrap(), // Example OP Address
        velo_router_addr: Address::from_str("0x9c12939390052919aF3155f41Bf41543Ca30607B").unwrap(), // Example OP Address (VERIFY)
        aerodrome_factory_addr: None, // Not testing Aero in this cycle
        aerodrome_router_addr: None,
        weth_address: sim_env.config.target_weth_address.parse()?,
        usdc_address: sim_env.config.target_usdc_address.parse()?,
        weth_decimals: 18,
        usdc_decimals: 6,
        deploy_executor: false, // Already deployed in SimEnv setup
        executor_bytecode_path: "".to_string(),
        min_loan_amount_weth: 0.1,
        max_loan_amount_weth: 10.0, // Keep test loans small
        optimal_loan_search_iterations: 5, // Fewer iterations for testing
        fetch_timeout_secs: Some(10),
        enable_univ3_dynamic_sizing: false,
        max_priority_fee_per_gas_gwei: 0.01,
        fallback_gas_price_gwei: Some(0.01),
        gas_limit_buffer_percentage: 50, // Higher buffer for testing robustness
        min_flashloan_gas_limit: 500_000, // Reasonable minimum
        private_rpc_url: None, // Use public Anvil RPC
        secondary_private_rpc_url: None,
        min_profit_buffer_bps: 20, // 0.2% buffer
        min_profit_abs_buffer_wei_str: "100000000000000".to_string(), // 0.0001 ETH
    };

    let app_state = Arc::new(AppState::new(test_config.clone()));
    let nonce_manager = Arc::new(NonceManager::new(sim_env.wallet_address));

    // --- Fetch Initial State for Target Pools ---
    info!("Fetching initial state for target pools...");
    let v3_pool_addr: Address = sim_env.config.target_uniswap_v3_pool_address.parse()?;
    let v2_pool_addr: Address = sim_env.config.target_velodrome_v2_pool_address.parse()?;
    let v3_factory_addr = app_state.config.uniswap_v3_factory_addr;
    let v2_factory_addr = app_state.config.velodrome_v2_factory_addr;

    state::fetch_and_cache_pool_state(v3_pool_addr, DexType::UniswapV3, v3_factory_addr, sim_env.http_client.clone(), app_state.clone()).await?;
    state::fetch_and_cache_pool_state(v2_pool_addr, DexType::VelodromeV2, v2_factory_addr, sim_env.http_client.clone(), app_state.clone()).await?;
    info!("Initial states fetched. V3 Pool: {}, V2 Pool: {}", app_state.pool_states.contains_key(&v3_pool_addr), app_state.pool_states.contains_key(&v2_pool_addr));
    assert!(app_state.pool_states.contains_key(&v3_pool_addr));
    assert!(app_state.pool_states.contains_key(&v2_pool_addr));


    // --- Create Arbitrage Opportunity ---
    info!("Manually creating price discrepancy between pools...");
    let v3_pool = UniswapV3Pool::new(v3_pool_addr, sim_env.http_client.clone());
    let swap1_amount_weth = 0.5; // Larger swap to create noticeable difference
    let swap1_amount = parse_ether(swap1_amount_weth)?;
    ensure_weth_balance_and_approval(&sim_env, v3_pool_addr, swap1_amount, U256::MAX).await?;
    let swap_tx_hash = local_simulator::trigger_v3_swap(&sim_env, v3_pool_addr, &v3_pool, sim_env.wallet_address, true, I256::from_raw(swap1_amount), U256::zero(), Bytes::new()).await?;
    info!("Sent initial swap on V3 pool: {:?}", swap_tx_hash);
    // Wait for swap to likely be included
    tokio::time::sleep(Duration::from_secs(2)).await;


    // --- Simulate Bot Logic Sequentially ---
    info!("Simulating bot's arbitrage check (sequential)...");

    // 1. Manually update snapshot (or re-fetch) - Re-fetching is simpler here
    state::fetch_and_cache_pool_state(v3_pool_addr, DexType::UniswapV3, v3_factory_addr, sim_env.http_client.clone(), app_state.clone()).await?;
    let updated_snapshot = app_state.pool_snapshots.get(&v3_pool_addr).expect("Updated snapshot missing").clone(); // Clone value

    // 2. Find routes
    let routes = path_optimizer::find_top_routes(
        &updated_snapshot,
        &app_state.pool_states,
        &app_state.pool_snapshots,
        &app_state.config,
        app_state.weth_address, app_state.usdc_address,
        app_state.weth_decimals, app_state.usdc_decimals,
    );
    assert!(!routes.is_empty(), "No arbitrage routes found after creating price discrepancy!");
    info!("Found {} potential routes. Evaluating top one.", routes.len());
    let route = routes.first().unwrap().clone(); // Evaluate the best route

    // 3. Find Optimal Loan
    let gas_info = transaction::fetch_gas_price(sim_env.http_client.clone(), &app_state.config).await?;
    let gas_price_gwei = gas_info.max_priority_fee_per_gas.to_f64_lossy() / 1e9;
    let buy_snap = app_state.pool_snapshots.get(&route.buy_pool_addr).map(|r| r.clone());
    let sell_snap = app_state.pool_snapshots.get(&route.sell_pool_addr).map(|r| r.clone());

    let optimal_result = simulation::find_optimal_loan_amount(
        sim_env.http_client.clone(),
        app_state.clone(),
        &route,
        buy_snap.as_ref(), // Pass Option<&PoolSnapshot>
        sell_snap.as_ref(),
        gas_price_gwei,
    ).await?;

    assert!(optimal_result.is_some(), "Expected to find a profitable loan amount, but found None.");
    let (loan_amount, net_profit) = optimal_result.unwrap();
    assert!(net_profit > I256::zero(), "Expected positive net profit, found {}", net_profit);
    info!("Optimal loan found: {} WETH, Profit: {} WEI", format_ether(loan_amount), net_profit);

    // 4. Submit Transaction
    info!("Attempting to submit arbitrage transaction...");
    let submit_result = transaction::submit_arbitrage_transaction(
        sim_env.http_client.clone(),
        app_state.clone(),
        route,
        loan_amount,
        net_profit,
        nonce_manager.clone(),
    ).await;

    assert!(submit_result.is_ok(), "Arbitrage transaction submission failed: {:?}", submit_result.err());
    let final_tx_hash = submit_result.unwrap();
    info!("✅ Arbitrage transaction submitted successfully: {:?}", final_tx_hash);

    // --- Assertions ---
    // Check Anvil logs/state manually or add receipt check here.
    let receipt = sim_env.http_client.get_transaction_receipt(final_tx_hash).await?
        .ok_or_else(|| eyre::eyre!("Receipt not found for submitted arb tx"))?;

    assert_eq!(receipt.status, Some(1.into()), "Arbitrage transaction reverted on Anvil!");
    info!("Arbitrage transaction confirmed successfully on Anvil.");


    info!("Full Arbitrage Cycle Test Completed.");
    Ok(())
}