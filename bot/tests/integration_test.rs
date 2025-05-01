// bot/tests/integration_test.rs

// This test file requires the 'local_simulation' feature to be enabled.
// Run with: cargo test --features local_simulation -- --ignored
// Add other test functions and run without --ignored, or adjust flags as needed.

#![cfg(feature = "local_simulation")]
#![allow(unexpected_cfgs)] // Allow the cfg check warning within tests too

use ulp1_5::{
    bindings::{IERC20, UniswapV3Pool, VelodromeV2Pool}, // Add necessary bindings
    local_simulator::{self, SimEnv}, // Assuming ulp1_5 is the crate name
};
use ethers::{
    prelude::*, // Includes Middleware, SignerMiddleware, Provider, Http, Ws etc.
    types::{Address, Bytes, I256, U256},
    utils::{parse_ether, format_ether}, // Add parse_ether, format_ether
};
use std::{str::FromStr, sync::Arc, time::Duration}; // Add FromStr
use tracing::info;
use tracing_subscriber::{filter::LevelFilter, fmt, EnvFilter};

// Helper to initialize tracing subscriber for tests
fn setup_tracing() {
    let _ = fmt()
        .with_max_level(LevelFilter::INFO) // Default to INFO level for tests
        .with_env_filter(EnvFilter::from_default_env()) // Allow overriding with RUST_LOG
        .with_target(true)
        .with_line_number(true)
        .try_init(); // Use try_init to avoid panic if already initialized
}

// Helper to ensure signer has enough WETH and has approved the spender
// NOTE: This interacts directly with Anvil state.
async fn ensure_weth_balance_and_approval(
    sim_env: &SimEnv,
    spender: Address, // The address that needs approval (e.g., pool or router)
    required_weth: U256, // Minimum WETH balance needed (in wei)
    approve_amount: U256, // Amount to approve (e.g., U256::MAX)
) -> eyre::Result<()> {
    info!(
        "Ensuring WETH balance >= {} and approval for spender {}",
        format_ether(required_weth),
        spender
    );
    let weth_addr: Address = sim_env.config.target_weth_address.parse()?;
    let weth_contract = IERC20::new(weth_addr, sim_env.http_client.clone());
    let signer_addr = sim_env.wallet_address;

    // 1. Check Balance
    let balance = weth_contract.balance_of(signer_addr).call().await?;
    info!("Current WETH Balance: {}", format_ether(balance));
    if balance < required_weth {
        // This part is tricky - minting/getting WETH usually involves wrapping ETH.
        // For simulation, we might need to use Anvil cheat codes or assume balance exists.
        // Or, if the Anvil account has ETH, we can wrap it.
        let eth_balance = sim_env.http_client.get_balance(signer_addr, None).await?;
        info!("Current ETH Balance: {}", format_ether(eth_balance));
        if eth_balance > required_weth { // Check ETH balance as proxy
            info!("Attempting to wrap ETH to get WETH...");
            // WETH contract usually has a deposit function payable with ETH
            let wrap_tx = TransactionRequest::new()
                .to(weth_addr)
                .value(required_weth); // Wrap the required amount
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
        // Re-check balance after wrapping attempt
        let new_balance = weth_contract.balance_of(signer_addr).call().await?;
        if new_balance < required_weth {
             return Err(eyre::eyre!("Still insufficient WETH balance after wrapping attempt"));
        }
    }

    // 2. Check Allowance
    let allowance = weth_contract.allowance(signer_addr, spender).call().await?;
    info!("Current WETH allowance for {}: {}", spender, format_ether(allowance));
    if allowance < approve_amount { // Check if allowance is less than desired
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
    // ... (test remains the same as previous version) ...
    setup_tracing();
    info!("############# IMPORTANT #############");
    info!("Ensure Anvil is running externally and matches SIMULATION_CONFIG in local_simulator.rs:");
    info!("HTTP: http://127.0.0.1:8545");
    info!("WS:   ws://127.0.0.1:8545");
    info!("Forking target network (e.g., Optimism) with relevant contracts deployed.");
    info!("#####################################");
    tokio::time::sleep(Duration::from_secs(3)).await;
    let setup_result = local_simulator::setup_simulation_environment().await;
    assert!(setup_result.is_ok(), "Failed to setup simulation environment: {:?}. Is Anvil running?", setup_result.err());
    let sim_env = setup_result.unwrap();
    info!("Simulation Environment Setup Complete.");
    info!("Wallet Address: {:?}", sim_env.wallet_address);
    info!("Executor Address: {:?}", sim_env.executor_address);
    if sim_env.config.deploy_executor_in_sim {
         assert!(sim_env.executor_address.is_some(), "Executor should have been deployed but address is None.");
         assert_ne!(sim_env.executor_address.unwrap(), Address::zero(), "Deployed executor address should not be zero.");
    }
    let fetch_result = local_simulator::fetch_simulation_data(&sim_env).await;
    assert!(fetch_result.is_ok(), "Failed to fetch simulation data from Anvil: {:?}", fetch_result.err());
    info!("Successfully fetched initial simulation data from Anvil.");
}

#[tokio::test]
#[ignore] // Ignore by default, requires Anvil + interaction simulation.
async fn test_anvil_v3_swap_trigger() {
    setup_tracing();
    info!("Starting V3 Swap Trigger Test - Ensure Anvil is running!");
    tokio::time::sleep(Duration::from_secs(1)).await;

    // --- Test Setup ---
    let sim_env = match local_simulator::setup_simulation_environment().await {
         Ok(env) => Arc::new(env),
         Err(e) => panic!("Failed to setup simulation environment: {:?}", e),
    };

    let pool_addr: Address = match sim_env.config.target_uniswap_v3_pool_address.parse() {
        Ok(addr) if addr != Address::zero() => addr,
        _ => {
            info!("Skipping V3 swap trigger test: Target pool address not configured or is zero.");
            return;
        }
    };

    // Create contract binding
    let pool_binding = UniswapV3Pool::new(pool_addr, sim_env.http_client.clone());

    // Define swap parameters (example: swap 0.01 WETH for USDC)
    let recipient = sim_env.wallet_address;
    let zero_for_one = true; // WETH -> USDC (assuming WETH is token0)
    let amount_weth = 0.01;
    let amount_in_wei = parse_ether(amount_weth).expect("Failed to parse WETH amount");

     // --- Prerequisites ---
     // Ensure signer has WETH and has approved the pool
    info!("Checking/Setting prerequisites for V3 swap...");
    let prereq_result = ensure_weth_balance_and_approval(
        &sim_env,
        pool_addr, // Spender is the pool itself for UniV3 swap
        amount_in_wei, // Need at least this much WETH
        U256::MAX, // Approve max
    ).await;
    assert!(prereq_result.is_ok(), "Failed to meet prerequisites for V3 swap: {:?}", prereq_result.err());
    info!("Prerequisites met.");


    // --- Action ---
    info!("Attempting to trigger V3 swap on pool: {}", pool_addr);
    let swap_result = local_simulator::trigger_v3_swap(
        &sim_env,
        pool_addr,
        &pool_binding,
        recipient,
        zero_for_one,
        I256::from_raw(amount_in_wei), // Positive amount = exact input
        U256::zero(), // No price limit for testing
        Bytes::new(), // No callback data
    ).await;

    // --- Assertions ---
    assert!(swap_result.is_ok(), "Failed to send V3 swap transaction: {:?}", swap_result.err());
    let tx_hash = swap_result.unwrap();
    info!("V3 Swap transaction sent successfully: {:?}", tx_hash);

    // Optional: Wait for receipt
    // ...
}


#[tokio::test]
#[ignore] // Ignore by default, requires Anvil + interaction simulation.
async fn test_anvil_v2_swap_trigger() {
    setup_tracing();
    info!("Starting V2 Swap Trigger Test - Ensure Anvil is running!");
    tokio::time::sleep(Duration::from_secs(1)).await;

    // --- Test Setup ---
    let sim_env = match local_simulator::setup_simulation_environment().await {
         Ok(env) => Arc::new(env),
         Err(e) => panic!("Failed to setup simulation environment: {:?}", e),
    };

    let pool_addr: Address = match sim_env.config.target_velodrome_v2_pool_address.parse() {
        Ok(addr) if addr != Address::zero() => addr,
        _ => {
            info!("Skipping V2 swap trigger test: Target pool address not configured or is zero.");
            return;
        }
    };

    // Create contract binding (using VeloV2Pool, assuming similar interface for Aero if needed)
    let pool_binding = VelodromeV2Pool::new(pool_addr, sim_env.http_client.clone());

    // Define swap parameters (example: swap 0.01 WETH for USDC)
    // V2 swaps often specify output amount desired OR input amount to send.
    // The local_simulator::trigger_v2_swap expects *output* amounts.
    // To trigger based on INPUT, we'd need to call approve + transferFrom or use a Router.
    // For this test, we'll simulate receiving tokens by setting output amounts.
    let amount_out_usdc_wei = U256::from(10_000_000); // Example: Target 10 USDC output (6 decimals)
    let amount0_out = U256::zero(); // Assuming token0 is WETH, we want 0 WETH out
    let amount1_out = amount_out_usdc_wei; // Assuming token1 is USDC
    let recipient = sim_env.wallet_address;

    // --- Prerequisites ---
    // For V2 swap where we specify output, the pool needs sufficient liquidity.
    // The signer also needs to *receive* the output tokens. No approval needed *by the signer*.
    info!("Checking prerequisites for V2 swap (pool liquidity)...");
    // TODO: Add checks for pool reserves if necessary for the test scenario.
    info!("Prerequisites assumed met (pool has liquidity).");


    // --- Action ---
    info!("Attempting to trigger V2 swap on pool: {}", pool_addr);
    let swap_result = local_simulator::trigger_v2_swap(
        &sim_env,
        pool_addr,
        &pool_binding,
        amount0_out,
        amount1_out,
        recipient,
        Bytes::new(), // No callback data
    ).await;

    // --- Assertions ---
    assert!(swap_result.is_ok(), "Failed to send V2 swap transaction: {:?}", swap_result.err());
    let tx_hash = swap_result.unwrap();
    info!("V2 Swap transaction sent successfully: {:?}", tx_hash);

    // Optional: Wait for receipt
    // ...
}

/*
#[tokio::test]
#[ignore] // Most complex test - requires careful setup and simulation
async fn test_full_arbitrage_cycle() {
    setup_tracing();
    info!("Starting Full Arbitrage Cycle Test - Ensure Anvil is running!");
    tokio::time::sleep(Duration::from_secs(1)).await;

    // --- Test Setup ---
    let sim_env = match local_simulator::setup_simulation_environment().await {
         Ok(env) => Arc::new(env),
         Err(e) => panic!("Failed to setup simulation environment: {:?}", e),
    };
    // Ensure executor was deployed or configured
    let executor_addr = sim_env.executor_address.expect("Executor address needed for full cycle test");

    // Get pool addresses and bindings
    let v3_pool_addr: Address = sim_env.config.target_uniswap_v3_pool_address.parse().unwrap();
    let v2_pool_addr: Address = sim_env.config.target_velodrome_v2_pool_address.parse().unwrap();
    let v3_pool = UniswapV3Pool::new(v3_pool_addr, sim_env.http_client.clone());
    let v2_pool = VelodromeV2Pool::new(v2_pool_addr, sim_env.http_client.clone());

    // --- Create Arbitrage Opportunity ---
    info!("Manually creating price discrepancy between pools...");
    // Example: Swap WETH->USDC on V3 pool, pushing its WETH price down
    let swap1_amount = parse_ether(0.1).unwrap();
    ensure_weth_balance_and_approval(&sim_env, v3_pool_addr, swap1_amount, U256::MAX).await.unwrap();
    local_simulator::trigger_v3_swap(&sim_env, v3_pool_addr, &v3_pool, sim_env.wallet_address, true, I256::from_raw(swap1_amount), U256::zero(), Bytes::new()).await.unwrap();
    info!("Sent initial swap on V3 pool");
    // Example: Swap USDC->WETH on V2 pool, pushing its WETH price up
    // (Requires Signer having USDC and approving V2 pool, more complex setup)
    // For now, assume price discrepancy exists or skip second swap trigger.
    warn!("Skipping second swap trigger for V2 pool due to setup complexity. Assuming price discrepancy exists.");
    tokio::time::sleep(Duration::from_secs(2)).await; // Allow state propagation if needed

    // --- Simulate Bot Logic ---
    // Ideally, we'd run the bot's main loop against the WS connection from sim_env.
    // As a simpler alternative, manually call the core logic functions.
    info!("Simulating bot's arbitrage check...");
    // 1. Need to construct AppState pointing to Anvil client/config
    //    (This might require refactoring AppState::new or creating a test version)
    // 2. Fetch initial states into test AppState using fetch_and_cache_pool_state
    // 3. Simulate receiving the swap event log from the V3 swap above.
    // 4. Call event_handler::check_for_arbitrage with the test AppState, sim_env client, etc.
    warn!("Simulating bot core logic directly is complex and NOT IMPLEMENTED YET.");
    info!("Test Outcome: Placeholder - check Anvil logs manually for triggered swaps.");


    // --- Assertions (Placeholder) ---
    // Assert based on expected outcome:
    // - If arb expected: Check if a flashloan tx was sent to Balancer Vault on Anvil.
    // - If no arb expected: Check that NO flashloan tx was sent.
    // - Use `cast run <TX_HASH> --debug` manually on Anvil to verify Huff execution if a tx was sent.

}
*/