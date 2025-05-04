// bot/src/local_simulator.rs
#![cfg(feature = "local_simulation")]
#![allow(unexpected_cfgs)] // Keep this allow

use crate::bindings::{UniswapV3Pool, VelodromeV2Pool}; // Use crate::
use ethers::{
    abi::Abi,
    prelude::{
        ContractFactory, Http, LocalWallet, Middleware, Provider, SignerMiddleware, StreamExt,
        Ws, *,
    },
    types::{Address, Bytes, Filter, Log, TransactionRequest, TxHash, U256, I256},
    utils::{hex},
};
use eyre::{Result, WrapErr, eyre}; // Keep Result and eyre
use std::{fs, sync::Arc, time::Duration};
use tracing::{debug, error, info, instrument, warn};
use tokio::time::error::Elapsed;


// --- Simulation Configuration ---
// Make the struct public to resolve the private_interfaces warning
#[derive(Debug, Clone)]
pub struct SimulationConfig { // Changed to pub struct
    // Make fields public so tests can access them via sim_env.config.field_name
    pub anvil_http_url: &'static str,
    pub anvil_ws_url: &'static str,
    pub anvil_private_key: &'static str,
    pub target_weth_address: &'static str,
    pub target_usdc_address: &'static str, // Keep even if warned unused
    pub target_uniswap_v3_pool_address: &'static str,
    pub target_velodrome_v2_pool_address: &'static str,
    pub deploy_executor_in_sim: bool,
    pub executor_bytecode_path: &'static str,
    pub emulated_send_latency_ms: u64,
    pub emulated_read_latency_ms: u64,
}

// Made this public so tests can potentially access it if needed,
// although direct access might still be tricky.
// Better approach: Access through SimEnv config field.
pub const SIMULATION_CONFIG: SimulationConfig = SimulationConfig {
    anvil_http_url: "http://127.0.0.1:8545",
    anvil_ws_url: "ws://127.0.0.1:8545",
    anvil_private_key: "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
    target_weth_address: "0x4200000000000000000000000000000000000006", // OP WETH
    target_usdc_address: "0x7F5c764cBc14f9669B88837ca1490cCa17c31607", // OP USDC.e
    target_uniswap_v3_pool_address: "0x851492574065EDE975391E141377067943aA08eF", // OP WETH/USDC 0.05%
    target_velodrome_v2_pool_address: "0x207addb05c548f262219f6b50eadff8640ed6488", // OP WETH/USDC Stable
    deploy_executor_in_sim: true,
    executor_bytecode_path: "./build/ArbitrageExecutor.bin",
    emulated_send_latency_ms: 10,
    emulated_read_latency_ms: 5,
};

// Keep types public for use in tests
pub type AnvilClient = SignerMiddleware<Provider<Http>, LocalWallet>;
pub type AnvilWsProvider = Provider<Ws>;

// Keep SimEnv struct public
#[derive(Debug)] // Automatically derived Debug is fine
pub struct SimEnv {
    pub http_client: Arc<AnvilClient>,
    pub ws_provider: Arc<AnvilWsProvider>,
    // Make SimulationConfig public so the field can be public
    pub config: SimulationConfig,
    pub wallet_address: Address,
    pub executor_address: Option<Address>,
}


#[instrument(skip_all, name = "sim_setup")]
pub async fn setup_simulation_environment() -> Result<SimEnv> {
    info!("Setting up simulation environment...");
    // Use the constant defined above directly
    let http_provider = Provider::<Http>::try_from(SIMULATION_CONFIG.anvil_http_url)
        .wrap_err("Failed to create HTTP provider")?;
    let ws_connect_timeout = Duration::from_secs(10);
    let ws_provider = tokio::time::timeout(
        ws_connect_timeout,
        Provider::<Ws>::connect(SIMULATION_CONFIG.anvil_ws_url)
    ).await
        .map_err(|_| eyre!("Timeout connecting WS after {}s", ws_connect_timeout.as_secs()))?
        .wrap_err("WS connection failed")?;

    let chain_id = http_provider.get_chainid().await?.as_u64();
    let wallet = SIMULATION_CONFIG.anvil_private_key.parse::<LocalWallet>()?
        .with_chain_id(chain_id);
    let wallet_address = wallet.address();
    let http_client = Arc::new(SignerMiddleware::new(http_provider, wallet));
    let ws_provider_arc = Arc::new(ws_provider);

    info!("Connected to Anvil (Chain ID: {}, Wallet: {:?})", chain_id, wallet_address);

    let executor_address = if SIMULATION_CONFIG.deploy_executor_in_sim {
        info!("Deploying Executor contract via Anvil...");
        let bytecode_hex = fs::read_to_string(SIMULATION_CONFIG.executor_bytecode_path)
            .wrap_err("Failed to read executor bytecode")?;
        let cleaned = bytecode_hex.trim().trim_start_matches("0x");
        let bytecode = hex::decode(cleaned)
            .wrap_err("Failed to decode executor bytecode")?;
        let bytes = Bytes::from(bytecode);
        let factory = ContractFactory::new(Abi::default(), bytes, http_client.clone());
        let deployer = factory.deploy(())
            .map_err(|e| eyre!("Executor deploy construction failed: {}", e))?;

        apply_send_latency().await; // Apply latency before sending

        let contract = deployer.send().await
            .wrap_err("Executor deploy send transaction failed")?;
        let addr = contract.address();
        info!("✅ Executor deployed to Anvil at: {:?}", addr);
        Some(addr)
    } else {
        warn!("Executor deployment skipped as per SIMULATION_CONFIG.");
        None
    };

    Ok(SimEnv {
        http_client,
        ws_provider: ws_provider_arc,
        config: SIMULATION_CONFIG.clone(), // Clone the config into the env struct
        wallet_address,
        executor_address,
    })
}

// Keep latency helpers internal if only used here
async fn apply_send_latency() {
    let l = SIMULATION_CONFIG.emulated_send_latency_ms;
    if l > 0 {
        debug!("Applying simulated SEND Latency: {}ms", l);
        tokio::time::sleep(Duration::from_millis(l)).await;
    }
}
async fn apply_read_latency() {
    let l = SIMULATION_CONFIG.emulated_read_latency_ms;
    if l > 0 {
        debug!("Applying simulated READ Latency: {}ms", l);
        tokio::time::sleep(Duration::from_millis(l)).await;
    }
}

#[instrument(skip(sim_env, pool_binding), fields(pool_addr=%pool_addr))]
pub async fn trigger_v2_swap(
    sim_env: &SimEnv, // Use borrow instead of Arc if possible directly
    pool_addr: Address,
    pool_binding: &VelodromeV2Pool<AnvilClient>, // Use crate::bindings
    amount0_out: U256,
    amount1_out: U256, // Expects U256 directly
    to_address: Address,
    data: Bytes,
) -> Result<TxHash> {
    info!(%amount0_out, %amount1_out, "Triggering V2 swap via Anvil...");
    warn!("V2 swap trigger assumes prerequisites (like token approvals if needed) are met in Anvil state.");

    let swap_call = pool_binding.swap(amount0_out, amount1_out, to_address, data);
    let tx_request: TransactionRequest = swap_call.tx.clone().into();

    apply_send_latency().await;

    let pending_tx = sim_env.http_client.send_transaction(tx_request, None).await
        .wrap_err("Send V2 swap transaction failed")?;
    let tx_hash = *pending_tx;
    info!(%tx_hash, "V2 Swap transaction sent to Anvil.");
    Ok(tx_hash)
}


#[instrument(skip(sim_env, pool_binding), fields(pool_addr=%pool_addr))]
pub async fn trigger_v3_swap(
    sim_env: &SimEnv, // Use borrow
    pool_addr: Address,
    pool_binding: &UniswapV3Pool<AnvilClient>, // Use crate::bindings
    recipient: Address,
    zero_for_one: bool,
    amount_specified: I256,
    sqrt_price_limit_x96: U256,
    data: Bytes,
) -> Result<TxHash> {
    info!(%recipient, %zero_for_one, %amount_specified, "Triggering V3 swap via Anvil...");
    warn!("V3 swap trigger assumes prerequisites met AND ABI contains 'swap' function.");

    // Create ContractCall object by calling the method on the binding
    let swap_call = pool_binding.swap(
        recipient,
        zero_for_one,
        amount_specified,
        sqrt_price_limit_x96,
        data,
    );
    // Access the internal transaction field (.tx) and convert it
    let tx_request: TransactionRequest = swap_call.tx.clone().into();

    apply_send_latency().await;

    let pending_tx = sim_env
        .http_client
        .send_transaction(tx_request, None)
        .await
        .wrap_err("Send V3 swap transaction failed")?;
    let tx_hash = *pending_tx;
    info!(%tx_hash, "V3 Swap transaction sent to Anvil.");
    Ok(tx_hash)
}

#[instrument(skip(sim_env))]
pub async fn fetch_simulation_data(sim_env: &SimEnv) -> Result<()> {
    info!("Fetching simulation data from Anvil...");
    apply_read_latency().await;

    // Fetch WETH balance
    let weth_addr: Address = sim_env.config.target_weth_address.parse()?;
    let weth_contract = crate::bindings::IERC20::new(weth_addr, sim_env.http_client.clone());
    let balance = weth_contract.balance_of(sim_env.wallet_address).call().await?;
    info!("Signer WETH Balance on Anvil: {}", ethers::utils::format_ether(balance));

    // Fetch UniV3 pool state if address is valid
    let pool_addr: Address = sim_env.config.target_uniswap_v3_pool_address.parse()?;
    if pool_addr != Address::zero() {
        let pool_contract = crate::bindings::UniswapV3Pool::new(pool_addr, sim_env.http_client.clone());
        let slot0 = pool_contract.slot_0().call().await?;
        info!(pool=%pool_addr, ?slot0, "Fetched UniV3 Pool slot0 from Anvil");
    } else {
        warn!("Skipping UniV3 fetch: target_uniswap_v3_pool_address is zero.");
    }

    // TODO: Add similar fetch for VeloV2 pool state if needed for basic checks

    info!("Anvil data fetch complete.");
    Ok(())
}

#[instrument(skip(sim_env))]
pub async fn run_simulation_scenario(sim_env: Arc<SimEnv>) -> Result<()> {
    info!("Starting Anvil simulation scenario...");
    fetch_simulation_data(&sim_env).await?; // Fetch initial data

    // Subscribe to swap events on Anvil WS
    let ws_provider = sim_env.ws_provider.clone();
    let filter = Filter::new()
        .address(vec![
            sim_env.config.target_uniswap_v3_pool_address.parse::<Address>()?,
            sim_env.config.target_velodrome_v2_pool_address.parse::<Address>()?,
        ])
        .topic0(vec![
            *crate::UNI_V3_SWAP_TOPIC, // Use crate::
            *crate::VELO_AERO_SWAP_TOPIC, // Use crate::
        ]);

    info!("Subscribing to Anvil swap events...");
    let ws_client = ws_provider.inner(); // Get inner Ws client
    let mut stream: SubscriptionStream<Ws, Log> = ws_client.subscribe_logs(&filter).await?;
    info!("✅ Subscribed to Anvil swap events.");

    // --- Simulate an external swap to trigger bot logic ---
    info!("Simulating an external swap on Anvil...");
    let uni_pool_addr: Address = sim_env.config.target_uniswap_v3_pool_address.parse()?;
    if uni_pool_addr != Address::zero() {
        // Use the SimEnv directly, not the Arc, as trigger_v3_swap takes a borrow
        let uni_pool = crate::bindings::UniswapV3Pool::new(uni_pool_addr, sim_env.http_client.clone());
        let amount_in = ethers::utils::parse_ether(0.1)?; // Simulate 0.1 WETH swap
        let _ = trigger_v3_swap(
            &sim_env, // Pass borrow of SimEnv from Arc
            uni_pool_addr,
            &uni_pool,
            sim_env.wallet_address, // Recipient is self for test
            true, // WETH -> USDC
            I256::from_raw(amount_in), // Amount specified in
            U256::zero(), // No price limit
            Bytes::new(), // No extra data
        )
        .await
        .map_err(|e| error!("Trigger V3 swap on Anvil failed: {:?}", e)); // Log error, don't stop test
        // If the known ABI issue persists, this might log an error but the test continues.
    } else {
        warn!("Skipping Anvil UniV3 trigger: address is zero.");
    }

    info!("Waiting for simulated event from Anvil...");

    let timeout_result: Result<Option<Log>, Elapsed> = // Corrected type hint
        tokio::time::timeout(Duration::from_secs(30), stream.next()).await;

    match timeout_result {
        Ok(Some(log)) => { // Stream yielded Option<Log>, match directly
            info!("Received simulated log from Anvil: {:?}", log.transaction_hash);
            apply_read_latency().await; // Apply latency
            warn!("Actual bot logic simulation based on received Anvil event is NOT YET IMPLEMENTED in run_simulation_scenario.");
            // TODO: Here you would plug in the call to your bot's event handler logic
            // e.g., crate::event_handler::handle_log_event(log, app_state_arc, client_arc, nonce_manager_arc).await;
        }
        Ok(None) => { // Stream closed gracefully
            warn!("Anvil event stream ended gracefully (or closed immediately?).");
        }
        Err(_elapsed) => { // Timeout occurred
            warn!("Timeout waiting for simulated event from Anvil.");
        }
    }

    info!("Anvil simulation scenario finished.");
    Ok(())
}