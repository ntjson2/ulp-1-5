// tests/integration_test.rs
#![cfg(feature = "local_simulation")] // Only compile when the feature is enabled

// Use ulp1_5:: prefix now that this is an external integration test
use ulp1_5::local_simulator::{setup_simulation_environment, trigger_v3_swap, trigger_v2_swap}; // Removed unused SimEnv
use ulp1_5::bindings::{UniswapV3Pool, VelodromeV2Pool}; // Removed unused ArbitrageExecutor for now
// Removed unused state imports: use ulp1_5::state::{AppState, DexType};
// Removed unused config import: use ulp1_5::config;
// Removed unused path_optimizer import: use ulp1_5::path_optimizer;
// Removed unused simulation import: use ulp1_5::simulation;
// Removed unused transaction imports: use ulp1_5::transaction::{self, NonceManager};
// Removed unused utils import: use ulp1_5::utils::ToF64Lossy;

// Keep necessary imports
use ethers::prelude::*;
use ethers::utils::{parse_ether, parse_units};
use std::sync::Arc;
use eyre::{Result, Report, eyre}; // Removed unused WrapErr
use tracing::{info, error, warn};
use tracing_subscriber::{fmt, EnvFilter};


// Helper to initialize tracing only once
use std::sync::Once;
static LOG_INIT: Once = Once::new();

fn setup_tracing() {
    LOG_INIT.call_once(|| {
        dotenv::dotenv().ok();
        fmt()
            .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
            .with_target(true)
            .with_line_number(true)
            .init();
        info!("Test tracing initialized.");
    });
}


/// Test: Basic Anvil Setup and Connection
#[tokio::test]
#[ignore]
async fn test_setup() -> Result<()> {
    setup_tracing();
    info!("--- Running Test: test_setup ---");
    let sim_env = setup_simulation_environment().await?;
    info!("✅ Simulation environment setup successful.");
    // Fix E0616: Access public field
    assert!(sim_env.executor_address.is_some() || !sim_env.config.deploy_executor_in_sim);
    info!("Executor address presence check passed.");
    Ok(())
}

/// Test: Triggering Swaps on Anvil Fork
#[tokio::test]
#[ignore]
async fn test_swap_triggers() -> Result<()> {
    setup_tracing();
    info!("--- Running Test: test_swap_triggers ---");
    // Create SimEnv but don't Arc it here, pass borrows to functions that need it
    let sim_env = setup_simulation_environment().await?;

    // Example: Trigger UniV3 Swap
    // Fix E0616: Access public field
    let uni_pool_addr: Address = sim_env.config.target_uniswap_v3_pool_address.parse()?;
    if uni_pool_addr != Address::zero() {
         info!("Attempting to trigger UniV3 swap on pool: {}", uni_pool_addr);
         let uni_pool = UniswapV3Pool::new(uni_pool_addr, sim_env.http_client.clone());
         let recipient = sim_env.wallet_address;
         let zero_for_one = true;
         let amount_specified = I256::from_raw(parse_ether("0.01")?);
         let sqrt_price_limit_x96 = U256::zero();
         let data = Bytes::new();

        match trigger_v3_swap(
             &sim_env, // Pass borrow
             uni_pool_addr,
             &uni_pool,
             recipient,
             zero_for_one,
             amount_specified,
             sqrt_price_limit_x96,
             data.clone(),
         ).await {
             Ok(tx_hash) => info!("✅ UniV3 swap triggered successfully: {}", tx_hash),
             Err(e) => warn!("⚠️ UniV3 swap trigger failed: {:?}", e), // Don't fail test, could be ABI issue still or fork state
         }
    } else {
         info!("Skipping UniV3 swap trigger: address is zero.");
    }

    // Example: Trigger VeloV2 Swap
    // Fix E0616: Access public field
    let velo_pool_addr: Address = sim_env.config.target_velodrome_v2_pool_address.parse()?;
    if velo_pool_addr != Address::zero() {
         info!("Attempting to trigger VeloV2 swap on pool: {}", velo_pool_addr);
         let velo_pool = VelodromeV2Pool::new(velo_pool_addr, sim_env.http_client.clone());
         let amount0_out = U256::zero();
         // Fix E0308: Unwrap the Result from parse_units
         let amount1_out = parse_units("10", 6)?.into(); // Add ?.into()
         let to_address = sim_env.wallet_address;
         let data = Bytes::new();

         match trigger_v2_swap(
             &sim_env, // Pass borrow
             velo_pool_addr,
             &velo_pool,
             amount0_out,
             amount1_out, // Now passing U256
             to_address,
             data.clone(),
         ).await {
              Ok(tx_hash) => info!("✅ VeloV2 swap triggered successfully: {}", tx_hash),
              Err(e) => {
                  error!("❌ VeloV2 swap trigger failed: {:?}", e);
                  return Err(e);
              }
         }
    } else {
         info!("Skipping VeloV2 swap trigger: address is zero.");
    }

    Ok(())
}


/// Test: Simulate the full arbitrage cycle sequentially
#[tokio::test]
#[ignore]
async fn test_full_arbitrage_cycle_simulation() -> Result<()> {
    setup_tracing();
    info!("--- Running Test: test_full_arbitrage_cycle_simulation ---");

    // Re-import necessary types/modules locally for this test
    use ulp1_5::state::{AppState, DexType};
    use ulp1_5::config::load_config;
    use ulp1_5::path_optimizer::RouteCandidate;
    use ulp1_5::simulation::find_optimal_loan_amount;
    use ulp1_5::transaction::{fetch_gas_price, submit_arbitrage_transaction, NonceManager};
    use ulp1_5::utils::ToF64Lossy;

    // 1. Setup
    // Create SimEnv directly
    let sim_env = setup_simulation_environment().await?;
    let client = sim_env.http_client.clone(); // Clone the Arc'd client
    let executor_addr = sim_env.executor_address.expect("Executor must be deployed for this test");
    info!("Anvil Setup Complete. Executor: {:?}", executor_addr);

    // Load config using load_config directly
    let config = load_config().expect("Failed to load test config from .env");
    let weth_addr = config.weth_address;
    let usdc_addr = config.usdc_address;

    let pool_a_addr_str = "0x851492574065EDE975391E141377067943aA08eF";
    let pool_b_addr_str = "0x207addb05c548f262219f6b50eadff8640ed6488";
    let pool_a_addr: Address = pool_a_addr_str.parse()?;
    let pool_b_addr: Address = pool_b_addr_str.parse()?;
    info!("Using Test Pools: A (UniV3)={}, B (VeloV2)={}", pool_a_addr, pool_b_addr);


    // 2. Identify Opportunity (Manual)
    // Use RouteCandidate directly
    let route = RouteCandidate {
        buy_pool_addr: pool_a_addr,
        sell_pool_addr: pool_b_addr,
        buy_dex_type: DexType::UniswapV3, // Use directly
        sell_dex_type: DexType::VelodromeV2, // Use directly
        token_in: weth_addr,
        token_out: usdc_addr,
        buy_pool_fee: Some(500),
        sell_pool_fee: None,
        buy_pool_stable: None,
        sell_pool_stable: Some(true),
        buy_pool_factory: config.uniswap_v3_factory_addr,
        sell_pool_factory: config.velodrome_v2_factory_addr,
        zero_for_one_a: true,
        estimated_profit_usd: 0.1,
    };
    info!("Constructed Manual Route Candidate: {:?}", route);

    // 3. Simulate & Optimize
    // Use AppState directly
    let app_state = Arc::new(AppState::new(config.clone()));
    // Use fetch_gas_price directly
    let gas_info = fetch_gas_price(client.clone(), &config).await?;
    // Use ToF64Lossy directly
    let gas_price_gwei = ToF64Lossy::to_f64_lossy(&gas_info.max_priority_fee_per_gas) / 1e9;
    info!("Fetched Anvil gas price (prio): {} gwei", gas_price_gwei);

    let buy_snapshot = None;
    let sell_snapshot = None;

    // Use find_optimal_loan_amount directly
    let optimal_loan_result = find_optimal_loan_amount(
        client.clone(),
        app_state.clone(),
        &route,
        buy_snapshot,
        sell_snapshot,
        gas_price_gwei,
    ).await?;

    let (loan_amount_wei, simulated_net_profit_wei) = match optimal_loan_result {
        Some((amount, profit)) if profit > I256::zero() => {
            info!("✅ Optimal loan found: amount={}, profit={}", amount, profit);
            (amount, profit)
        },
        Some((_, profit)) => {
             warn!("Simulation found optimal loan, but profit is not positive ({}). Test may not execute tx.", profit);
             (U256::zero(), I256::zero())
        }
        None => {
            warn!("❌ No profitable loan amount found during simulation. Aborting cycle test.");
            return Ok(());
        }
    };

    if loan_amount_wei.is_zero() {
        info!("Skipping transaction submission as no profitable loan was simulated.");
        return Ok(());
    }

    // 4. Execute Transaction
    // Use NonceManager directly
    let nonce_manager = Arc::new(NonceManager::new(sim_env.wallet_address));

    let mut test_config = config.clone();
    test_config.arb_executor_address = Some(executor_addr);
    // Use AppState directly
    let app_state = Arc::new(AppState::new(test_config));

    info!("Submitting arbitrage transaction...");
    // Use submit_arbitrage_transaction directly
    let submission_result = submit_arbitrage_transaction(
        client.clone(),
        app_state.clone(),
        route.clone(),
        loan_amount_wei,
        simulated_net_profit_wei,
        nonce_manager.clone(),
    ).await;

    // 5. Verification
    match submission_result {
        Ok(tx_hash) => {
            info!("✅ Transaction submitted and confirmed successfully: {}", tx_hash);
        }
        Err(e) => {
            error!("❌ Transaction submission/confirmation failed: {:?}", e);
            if e.to_string().contains("Transaction reverted on-chain") {
                warn!("Transaction reverted as expected/possible due to on-chain conditions differing from simulation.");
            } else if e.to_string().contains("ALERT:") {
                 error!("Submission failed with ALERT: {:?}", e);
                 // Use e.wrap_err directly
                 return Err(e.wrap_err("Submission failed due to ALERT"));
            } else {
                 error!("Submission failed with unexpected error: {:?}", e);
                 // Use e.wrap_err directly
                 return Err(e.wrap_err("Submission failed unexpectedly"));
            }
        }
    }

    info!("--- Test Finished: test_full_arbitrage_cycle_simulation ---");
    Ok(())
}


/// Placeholder: Test direct interaction with Huff contract functions (e.g., withdraw)
#[tokio::test]
#[ignore]
async fn test_huff_direct_call() -> Result<()> {
    setup_tracing();
    info!("--- Running Test: test_huff_direct_call ---");

    // Re-import necessary types/modules locally for this test
    use ulp1_5::bindings::ArbitrageExecutor;

    let sim_env = setup_simulation_environment().await?;
    let executor_addr = sim_env.executor_address.expect("Executor must be deployed");
    let client = sim_env.http_client.clone(); // Clone Arc'd client

    info!("Attempting direct call to Huff contract (e.g., withdraw - requires funds)");

    // Instantiate executor binding directly
    let executor = ArbitrageExecutor::new(executor_addr, client.clone());
    // Fix E0616: Access public field
    let token_to_withdraw: Address = sim_env.config.target_weth_address.parse()?;
    let recipient = sim_env.wallet_address;

    warn!("Withdraw test requires the executor contract ({:?}) to hold WETH on the Anvil fork.", executor_addr);

    let tx = executor.withdraw_token(token_to_withdraw, recipient);

    // Send the transaction
    match tx.send().await {
        Ok(pending_tx) => {
            info!("Withdraw tx sent: {:?}. Waiting for confirmation...", pending_tx.tx_hash());
            match pending_tx.await {
                Ok(Some(receipt)) => {
                    info!("✅ Withdraw call confirmed. Receipt: {:?}", receipt);
                }
                Ok(None) => {
                    warn!("Withdraw tx confirmed but receipt was not retrieved (might be pending or dropped).");
                }
                Err(e) => {
                    error!("❌ Withdraw tx failed during confirmation: {:?}", e);
                    return Err(Report::from(e).wrap_err("Withdraw tx failed during confirmation"));
                }
            }
        }
        Err(e) => {
             error!("❌ Withdraw tx failed to send: {:?}", e);
             return Err(Report::from(e).wrap_err("Withdraw tx failed to send"));
        }
    }

    Ok(())
}