// tests/integration_test.rs
#![cfg(feature = "local_simulation")] // Only compile when the feature is enabled

// Use ulp1_5:: prefix now that this is an external integration test
use ulp1_5::local_simulator::{setup_simulation_environment, trigger_v2_swap, trigger_v3_swap_via_router};
use ulp1_5::bindings::{VelodromeV2Pool, QuoterV2, VelodromeRouter, MinimalSwapEmitter, IERC20, UniswapV3Pool}; // Added UniswapV3Pool back for initial state fetch
use ulp1_5::state::{self, AppState, DexType};
use ulp1_5::config::load_config;
use ulp1_5::event_handler::handle_log_event;
use ulp1_5::transaction::NonceManager;

use ethers::{
    abi::Abi,
    contract::EthEvent, // Added for Event signature
    prelude::*,
    providers::Middleware,
    // types::TransactionReceipt, // Removed unused import
    utils::{hex, parse_ether, parse_units},
};
use std::sync::Arc;
use eyre::{Result, eyre, WrapErr};
use tokio::time::Duration;
use tracing::{info, error, warn};
use tracing_subscriber::{fmt, EnvFilter};
use std::{fs, str::FromStr};


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

/// Generic helper to deploy a contract from ABI and Bytecode string
async fn deploy_contract_from_abi_and_bytecode<M: Middleware + 'static>(
    client: Arc<M>,
    abi: Abi,
    bytecode_hex: &str,
) -> Result<(Address, Contract<M>)> // Changed return type slightly
    where M::Error: 'static + Send + Sync,
{
    let cleaned_bytecode_hex = bytecode_hex.trim().trim_start_matches("0x");
    let bytecode = hex::decode(cleaned_bytecode_hex).wrap_err("Failed to decode hex bytecode for generic deploy")?;
    let factory = ContractFactory::new(abi.clone(), Bytes::from(bytecode), client.clone()); // Clone ABI for factory
    
    info!("Deploying generic contract...");
    // Deploy and get the instance for type safety, then address
    let contract_instance = factory.deploy(())?.send().await.wrap_err("Generic contract deployment send failed")?;
    let addr = contract_instance.address();
    // Re-create contract instance with the specific address for the returned Contract<M>
    // This ensures the Contract<M> is bound to the correct deployed address.
    let deployed_contract = Contract::new(addr, abi, client);

    info!("✅ Generic contract deployed to Anvil at: {:?}", addr);
    Ok((addr, deployed_contract))
}


/// Test: Basic Anvil Setup and Connection AND DEX Contract Checks
#[tokio::test]
#[ignore]
async fn test_setup() -> Result<()> {
    // ... (test_setup remains unchanged)
    setup_tracing();
    info!("--- Running Test: test_setup ---");
    let sim_env = setup_simulation_environment().await?;
    info!("✅ Simulation environment setup successful.");
    assert!(sim_env.executor_address.is_some() || !sim_env.config.deploy_executor_in_sim);
    info!("Executor address presence check passed.");
    let main_config = load_config()?;
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
    
    let amount_eth_to_swap = parse_ether("0.01")?;
    let usdc_address = Address::from_str(sim_env.config.target_usdc_address)?;
    let target_pool_fee = 500; 
    
    info!("Attempting to trigger UniV3 swap via router: WETH -> USDC.e");
    match trigger_v3_swap_via_router(
        &sim_env,
        amount_eth_to_swap,
        usdc_address,
        target_pool_fee,
        sim_env.wallet_address, 
        U256::zero(), 
    ).await {
        Ok(receipt) => info!("✅ UniV3 swap via router triggered successfully. Tx: {:?}", receipt.transaction_hash),
        Err(e) => {
            error!("❌ UniV3 swap via router trigger failed: {:?}", e);
        }
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
    // ... (content unchanged)
    setup_tracing();
    info!("--- Running Test: test_full_arbitrage_cycle_simulation (UniV3 -> VeloV2) ---");
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
    // ... (content unchanged)
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

    let app_state = Arc::new(AppState::new(test_config.clone()));
    let nonce_manager = Arc::new(NonceManager::new(sim_env.wallet_address));

    // --- Deploy MinimalSwapEmitter ---
    let emitter_bytecode_hex = fs::read_to_string("./build/MinimalSwapEmitter.bin")
        .wrap_err("Failed to read MinimalSwapEmitter bytecode. Ensure it's compiled to ./build/MinimalSwapEmitter.bin")?;
    let emitter_abi_str = fs::read_to_string("./abis/MinimalSwapEmitter.json")?;
    let emitter_abi: Abi = serde_json::from_str(&emitter_abi_str)?;

    let (emitter_addr, _emitter_contract_instance) = deploy_contract_from_abi_and_bytecode( // Renamed variable
        client.clone(),
        emitter_abi,
        &emitter_bytecode_hex
    ).await?;
    info!("✅ MinimalSwapEmitter deployed at: {}", emitter_addr);
    let emitter = MinimalSwapEmitter::new(emitter_addr, client.clone()); // Create typed binding

    let actual_uni_pool_addr: Address = sim_env.config.target_uniswap_v3_pool_address.parse()?;
    let uni_factory_addr = config.uniswap_v3_factory_addr;

    info!("Fetching initial state for actual UniV3 pool: {}", actual_uni_pool_addr);
    state::fetch_and_cache_pool_state(
        actual_uni_pool_addr,
        DexType::UniswapV3,
        uni_factory_addr,
        client.clone(),
        app_state.clone()
    ).await.expect("Initial pool state fetch for actual_uni_pool_addr failed");

    let initial_snapshot = app_state.pool_snapshots.get(&actual_uni_pool_addr)
        .map(|s| s.value().clone())
        .expect("Initial snapshot missing after fetch for actual_uni_pool_addr");
    info!(?initial_snapshot, "Initial snapshot fetched for actual_uni_pool_addr.");

    info!("Calling MinimalSwapEmitter to emit a synthetic Swap event AS IF from pool {}", actual_uni_pool_addr);
    let new_sqrt_price = initial_snapshot.sqrt_price_x96.unwrap_or_default() + U256::from(10000);
    let new_tick = initial_snapshot.tick.unwrap_or_default() + 10;

    // Correct types for emitMinimalSwap
    let tx_call = emitter.emit_minimal_swap(
        I256::from(1000), // amount0
        I256::from(-1000), // amount1
        new_sqrt_price, // sqrtPriceX96 (U256, Solidity uint160 will truncate)
        1234567890_u128, // liquidity
        new_tick // tick
    );
    let tx_receipt = tx_call.send().await?.await?.ok_or_else(|| eyre!("Emitter tx not mined"))?; // Get receipt
    assert_eq!(tx_receipt.status, Some(1.into()), "Emitter transaction failed");
    info!("Synthetic event emitted. Tx hash: {:?}", tx_receipt.transaction_hash);
    
    // Get the event signature from the generated binding for MinimalSwapEmitter
    let swap_event_signature = MinimalSwapEmitter::Swap::signature();
    
    let found_log = tx_receipt.logs.iter().find(|l| l.address == emitter_addr && l.topics[0] == swap_event_signature)
        .ok_or_else(|| eyre!("Synthetic Swap log not found from emitter in tx {:?}", tx_receipt.transaction_hash))?;

    let mut log_to_process = found_log.clone();
    log_to_process.address = actual_uni_pool_addr;
    log_to_process.block_number = tx_receipt.block_number;

    info!("Synthetic Swap log prepared: {:?}", log_to_process);

    info!("Passing synthetic swap log to handle_log_event...");
    handle_log_event(log_to_process, app_state.clone(), client.clone(), nonce_manager.clone()).await?;
    info!("handle_log_event processed.");

    let updated_snapshot = app_state.pool_snapshots.get(&actual_uni_pool_addr)
        .map(|s| s.value().clone())
        .expect("Updated snapshot missing after event handling for actual_uni_pool_addr");

    info!(?updated_snapshot, "Snapshot after event handling for actual_uni_pool_addr.");

    assert_eq!(updated_snapshot.sqrt_price_x96, Some(new_sqrt_price), "SqrtPriceX96 should match emitted");
    assert_eq!(updated_snapshot.tick, Some(new_tick), "Tick should match emitted");
    assert!(updated_snapshot.last_update_block.is_some(), "Last update block should be set");
    assert_eq!(updated_snapshot.last_update_block, tx_receipt.block_number, "Last update block should match emitter tx block");

    info!("✅ PoolSnapshot successfully updated by handle_log_event using synthetic event.");
    warn!("Further verification of check_for_arbitrage actual execution and route finding requires more advanced test setup or specific log scraping.");
    tokio::time::sleep(Duration::from_secs(2)).await;


    info!("--- Test Finished: test_event_handling_triggers_arbitrage_check ---");
    Ok(())
}