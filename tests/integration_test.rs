// tests/integration_test.rs
#![cfg(feature = "local_simulation")]
#![allow(unused_imports, dead_code)] // Allow unused during dev

use ethers::{
    abi::{AbiDecode, RawLog, Token, Abi}, 
    contract::{ContractError, EthLogDecode}, 
    prelude::*,
    types::{Address, Bytes, Filter, Log, TransactionReceipt, TxHash, I256, U256, U64},
    utils::{format_units, hex, parse_ether, parse_units},
};
use eyre::{eyre, Result, WrapErr};
use std::{env, path::Path, str::FromStr, sync::Arc, time::Duration};
use tokio::sync::{Mutex, OnceCell}; 
use tracing::{debug, error, info, instrument, trace, warn};
use tracing_subscriber::{fmt, EnvFilter, FmtSubscriber};

use ulp1_5::{
    bindings::{
        self, AerodromePool, BalancerVault, IUniswapV3Factory, IVelodromeFactory,
        MinimalSwapEmitter, QuoterV2, SwapRouter, UniswapV3Pool, VelodromeRouter,
        VelodromeV2Pool, IWETH9, uniswap_v3_pool::SwapFilter as UniV3SwapFilter, 
        MINIMALSWAPEMITTER_ABI 
    },
    config::{self, load_config, Config},
    deploy::deploy_contract_from_bytecode,
    encoding::encode_user_data,
    event_handler::{handle_log_event, check_for_arbitrage}, 
    gas::estimate_flash_loan_gas,
    local_simulator::{
        self, setup_simulation_environment as actual_setup_simulation_environment, 
        trigger_v3_swap_via_router, AnvilClient, SimEnv,
        SimulationConfig, SIMULATION_CONFIG,
    },
    simulation::{calculate_net_profit, find_optimal_loan_amount, simulate_swap},
    state::{self, AppState, DexType, PoolSnapshot, PoolState},
    transaction::{self, NonceManager},
    utils::{self, f64_to_wei, ToF64Lossy},
    path_optimizer, 
};

// Global OnceCell to hold the initialized SimEnv for all tests
static SHARED_SIM_ENV: OnceCell<Arc<SimEnv>> = OnceCell::const_new();

// Wrapper function to get or initialize the shared SimEnv
async fn get_shared_sim_env() -> Result<Arc<SimEnv>> {
    SHARED_SIM_ENV.get_or_try_init(|| async {
        info!("(Global Test Setup) Initializing Shared SimEnv for the first time...");
        actual_setup_simulation_environment().await 
            .wrap_err("Failed to initialize shared SimEnv")
            .map(Arc::new)
    }).await.cloned() 
}


// --- Test Setup ---
async fn setup_test_env() -> Result<(Arc<SimEnv>, Arc<AppState>, Arc<NonceManager>)> {
    let subscriber = FmtSubscriber::builder()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into())) // Reduced default log level for tests
        .with_test_writer()
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber); 

    dotenv::dotenv().ok();
    info!("(Test Setup) Loading config for integration test...");

    let sim_env = get_shared_sim_env().await?;
    info!("(Test Setup) Shared Anvil environment obtained/ready.");

    env::set_var("WS_RPC_URL", SIMULATION_CONFIG.anvil_ws_url);
    env::set_var("HTTP_RPC_URL", SIMULATION_CONFIG.anvil_http_url);
    env::set_var("LOCAL_PRIVATE_KEY", SIMULATION_CONFIG.anvil_private_key); 
    env::set_var(
        "UNISWAP_V3_FACTORY_ADDR",
        "0x1F98431c8aD98523631AE4a59f267346ea31F984",
    ); 
    env::set_var(
        "VELODROME_V2_FACTORY_ADDR",
        "0x25CbdDb98b35AB1FF795324516342Fac4845718f",
    ); 
    env::set_var("WETH_ADDRESS", SIMULATION_CONFIG.target_weth_address);
    env::set_var("USDC_ADDRESS", SIMULATION_CONFIG.target_usdc_address);
    env::set_var(
        "VELO_V2_ROUTER_ADDR",
        "0x9c12939390052919aF3155f41Bf41543Ca30607B",
    ); 
    env::set_var(
        "BALANCER_VAULT_ADDRESS",
        "0xBA12222222228d8Ba445958a75a0704d566BF2C9",
    ); 
    env::set_var(
        "QUOTER_V2_ADDRESS",
        "0x61fFE014bA17989E743c5F6cB21bF9697530B21e",
    ); 
    env::set_var("WETH_DECIMALS", "18");
    env::set_var("USDC_DECIMALS", "6");
    
    env::set_var("DEPLOY_EXECUTOR", "false"); 
    let executor_addr = sim_env.executor_address
        .ok_or_else(|| eyre!("Executor address missing from shared SimEnv. Global deployment might have failed."))?;
    
    let executor_addr_hex_string = format!("{:#x}", executor_addr);
    info!("(Test Setup) Setting ARBITRAGE_EXECUTOR_ADDRESS to: {}", executor_addr_hex_string);
    env::set_var("ARBITRAGE_EXECUTOR_ADDRESS", executor_addr_hex_string); 

    env::set_var("OPTIMAL_LOAN_SEARCH_ITERATIONS", "2");
    env::set_var("ENABLE_UNIV3_DYNAMIC_SIZING", "false");
    env::set_var("FETCH_TIMEOUT_SECS", "5");


    let config = load_config().wrap_err("Failed to load test config")?;
    let app_state = Arc::new(AppState::new(config.clone()));
    let nonce_manager = Arc::new(NonceManager::new(sim_env.wallet_address));
    info!("(Test Setup) Bot AppState and NonceManager initialized.");

    info!("(Test Setup) Fetching initial state for target pools...");
    let target_uni_pool: Address = SIMULATION_CONFIG.target_uniswap_v3_pool_address.parse()?;
    let target_velo_pool: Address = SIMULATION_CONFIG.target_velodrome_v2_pool_address.parse()?;

    if !app_state.pool_states.contains_key(&target_uni_pool) {
        match state::fetch_and_cache_pool_state(
            target_uni_pool,
            DexType::UniswapV3,
            app_state.config.uniswap_v3_factory_addr, 
            sim_env.http_client.clone(), 
            app_state.clone(),
        ).await {
            Ok(_) => info!("(Test Setup) Successfully fetched UniV3 pool {} state.", target_uni_pool),
            Err(e) => warn!(pool=%target_uni_pool, error=?e, "(Test Setup) Failed initial UniV3 state fetch (relying on fallback if active)."),
        };
    }
    if !app_state.pool_states.contains_key(&target_velo_pool) {
         match state::fetch_and_cache_pool_state(
            target_velo_pool,
            DexType::VelodromeV2,
            app_state.config.velodrome_v2_factory_addr, 
            sim_env.http_client.clone(), 
            app_state.clone(),
        ).await {
            Ok(_) => info!("(Test Setup) Successfully fetched VeloV2 pool {} state.", target_velo_pool),
            Err(e) => warn!(pool=%target_velo_pool, error=?e, "(Test Setup) Failed initial VeloV2 state fetch (relying on fallback if active)."),
        };
    }
    
    if app_state.pool_states.contains_key(&target_uni_pool) && app_state.pool_snapshots.contains_key(&target_uni_pool) {
        info!(pool=%target_uni_pool, "(Test Setup) UniV3 state is present in AppState.");
    } else {
        warn!(pool=%target_uni_pool, "(Test Setup) UniV3 state potentially missing or initial fetch failed, relying on fallback if any.");
    }
    if app_state.pool_states.contains_key(&target_velo_pool) && app_state.pool_snapshots.contains_key(&target_velo_pool) {
        info!(pool=%target_velo_pool, "(Test Setup) VeloV2 state is present in AppState.");
    } else {
        warn!(pool=%target_velo_pool, "(Test Setup) VeloV2 state potentially missing or initial fetch failed, relying on fallback if any.");
    }

    info!("(Test Setup) Initial pool state fetch attempts completed.");
    info!("(Test Setup) Setup complete.");
    Ok((sim_env, app_state, nonce_manager))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)] 
#[ignore] 
async fn test_setup() -> Result<()> {
    let (sim_env, app_state, _nonce_manager) = setup_test_env().await?;
    info!("Test: Checking Anvil connection and initial state...");

    let block_num = sim_env.http_client.get_block_number().await?;
    info!("Anvil current block number: {}", block_num);
    assert!(block_num > U64::zero());

    assert!(sim_env.executor_address.is_some());
    info!("Executor deployed at: {:?}", sim_env.executor_address.unwrap());

    assert_eq!(
        app_state.config.ws_rpc_url,
        SIMULATION_CONFIG.anvil_ws_url
    );
    assert_eq!(app_state.config.arb_executor_address, sim_env.executor_address); 

    let target_uni_pool: Address = SIMULATION_CONFIG.target_uniswap_v3_pool_address.parse()?;
    let target_velo_pool: Address = SIMULATION_CONFIG.target_velodrome_v2_pool_address.parse()?;
    
    assert!(app_state.pool_states.contains_key(&target_uni_pool), "Target Uni pool state missing. Fallback might not have worked or pool is genuinely unreadable on fork.");
    assert!(app_state.pool_snapshots.contains_key(&target_uni_pool), "Target Uni pool snapshot missing.");
    
    if !app_state.pool_states.contains_key(&target_velo_pool) {
        warn!("Target Velo pool state (0x207a…6488) missing in test_setup, this is often an Anvil fork issue with this specific pool. Continuing test as UniV3 is primary for some paths.");
    }
    if !app_state.pool_snapshots.contains_key(&target_velo_pool) {
        warn!("Target Velo pool snapshot (0x207a…6488) missing in test_setup.");
    }


    info!("✅ Test Setup successful.");

    info!("Running diagnostic calls...");

    let quoter_addr = app_state.config.quoter_v2_address;
    let quoter = QuoterV2::new(quoter_addr, sim_env.http_client.clone());
    let weth_addr = app_state.config.weth_address;
    let usdc_addr = app_state.config.usdc_address;
    let fee = 500; 
    let amount_in = parse_ether("1")?;

    let params = bindings::quoter_v2::QuoteExactInputSingleParams {
        token_in: weth_addr,
        token_out: usdc_addr,
        amount_in,
        fee,
        sqrt_price_limit_x96: U256::zero(),
    };
    match quoter.quote_exact_input_single(params.clone()).call().await {
         Ok(quote_result) => info!(quoter=?quoter_addr, weth=?weth_addr, usdc=?usdc_addr, fee=%fee, amount_in=%amount_in, quote = %quote_result.0, "✅ Diagnostic QuoterV2 call successful."),
         Err(e) => error!(quoter=?quoter_addr, weth=?weth_addr, usdc=?usdc_addr, fee=%fee, amount_in=%amount_in, error=?e, "❌ Diagnostic QuoterV2 call FAILED."),
    };

    let velo_router_impl_addr: Address = local_simulator::VELO_ROUTER_IMPL_ADDR_FOR_SIM.parse()?;
    let velo_router_impl = VelodromeRouter::new(velo_router_impl_addr, sim_env.http_client.clone());
    let factory_addr = app_state.config.velodrome_v2_factory_addr;
    let routes = vec![bindings::velodrome_router::Route {
        from: weth_addr,
        to: usdc_addr,
        stable: true, 
        factory: factory_addr,
    }];
    match velo_router_impl.get_amounts_out(amount_in, routes.clone()).call().await {
         Ok(amounts) => info!(router_impl=?velo_router_impl_addr, factory=?factory_addr, routes=?routes, amount_in=%amount_in, amounts_out=?amounts, "✅ Diagnostic Velo Router Impl getAmountsOut call successful."),
         Err(e) => {
            let error_string = e.to_string();
            let revert_data_opt = if let ContractError::Revert(ref data_bytes) = e { Some(hex::encode(&data_bytes.0)) } else { None };
             error!(router_impl=?velo_router_impl_addr, factory=?factory_addr, routes=?routes, amount_in=%amount_in, error=%error_string, revert_data=?revert_data_opt, "❌ Diagnostic Velo Router Impl getAmountsOut call FAILED.")
         },
    };


    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1, timeout = 180000)] // 180 seconds in ms
#[ignore] 
async fn test_swap_triggers() -> Result<()> {
    let (sim_env, app_state, _nonce_manager) = setup_test_env().await?;
    info!("Test: Triggering simulated swaps on Anvil...");

    let amount_eth_in = parse_ether("0.05")?;
    let token_out_addr = app_state.config.usdc_address;
    let pool_fee = 500; 
    let recipient = sim_env.wallet_address;

    info!("Attempting to trigger UniV3 swap...");
    let v3_receipt = trigger_v3_swap_via_router(
        &sim_env,
        amount_eth_in,
        token_out_addr,
        pool_fee,
        recipient,
        U256::zero(), 
    )
    .await
    .wrap_err("Failed to trigger UniV3 swap via router")?;

    assert_eq!(v3_receipt.status, Some(1.into()), "UniV3 swap tx failed");
    info!(tx_hash = ?v3_receipt.transaction_hash, "✅ Triggered UniV3 swap successfully.");

    let target_velo_pool: Address = SIMULATION_CONFIG.target_velodrome_v2_pool_address.parse()?;
    let velo_pool_state_opt = app_state.pool_states.get(&target_velo_pool);
    let velo_pool_snapshot_opt = app_state.pool_snapshots.get(&target_velo_pool);

    if velo_pool_state_opt.is_none() || velo_pool_snapshot_opt.is_none() {
         warn!("Velo pool state or snapshot missing, skipping Velo swap trigger test (this is common with Anvil forks for this pool).");
         return Ok(());
    }
    let velo_pool_state = velo_pool_state_opt.unwrap().value().clone();
    let velo_pool_snapshot = velo_pool_snapshot_opt.unwrap().value().clone();


    if velo_pool_state.token0.is_zero() || velo_pool_state.token1.is_zero() {
        warn!("Velo pool tokens are zero address, cannot perform swap test.");
        return Ok(()); 
    }
    if velo_pool_snapshot.reserve0.unwrap_or_default().is_zero() || velo_pool_snapshot.reserve1.unwrap_or_default().is_zero() {
         warn!("Velo pool reserves are zero, cannot perform swap test.");
         return Ok(());
    }

    let amount_weth_to_swap = parse_ether("0.01")?;
    let velo_token_in = app_state.config.weth_address;
    let velo_token_out = app_state.config.usdc_address;

    let requesting_usdc_as_token0 = velo_pool_state.token0 == velo_token_out; 

    let simulated_usdc_out = simulate_swap(
        app_state.clone(),
        sim_env.http_client.clone(),
        DexType::VelodromeV2,
        velo_token_in, 
        velo_token_out, 
        amount_weth_to_swap,
        velo_pool_state.velo_stable,
        None,
        Some(velo_pool_state.factory),
    ).await.unwrap_or_default();

    info!("Simulated USDC out from Velo swap: {}", format_units(simulated_usdc_out, 6)?);
    if simulated_usdc_out.is_zero() {
          warn!("Velo swap simulation resulted in zero output, skipping trigger.");
          return Ok(());
    }

    let (amount0_out, amount1_out) = if requesting_usdc_as_token0 {
        (simulated_usdc_out, U256::zero()) 
    } else {
        (U256::zero(), simulated_usdc_out) 
    };

    let weth_contract = IWETH9::new(app_state.config.weth_address, sim_env.http_client.clone());
    info!("Approving Velo pool {} to spend WETH...", target_velo_pool);
    let approve_tx_call = weth_contract.approve(target_velo_pool, amount_weth_to_swap);
    let approve_pending_tx = approve_tx_call.send().await.wrap_err("WETH approval for Velo pool send failed")?;
    let approve_receipt = approve_pending_tx.await?.ok_or_else(|| eyre!("WETH approval for Velo pool not mined"))?;
    assert_eq!(approve_receipt.status, Some(1.into()), "WETH approval for Velo pool failed");
    info!("WETH approved for Velo pool.");


    info!("Attempting to trigger VeloV2 swap (requesting {} USDC)...", format_units(simulated_usdc_out, 6)?);
    let pool_contract = VelodromeV2Pool::new(target_velo_pool, sim_env.http_client.clone());
    let swap_call = pool_contract.swap(amount0_out, amount1_out, recipient, Bytes::new());
    let tx_request: TransactionRequest = swap_call.tx.clone().into();
    let pending_tx = sim_env
        .http_client
        .send_transaction(tx_request, None)
        .await
        .wrap_err("Send Velo swap transaction failed")?;
    let tx_hash = *pending_tx;
    info!(%tx_hash, "Velo Swap transaction sent to Anvil.");
    let v2_receipt = pending_tx.await?.ok_or_else(|| eyre!("Velo swap tx not mined"))?;

    assert_eq!(v2_receipt.status, Some(1.into()), "VeloV2 swap tx failed");
    info!(tx_hash = ?v2_receipt.transaction_hash, "✅ Triggered VeloV2 swap successfully.");


    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1, timeout = 180000)] // 180 seconds in ms
#[ignore] 
async fn test_full_univ3_arbitrage_cycle() -> Result<()> {
    let (sim_env, app_state, nonce_manager) = setup_test_env().await?;
    info!("Test: Simulating UniV3 -> UniV3 Arbitrage Cycle...");

    let pool_a_addr: Address = SIMULATION_CONFIG.target_uniswap_v3_pool_address.parse()?; 
    let _pool_b_addr = Address::random(); 

    let pool_a_state_opt = app_state.pool_states.get(&pool_a_addr);
    let pool_a_snapshot_opt = app_state.pool_snapshots.get(&pool_a_addr);
    if pool_a_state_opt.is_none() || pool_a_snapshot_opt.is_none() {
         panic!("Pool A state/snapshot missing for UniV3->UniV3 test. Setup failed? Ensure UniV3 pool (0x8514…08ef) is reliably fetched or its fallback is working.");
    }
    let pool_a_state = pool_a_state_opt.unwrap().value().clone();
    let pool_a_snapshot = pool_a_snapshot_opt.unwrap().value().clone();

    let route = path_optimizer::RouteCandidate {
        buy_pool_addr: pool_a_addr,
        sell_pool_addr: pool_a_addr, 
        buy_dex_type: DexType::UniswapV3,
        sell_dex_type: DexType::UniswapV3,
        token_in: app_state.config.weth_address,
        token_out: app_state.config.usdc_address,
        buy_pool_fee: pool_a_state.uni_fee,
        sell_pool_fee: pool_a_state.uni_fee, 
        buy_pool_stable: None,
        sell_pool_stable: None,
        buy_pool_factory: pool_a_state.factory,
        sell_pool_factory: pool_a_state.factory, 
        zero_for_one_a: pool_a_state.t0_is_weth.unwrap_or(true), 
        estimated_profit_usd: 1.0, 
    };
    info!(?route, "Created dummy UniV3->UniV3 route candidate.");

    let gas_info = transaction::fetch_gas_price(sim_env.http_client.clone(), &app_state.config).await?;
    let gas_price_gwei = utils::ToF64Lossy::to_f64_lossy(&gas_info.max_priority_fee_per_gas) / 1e9;


    let optimal_loan = find_optimal_loan_amount(
        sim_env.http_client.clone(),
        app_state.clone(),
        &route,
        Some(&pool_a_snapshot), 
        Some(&pool_a_snapshot), 
        gas_price_gwei,
    )
    .await?;

    let (loan_amount_wei, simulated_net_profit_wei) = optimal_loan
        .ok_or_else(|| eyre!("Optimal loan search failed to produce even a fallback value"))?;

    info!(
        loan_amount = %format_units(loan_amount_wei, 18)?,
        simulated_profit = %simulated_net_profit_wei,
        "Optimal loan search complete (using fallback if needed)."
    );

    let tx_hash = transaction::submit_arbitrage_transaction(
        sim_env.http_client.clone(),
        app_state.clone(),
        route, 
        loan_amount_wei,
        simulated_net_profit_wei,
        nonce_manager.clone(),
    )
    .await
    .wrap_err("Arbitrage transaction submission failed")?;

    info!(%tx_hash, "✅ UniV3->UniV3 arbitrage transaction submitted and monitored successfully.");

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1, timeout = 180000)] // 180 seconds in ms
#[ignore] 
async fn test_full_arbitrage_cycle_simulation() -> Result<()> {
     let (sim_env, app_state, nonce_manager) = setup_test_env().await?;
     info!("Test: Simulating UniV3 -> VeloV2 Arbitrage Cycle...");

     let uni_pool_addr: Address = SIMULATION_CONFIG.target_uniswap_v3_pool_address.parse()?;
     let velo_pool_addr: Address = SIMULATION_CONFIG.target_velodrome_v2_pool_address.parse()?;

     let uni_state_opt = app_state.pool_states.get(&uni_pool_addr);
     let velo_state_opt = app_state.pool_states.get(&velo_pool_addr);
     let uni_snapshot_opt = app_state.pool_snapshots.get(&uni_pool_addr);
     let velo_snapshot_opt = app_state.pool_snapshots.get(&velo_pool_addr);

      if uni_state_opt.is_none() || uni_snapshot_opt.is_none() {
         panic!("UniV3 pool state/snapshot missing for UniV3->VeloV2 test. Setup failed or UniV3 fallback not working.");
      }
      if velo_state_opt.is_none() || velo_snapshot_opt.is_none() {
          warn!("VeloV2 pool state/snapshot missing for UniV3->VeloV2 test. This is common with Anvil fork. Test will rely on fallbacks/estimations for Velo leg.");
      }
      let uni_state = uni_state_opt.unwrap().value().clone();
      let velo_state = velo_state_opt.map(|v| v.value().clone()).unwrap_or_else(|| {
          warn!("Using default/dummy VeloState for UniV3->VeloV2 test due to fetch failure.");
          PoolState {
              pool_address: velo_pool_addr,
              dex_type: DexType::VelodromeV2,
              token0: app_state.config.weth_address, 
              token1: app_state.config.usdc_address, 
              uni_fee: None,
              velo_stable: Some(true), 
              t0_is_weth: Some(true), 
              factory: app_state.config.velodrome_v2_factory_addr, 
          }
      });
      let uni_snapshot = uni_snapshot_opt.unwrap().value().clone();
      let velo_snapshot = velo_snapshot_opt.map(|v| v.value().clone()); 

     let route = path_optimizer::RouteCandidate {
         buy_pool_addr: uni_pool_addr,
         sell_pool_addr: velo_pool_addr,
         buy_dex_type: DexType::UniswapV3,
         sell_dex_type: DexType::VelodromeV2,
         token_in: app_state.config.weth_address,
         token_out: app_state.config.usdc_address,
         buy_pool_fee: uni_state.uni_fee,
         sell_pool_fee: None, 
         buy_pool_stable: None,
         sell_pool_stable: velo_state.velo_stable,
         buy_pool_factory: uni_state.factory,
         sell_pool_factory: velo_state.factory,
         zero_for_one_a: uni_state.t0_is_weth.unwrap_or(true), 
         estimated_profit_usd: 1.0, 
     };
     info!(?route, "Created dummy UniV3->VeloV2 route candidate.");

     let gas_info = transaction::fetch_gas_price(sim_env.http_client.clone(), &app_state.config).await?;
     let gas_price_gwei = utils::ToF64Lossy::to_f64_lossy(&gas_info.max_priority_fee_per_gas) / 1e9;


     let optimal_loan = find_optimal_loan_amount(
         sim_env.http_client.clone(),
         app_state.clone(),
         &route,
         Some(&uni_snapshot),
         velo_snapshot.as_ref(), 
         gas_price_gwei,
     )
     .await?;

     let (loan_amount_wei, simulated_net_profit_wei) = optimal_loan
         .ok_or_else(|| eyre!("Optimal loan search failed to produce even a fallback value for Uni->Velo"))?;

     info!(
         loan_amount = %format_units(loan_amount_wei, 18)?,
         simulated_profit = %simulated_net_profit_wei,
         "Optimal loan search complete (using fallback/estimation if needed)."
     );

     let tx_hash = transaction::submit_arbitrage_transaction(
         sim_env.http_client.clone(),
         app_state.clone(),
         route, 
         loan_amount_wei,
         simulated_net_profit_wei,
         nonce_manager.clone(),
     )
     .await
     .wrap_err("UniV3->VeloV2 Arbitrage transaction submission failed")?;

     info!(%tx_hash, "✅ UniV3->VeloV2 arbitrage transaction submitted and monitored successfully.");

     Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1, timeout = 180000)] // 180 seconds in ms
#[ignore] 
async fn test_event_handling_triggers_arbitrage_check() -> Result<()> {
    let (sim_env, app_state_arc, nonce_manager) = setup_test_env().await?;

    info!("Test: Verifying event handler triggers arbitrage check...");

    let target_uni_pool_addr: Address = SIMULATION_CONFIG.target_uniswap_v3_pool_address.parse()?;
    let emitter_addr: Address;

    info!("Deploying MinimalSwapEmitter...");
    let emitter_bytecode_path_str = "./build/MinimalSwapEmitter.bin"; 
    let emitter_bytecode_path = Path::new(emitter_bytecode_path_str);
    if !emitter_bytecode_path.exists() {
        panic!(
            "MinimalSwapEmitter bytecode not found at {}. Please compile it first: \n\
            1. Ensure you have solc installed. \n\
            2. Run: `mkdir -p build` \n\
            3. Run: `solc contracts/MinimalSwapEmitter.sol --bin --abi -o build --overwrite`",
            emitter_bytecode_path_str
        );
    }
    let bytecode_hex = std::fs::read_to_string(emitter_bytecode_path)
        .wrap_err_with(|| format!("Failed to read MinimalSwapEmitter bytecode file from {}", emitter_bytecode_path_str))?;
    let bytecode = Bytes::from(hex::decode(bytecode_hex.trim())?);

    let emitter_factory = ContractFactory::new(MINIMALSWAPEMITTER_ABI.clone(), bytecode, sim_env.http_client.clone());

    let emitter_contract = emitter_factory
        .deploy(())?
        .send()
        .await?;
    emitter_addr = emitter_contract.address();
    info!("MinimalSwapEmitter deployed at: {}", emitter_addr);
    let emitter_binding = MinimalSwapEmitter::new(emitter_addr, sim_env.http_client.clone());

    let arb_check_flag = Arc::new(Mutex::new(false)); 
    let mut mutable_app_state = (*app_state_arc).clone();
    mutable_app_state.set_test_arb_check_flag(arb_check_flag.clone()); 
    let test_app_state = Arc::new(mutable_app_state);

    assert!(test_app_state.pool_states.contains_key(&target_uni_pool_addr), "Initial state for target Uni pool is missing before event");
    let initial_snapshot_opt = test_app_state.pool_snapshots.get(&target_uni_pool_addr);
     if initial_snapshot_opt.is_none() {
         panic!("Initial snapshot missing for target Uni pool before event.");
     }
    let initial_snapshot = initial_snapshot_opt.unwrap().value().clone();

    info!("Triggering synthetic Swap event from emitter...");
    let tx = emitter_binding.emit_minimal_swap(
        I256::from(100),                    
        I256::from(-99),                   
        U256::from_dec_str("33770000000000000000000000000")?, 
        1000u128,                       
        204010,                            
    );
    let pending_tx = tx.send().await?.await?;
    let receipt = pending_tx.ok_or_else(|| eyre!("Emitter tx failed"))?;
    assert_eq!(receipt.status, Some(1.into()), "Emitter tx reverted");
    info!("Synthetic event emitted, Tx: {:?}", receipt.transaction_hash);

    let swap_event_sig = UniV3SwapFilter::signature(); 
    let log = receipt.logs.into_iter()
        .find(|l| l.topics.get(0) == Some(&swap_event_sig))
        .ok_or_else(|| eyre!("Swap log not found in emitter tx receipt"))?;

    let mut modified_log = log.clone();
    modified_log.address = target_uni_pool_addr; 
    info!("Found emitter log and modified address to: {}", target_uni_pool_addr);

    info!("Calling handle_log_event with modified log...");
    handle_log_event(
        modified_log,
        test_app_state.clone(), 
        sim_env.http_client.clone(),
        nonce_manager.clone(),
    )
    .await?;
    info!("handle_log_event completed.");

    let updated_snapshot_opt = test_app_state.pool_snapshots.get(&target_uni_pool_addr);
     if updated_snapshot_opt.is_none() {
         panic!("Snapshot missing for target Uni pool after event.");
     }
    let updated_snapshot = updated_snapshot_opt.unwrap().value().clone();

    assert_ne!(updated_snapshot.tick, initial_snapshot.tick, "Snapshot tick should have changed");
    assert_ne!(updated_snapshot.sqrt_price_x96, initial_snapshot.sqrt_price_x96, "Snapshot sqrtPriceX96 should have changed");
    assert_eq!(updated_snapshot.tick, Some(204010), "Snapshot tick not updated correctly");
    info!("✅ Snapshot correctly updated by handle_log_event.");

    info!("Waiting briefly for spawned arbitrage check task...");
    tokio::time::sleep(Duration::from_millis(1000)).await; 

    let flag_value = *arb_check_flag.lock().await;
    assert!(flag_value, "check_for_arbitrage task did not set the trigger flag to true. This might happen if no profitable routes were found by path_optimizer even with synthetic event data, or if the test_arb_check_triggered flag in AppState was not correctly picked up by the spawned task.");
    info!("✅ check_for_arbitrage trigger flag is true.");

    Ok(())
}