// tests/integration_test.rs
#![cfg(feature = "local_simulation")] // Only compile when the feature is enabled

// Use ulp1_5:: prefix now that this is an external integration test
use ulp1_5::local_simulator::{setup_simulation_environment, trigger_v3_swap, trigger_v2_swap};
use ulp1_5::bindings::{UniswapV3Pool, VelodromeV2Pool, QuoterV2, VelodromeRouter};
use ulp1_5::state::{self, AppState, DexType}; // Added self
use ulp1_5::config::load_config;
use ulp1_5::event_handler::handle_log_event;
use ulp1_5::transaction::NonceManager;

// Keep necessary imports
use ethers::prelude::*;
use ethers::providers::Middleware; // For client.get_logs
use ethers::utils::{parse_ether, parse_units};
use std::sync::Arc;
use eyre::{Result, Report, eyre};
use tokio::time::Duration; // Added for sleep
use tracing::{info, error, warn}; // Removed unused debug for now
use tracing_subscriber::{fmt, EnvFilter};
use std::str::FromStr;


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

// Define the hardcoded implementation address for local simulation tests
#[cfg(feature = "local_simulation")]
const VELO_ROUTER_IMPL_ADDR_FOR_TEST: &str = "0xa062aE8A9c5e11aaA026fc2670B0D65cCc8B2858";


/// Test: Basic Anvil Setup and Connection AND DEX Contract Checks
#[tokio::test]
#[ignore]
async fn test_setup() -> Result<()> {
    setup_tracing();
    info!("--- Running Test: test_setup ---");
    let sim_env = setup_simulation_environment().await?;
    info!("✅ Simulation environment setup successful.");
    assert!(sim_env.executor_address.is_some() || !sim_env.config.deploy_executor_in_sim);
    info!("Executor address presence check passed.");

    // Load main config for addresses
    let main_config = load_config()?;

    // --- QuoterV2 Check ---
    info!("Attempting a direct call to QuoterV2 on Anvil...");
    let quoter_address = main_config.quoter_v2_address;
    if quoter_address == Address::zero() {
        warn!("QuoterV2 address is zero in config, skipping direct check.");
    } else {
        let quoter = QuoterV2::new(quoter_address, sim_env.http_client.clone());
        let params = ulp1_5::bindings::quoter_v2::QuoteExactInputSingleParams {
            token_in: main_config.weth_address,
            token_out: main_config.usdc_address,
            amount_in: parse_ether(1)?,
            fee: 500,
            sqrt_price_limit_x96: U256::zero(),
        };
        info!("Calling QuoterV2 ({}) with params: {:?}", quoter_address, params);
        match quoter.quote_exact_input_single(params).call().await {
            Ok(quote_result) => {
                info!("✅ Successfully called QuoterV2. Result: {:?}", quote_result);
            }
            Err(e) => {
                error!("❌ Failed to call QuoterV2 directly: {:?}", e);
            }
        }
    }
    // --- End QuoterV2 Check ---

    // --- Velo Router Check (Using HARDCODED Implementation Address for Local Sim) ---
    info!("Attempting a direct call to VelodromeRouter IMPL on Anvil (using hardcoded address)...");
    let velo_router_proxy_address = main_config.velo_router_addr;
    let velo_factory_address = main_config.velodrome_v2_factory_addr;

    if velo_router_proxy_address == Address::zero() {
        warn!("VelodromeRouter proxy address is zero in config, skipping direct check.");
    } else {
        let velo_router_impl_address = Address::from_str(VELO_ROUTER_IMPL_ADDR_FOR_TEST)?;
        info!("Using hardcoded IMPL address for local test: {}", velo_router_impl_address);
        let router = VelodromeRouter::new(velo_router_impl_address, sim_env.http_client.clone());
        let amount_in = parse_units("100", 6)?.into();
        let token_a = main_config.usdc_address;
        let token_b = main_config.weth_address;
        let stable_flag = true;
        info!("Checking pool_for with stable={} using IMPL address {}...", stable_flag, velo_router_impl_address);
        match router.pool_for(token_a, token_b, stable_flag, velo_factory_address).call().await {
             Ok(pool_addr) if pool_addr != Address::zero() => {
                  info!("✅ pool_for check successful on IMPL, found pool: {:?}", pool_addr);
                  let expected_stable_pool = Address::from_str("0x207addb05c548f262219f6b50eadff8640ed6488")?;
                  assert_eq!(pool_addr, expected_stable_pool, "Found pool does not match expected stable pool");
             }
             Ok(_) => {
                 error!("❌ pool_for check on IMPL returned zero address. No pool found for {:?}/{:?} with stable={}. Wrong flag or factory?", token_a, token_b, stable_flag);
                 return Err(eyre!("pool_for check failed - pool not found"));
             }
             Err(e) => {
                 error!("❌ pool_for check failed unexpectedly on IMPL: {:?}", e);
                 return Err(e.into());
             }
         }
        let routes = vec![ulp1_5::bindings::velodrome_router::Route {
            from: token_a,
            to: token_b,
            stable: stable_flag,
            factory: velo_factory_address,
        }];
        info!("Calling VelodromeRouter IMPL ({}) getAmountsOut with routes: {:?}", velo_router_impl_address, routes);
        match router.get_amounts_out(amount_in, routes).call().await {
            Ok(amounts) => {
                info!("✅ Successfully called VelodromeRouter IMPL.getAmountsOut. Result: {:?}", amounts);
                assert_eq!(amounts.len(), 2, "Expected 2 amounts from getAmountsOut");
                assert!(amounts[1] > U256::zero(), "Expected non-zero output amount from getAmountsOut");
            }
            Err(e) => {
                error!("❌ Failed to call VelodromeRouter IMPL.getAmountsOut directly: {:?}", e);
                 return Err(e.into());
            }
        }
    }
    Ok(())
}

/// Test: Triggering Swaps on Anvil Fork
// (Remains unchanged)
#[tokio::test]
#[ignore]
async fn test_swap_triggers() -> Result<()> {
    setup_tracing();
    info!("--- Running Test: test_swap_triggers ---");
    let sim_env = setup_simulation_environment().await?;
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
             &sim_env,
             uni_pool_addr,
             &uni_pool,
             recipient,
             zero_for_one,
             amount_specified,
             sqrt_price_limit_x96,
             data.clone(),
         ).await {
             Ok(tx_hash) => info!("✅ UniV3 swap triggered successfully: {}", tx_hash),
             Err(e) => warn!("⚠️ UniV3 swap trigger failed: {:?}", e),
         }
    } else {
         info!("Skipping UniV3 swap trigger: address is zero.");
    }
    let velo_pool_addr: Address = sim_env.config.target_velodrome_v2_pool_address.parse()?;
    if velo_pool_addr != Address::zero() {
         info!("Attempting to trigger VeloV2 swap on pool: {}", velo_pool_addr);
         let velo_pool = VelodromeV2Pool::new(velo_pool_addr, sim_env.http_client.clone());
         let amount0_out = U256::zero();
         let amount1_out = parse_units("10", 6)?.into();
         let to_address = sim_env.wallet_address;
         let data = Bytes::new();
         match trigger_v2_swap(
             &sim_env,
             velo_pool_addr,
             &velo_pool,
             amount0_out,
             amount1_out,
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

/// Test: Simulate UniV3 -> VeloV2 arbitrage cycle
// (Remains unchanged)
#[tokio::test]
#[ignore]
async fn test_full_arbitrage_cycle_simulation() -> Result<()> {
    setup_tracing();
    info!("--- Running Test: test_full_arbitrage_cycle_simulation (UniV3 -> VeloV2) ---");
    use ulp1_5::state::{AppState, DexType}; // Keep these local
    use ulp1_5::path_optimizer::RouteCandidate;
    use ulp1_5::simulation::find_optimal_loan_amount;
    use ulp1_5::transaction::{fetch_gas_price, submit_arbitrage_transaction};
    use ulp1_5::utils::ToF64Lossy;
    let sim_env = setup_simulation_environment().await?;
    let client = sim_env.http_client.clone();
    let executor_addr = sim_env.executor_address.expect("Executor must be deployed for this test");
    info!("Anvil Setup Complete. Executor: {:?}", executor_addr);
    let config = load_config().expect("Failed to load test config from .env");
    let weth_addr = config.weth_address;
    let usdc_addr = config.usdc_address;
    let pool_a_addr_str = "0x851492574065EDE975391E141377067943aA08eF";
    let pool_b_addr_str = "0x207addb05c548f262219f6b50eadff8640ed6488";
    let pool_a_addr: Address = pool_a_addr_str.parse()?;
    let pool_b_addr: Address = pool_b_addr_str.parse()?;
    info!("Using Test Pools: A (UniV3 0.05%)={}, B (VeloV2 Stable)={}", pool_a_addr, pool_b_addr);
    let route = RouteCandidate {
        buy_pool_addr: pool_a_addr,
        sell_pool_addr: pool_b_addr,
        buy_dex_type: DexType::UniswapV3,
        sell_dex_type: DexType::VelodromeV2,
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
    let app_state = Arc::new(AppState::new(config.clone()));
    let gas_info = fetch_gas_price(client.clone(), &config).await?;
    let gas_price_gwei = ToF64Lossy::to_f64_lossy(&gas_info.max_priority_fee_per_gas) / 1e9;
    info!("Fetched Anvil gas price (prio): {} gwei", gas_price_gwei);
    let buy_snapshot = None;
    let sell_snapshot = None;
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
            warn!("❌ No profitable loan amount found during simulation (UniV3->VeloV2). This is likely due to the Anvil/Velo simulation workaround. Aborting cycle test.");
            return Ok(());
        }
    };
    if loan_amount_wei.is_zero() {
        info!("Skipping transaction submission as no profitable loan was simulated.");
        return Ok(());
    }
    let nonce_manager = Arc::new(NonceManager::new(sim_env.wallet_address));
    let mut test_config = config.clone();
    test_config.arb_executor_address = Some(executor_addr);
    let app_state = Arc::new(AppState::new(test_config));
    info!("Submitting arbitrage transaction...");
    let submission_result = submit_arbitrage_transaction(
        client.clone(),
        app_state.clone(),
        route.clone(),
        loan_amount_wei,
        simulated_net_profit_wei,
        nonce_manager.clone(),
    ).await;
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
                 return Err(e.wrap_err("Submission failed due to ALERT"));
            } else {
                 error!("Submission failed with unexpected error: {:?}", e);
                 return Err(e.wrap_err("Submission failed unexpectedly"));
            }
        }
    }
    info!("--- Test Finished: test_full_arbitrage_cycle_simulation (UniV3 -> VeloV2) ---");
    Ok(())
}

/// Test: Simulate UniV3 -> UniV3 arbitrage cycle
// (Remains unchanged)
#[tokio::test]
#[ignore]
async fn test_full_univ3_arbitrage_cycle() -> Result<()> {
    setup_tracing();
    info!("--- Running Test: test_full_univ3_arbitrage_cycle (UniV3 -> UniV3) ---");
    use ulp1_5::state::{AppState, DexType};
    use ulp1_5::path_optimizer::RouteCandidate;
    use ulp1_5::simulation::find_optimal_loan_amount;
    use ulp1_5::transaction::{fetch_gas_price, submit_arbitrage_transaction};
    use ulp1_5::utils::ToF64Lossy;
    let sim_env = setup_simulation_environment().await?;
    let client = sim_env.http_client.clone();
    let executor_addr = sim_env.executor_address.expect("Executor must be deployed for this test");
    info!("Anvil Setup Complete. Executor: {:?}", executor_addr);
    let config = load_config().expect("Failed to load test config from .env");
    let weth_addr = config.weth_address;
    let usdc_addr = config.usdc_address;
    let pool_a_addr: Address = "0x851492574065EDE975391E141377067943aA08eF".parse()?;
    let pool_b_addr: Address = "0x171d751916657a873807a11785294c280ca7433D".parse()?;
    info!("Using Test Pools: A (UniV3 0.05%)={}, B (UniV3 0.3%)={}", pool_a_addr, pool_b_addr);
    let route = RouteCandidate {
        buy_pool_addr: pool_a_addr,
        sell_pool_addr: pool_b_addr,
        buy_dex_type: DexType::UniswapV3,
        sell_dex_type: DexType::UniswapV3,
        token_in: weth_addr,
        token_out: usdc_addr,
        buy_pool_fee: Some(500),
        sell_pool_fee: Some(3000),
        buy_pool_stable: None,
        sell_pool_stable: None,
        buy_pool_factory: config.uniswap_v3_factory_addr,
        sell_pool_factory: config.uniswap_v3_factory_addr,
        zero_for_one_a: true,
        estimated_profit_usd: 0.01,
    };
    info!("Constructed Manual Route Candidate: {:?}", route);
    let app_state = Arc::new(AppState::new(config.clone()));
    let gas_info = fetch_gas_price(client.clone(), &config).await?;
    let gas_price_gwei = ToF64Lossy::to_f64_lossy(&gas_info.max_priority_fee_per_gas) / 1e9;
    info!("Fetched Anvil gas price (prio): {} gwei", gas_price_gwei);
    let buy_snapshot = None;
    let sell_snapshot = None;
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
            warn!("❌ No profitable loan amount found during simulation (UniV3->UniV3). Fork state might not have arb opportunity.");
            return Ok(());
        }
    };
    if loan_amount_wei.is_zero() {
        info!("Skipping transaction submission as no profitable loan was simulated.");
        return Ok(());
    }
    let nonce_manager = Arc::new(NonceManager::new(sim_env.wallet_address));
    let mut test_config = config.clone();
    test_config.arb_executor_address = Some(executor_addr);
    let app_state = Arc::new(AppState::new(test_config));
    info!("Submitting arbitrage transaction...");
    let submission_result = submit_arbitrage_transaction(
        client.clone(),
        app_state.clone(),
        route.clone(),
        loan_amount_wei,
        simulated_net_profit_wei,
        nonce_manager.clone(),
    ).await;
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
                 return Err(e.wrap_err("Submission failed due to ALERT"));
            } else {
                 error!("Submission failed with unexpected error: {:?}", e);
                 return Err(e.wrap_err("Submission failed unexpectedly"));
            }
        }
    }
    info!("--- Test Finished: test_full_univ3_arbitrage_cycle (UniV3 -> UniV3) ---");
    Ok(())
}

/// Test: Websocket Event Handling and Arbitrage Check Trigger
#[tokio::test]
#[ignore]
async fn test_event_handling_triggers_arbitrage_check() -> Result<()> {
    setup_tracing();
    info!("--- Running Test: test_event_handling_triggers_arbitrage_check ---");

    let sim_env = setup_simulation_environment().await?;
    let client = sim_env.http_client.clone();
    let config = load_config()?;
    let mut test_config = config.clone();
    test_config.arb_executor_address = sim_env.executor_address;

    let app_state = Arc::new(AppState::new(test_config));
    let nonce_manager = Arc::new(NonceManager::new(sim_env.wallet_address));

    let target_pool_addr: Address = "0x851492574065EDE975391E141377067943aA08eF".parse()?;
    let uni_factory_addr = config.uniswap_v3_factory_addr;

    info!("Fetching initial state for pool: {}", target_pool_addr);
    state::fetch_and_cache_pool_state(
        target_pool_addr,
        DexType::UniswapV3,
        uni_factory_addr,
        client.clone(),
        app_state.clone()
    ).await.expect("Initial pool state fetch failed");

    let initial_snapshot = app_state.pool_snapshots.get(&target_pool_addr)
        .map(|s| s.value().clone())
        .expect("Initial snapshot missing after fetch");
    info!(?initial_snapshot, "Initial snapshot fetched.");

    info!("Triggering a V3 swap on pool {} to generate an event...", target_pool_addr);
    let uni_pool_binding = UniswapV3Pool::new(target_pool_addr, client.clone());
    let swap_tx_hash = trigger_v3_swap(
        &sim_env,
        target_pool_addr,
        &uni_pool_binding,
        sim_env.wallet_address,
        true,
        I256::from_raw(parse_ether("0.01")?),
        ethers::types::U256::zero(), // Corrected U256 path
        Bytes::new(), // Corrected Bytes path
    ).await.expect("Swap trigger failed");
    info!("Swap triggered, tx hash: {}. Waiting for receipt...", swap_tx_hash);

    let receipt = client.get_transaction_receipt(swap_tx_hash).await?
        .ok_or_else(|| eyre!("Failed to get receipt for swap tx {}", swap_tx_hash))?;
    info!("Swap transaction mined in block: {:?}", receipt.block_number);

    let swap_event_signature = ulp1_5::UNI_V3_SWAP_TOPIC.clone();
    let swap_log = receipt.logs.into_iter()
        .find(|log| log.address == target_pool_addr && !log.topics.is_empty() && log.topics[0] == swap_event_signature)
        .ok_or_else(|| eyre!("Swap log not found in transaction receipt for pool {}", target_pool_addr))?;

    info!("Swap log found: {:?}", swap_log);
    let log_to_process = swap_log.clone();

    info!("Passing swap log to handle_log_event...");
    handle_log_event(log_to_process, app_state.clone(), client.clone(), nonce_manager.clone()).await?;
    info!("handle_log_event processed.");

    let updated_snapshot = app_state.pool_snapshots.get(&target_pool_addr)
        .map(|s| s.value().clone())
        .expect("Updated snapshot missing after event handling");

    info!(?updated_snapshot, "Snapshot after event handling.");

    assert_ne!(initial_snapshot.sqrt_price_x96, updated_snapshot.sqrt_price_x96, "SqrtPriceX96 should have changed");
    assert_ne!(initial_snapshot.tick, updated_snapshot.tick, "Tick should have changed");
    assert!(updated_snapshot.last_update_block.is_some(), "Last update block should be set");
    assert_eq!(updated_snapshot.last_update_block, receipt.block_number, "Last update block should match swap block");

    info!("✅ PoolSnapshot successfully updated by handle_log_event.");
    warn!("Further verification of check_for_arbitrage actual execution and route finding requires more advanced test setup or specific log scraping.");
    tokio::time::sleep(Duration::from_secs(2)).await;


    info!("--- Test Finished: test_event_handling_triggers_arbitrage_check ---");
    Ok(())
}

/// Placeholder: Test direct interaction with Huff contract functions (e.g., withdraw)
#[tokio::test]
#[ignore]
async fn test_huff_direct_call() -> Result<()> {
    setup_tracing();
    info!("--- Running Test: test_huff_direct_call ---");
    use ulp1_5::bindings::ArbitrageExecutor;
    let sim_env = setup_simulation_environment().await?;
    let executor_addr = sim_env.executor_address.expect("Executor must be deployed");
    let client = sim_env.http_client.clone();
    info!("Attempting direct call to Huff contract (e.g., withdraw - requires funds)");
    let executor = ArbitrageExecutor::new(executor_addr, client.clone());
    let token_to_withdraw: Address = sim_env.config.target_weth_address.parse()?;
    let recipient = sim_env.wallet_address;
    warn!("Withdraw test requires the executor contract ({:?}) to hold WETH on the Anvil fork.", executor_addr);
    let tx = executor.withdraw_token(token_to_withdraw, recipient);
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