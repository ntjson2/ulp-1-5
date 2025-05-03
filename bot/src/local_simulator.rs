// bot/src/local_simulator.rs

//! # Local Simulation Framework (Connects to External Anvil)
// ... comments ...

#![cfg(feature = "local_simulation")]
#![allow(unexpected_cfgs)]

use ethers::{
    abi::Abi,
    prelude::{
        ContractFactory, Http, LocalWallet, Middleware, Provider, SignerMiddleware, StreamExt,
        Ws, *,
    },
    // FIX: Remove unused ProviderError import
    // providers::ProviderError,
    // FIX: Removed Log import as it's inferred in the match
    types::{Address, Bytes, Filter, Log, TransactionRequest, TxHash, U256, I256},
    utils::{hex},
};
use eyre::{Result, WrapErr, eyre};
// FIX: Remove unused std::result import
use std::{fs, sync::Arc, time::Duration};
use tracing::{debug, error, info, instrument, warn};
use tokio::time::error::Elapsed;


// --- Simulation Configuration ---
// ... (remains the same) ...
#[derive(Debug, Clone)] struct SimulationConfig { anvil_http_url: &'static str, anvil_ws_url: &'static str, anvil_private_key: &'static str, target_weth_address: &'static str, target_usdc_address: &'static str, target_uniswap_v3_pool_address: &'static str, target_velodrome_v2_pool_address: &'static str, deploy_executor_in_sim: bool, executor_bytecode_path: &'static str, emulated_send_latency_ms: u64, emulated_read_latency_ms: u64, }
const SIMULATION_CONFIG: SimulationConfig = SimulationConfig { anvil_http_url: "http://127.0.0.1:8545", anvil_ws_url: "ws://127.0.0.1:8545", anvil_private_key: "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80", target_weth_address: "0x4200000000000000000000000000000000000006", target_usdc_address: "0x7F5c764cBc14f9669B88837ca1490cCa17c31607", target_uniswap_v3_pool_address: "0x851492574065EDE975391E141377067943aA08eF", target_velodrome_v2_pool_address: "0x207addb05c548f262219f6b50eadff8640ed6488", deploy_executor_in_sim: true, executor_bytecode_path: "./build/ArbitrageExecutor.bin", emulated_send_latency_ms: 10, emulated_read_latency_ms: 5, };
type AnvilClient = SignerMiddleware<Provider<Http>, LocalWallet>;
type AnvilWsProvider = Provider<Ws>;
pub struct SimEnv { pub http_client: Arc<AnvilClient>, pub ws_provider: Arc<AnvilWsProvider>, pub config: SimulationConfig, pub wallet_address: Address, pub executor_address: Option<Address>, }

#[instrument(skip_all, name = "sim_setup")]
pub async fn setup_simulation_environment() -> Result<SimEnv> { /* ... remains same ... */ info!("Setting up..."); let http_provider = Provider::<Http>::try_from(SIMULATION_CONFIG.anvil_http_url).wrap_err("HTTP")?; let ws_connect_timeout = Duration::from_secs(10); let ws_provider = tokio::time::timeout(ws_connect_timeout, Provider::<Ws>::connect(SIMULATION_CONFIG.anvil_ws_url)).await.map_err(|_| eyre!("Timeout connecting WS after {}s", ws_connect_timeout.as_secs()))?.wrap_err("WS connect")?; let chain_id = http_provider.get_chainid().await?.as_u64(); let wallet = SIMULATION_CONFIG.anvil_private_key.parse::<LocalWallet>()?.with_chain_id(chain_id); let wallet_address = wallet.address(); let http_client = Arc::new(SignerMiddleware::new(http_provider, wallet)); let ws_provider_arc = Arc::new(ws_provider); info!("Connected (Chain ID: {}, Wallet: {:?})", chain_id, wallet_address); let executor_address = if SIMULATION_CONFIG.deploy_executor_in_sim { info!("Deploying Executor..."); let bytecode_hex = fs::read_to_string(SIMULATION_CONFIG.executor_bytecode_path).wrap_err("Read bytecode")?; let cleaned = bytecode_hex.trim().trim_start_matches("0x"); let bytecode = hex::decode(cleaned).wrap_err("Decode bytecode")?; let bytes = Bytes::from(bytecode); let factory = ContractFactory::new(Abi::default(), bytes, http_client.clone()); let deployer = factory.deploy(()).map_err(|e| eyre!("Deploy construct: {}", e))?; apply_send_latency().await; let contract = deployer.send().await.wrap_err("Executor deploy send")?; let addr = contract.address(); info!("✅ Executor deployed: {:?}", addr); Some(addr) } else { warn!("Executor deployment skipped."); None }; Ok(SimEnv { http_client, ws_provider: ws_provider_arc, config: SIMULATION_CONFIG.clone(), wallet_address, executor_address }) }
async fn apply_send_latency() { /* ... */ let l = SIMULATION_CONFIG.emulated_send_latency_ms; if l > 0 { debug!("SEND Latency: {}ms", l); tokio::time::sleep(Duration::from_millis(l)).await; } }
async fn apply_read_latency() { /* ... */ let l = SIMULATION_CONFIG.emulated_read_latency_ms; if l > 0 { debug!("READ Latency: {}ms", l); tokio::time::sleep(Duration::from_millis(l)).await; } }

#[instrument(skip(sim_env, pool_binding), fields(pool_addr=%pool_addr))]
pub async fn trigger_v2_swap( sim_env: &SimEnv, pool_addr: Address, pool_binding: &crate::bindings::VelodromeV2Pool<AnvilClient>, amount0_out: U256, amount1_out: U256, to_address: Address, data: Bytes, ) -> Result<TxHash> { /* ... remains same ... */ info!(%amount0_out, %amount1_out, "Triggering V2 swap..."); warn!("V2 swap trigger assumes prerequisites met."); let swap_call = pool_binding.swap(amount0_out, amount1_out, to_address, data); let tx_request: TransactionRequest = swap_call.tx.clone().into(); apply_send_latency().await; let pending_tx = sim_env.http_client.send_transaction(tx_request, None).await.wrap_err("Send V2 swap")?; let tx_hash = *pending_tx; info!(%tx_hash, "V2 Swap transaction sent."); Ok(tx_hash) }

// FIX E0599: The root cause is likely the missing 'swap' function in the UniswapV3Pool ABI JSON file.
//            The Rust code below is technically correct IF the ABI contains the function.
//            Ensure the ABI JSON has the 'swap' function definition added.
#[instrument(skip(sim_env, pool_binding), fields(pool_addr=%pool_addr))]
pub async fn trigger_v3_swap(
    sim_env: &SimEnv,
    pool_addr: Address,
    pool_binding: &crate::bindings::UniswapV3Pool<AnvilClient>, // Binding type
    recipient: Address,
    zero_for_one: bool,
    amount_specified: I256,
    sqrt_price_limit_x96: U256,
    data: Bytes,
) -> Result<TxHash> {
    info!(%recipient, %zero_for_one, %amount_specified, "Triggering V3 swap...");
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
        .wrap_err("Send V3 swap")?;
    let tx_hash = *pending_tx;
    info!(%tx_hash, "V3 Swap transaction sent.");
    Ok(tx_hash)
}

#[instrument(skip(sim_env))]
pub async fn fetch_simulation_data(sim_env: &SimEnv) -> Result<()> { /* ... remains same ... */ info!("Fetching simulation data..."); apply_read_latency().await; let weth_addr: Address = sim_env.config.target_weth_address.parse()?; let weth_contract = crate::bindings::IERC20::new(weth_addr, sim_env.http_client.clone()); let balance = weth_contract.balance_of(sim_env.wallet_address).call().await?; info!("Signer WETH Balance: {}", ethers::utils::format_ether(balance)); let pool_addr: Address = sim_env.config.target_uniswap_v3_pool_address.parse()?; if pool_addr != Address::zero() { let pool_contract = crate::bindings::UniswapV3Pool::new(pool_addr, sim_env.http_client.clone()); let slot0 = pool_contract.slot_0().call().await?; info!(pool=%pool_addr, ?slot0, "Fetched UniV3 Pool slot0"); } else { warn!("Skip UniV3 fetch: zero address."); } info!("Fetch complete."); Ok(()) }

#[instrument(skip(sim_env))]
pub async fn run_simulation_scenario(sim_env: Arc<SimEnv>) -> Result<()> {
    info!("Starting simulation scenario..."); fetch_simulation_data(&sim_env).await?;
    let ws_provider = sim_env.ws_provider.clone();
    let filter = Filter::new().address(vec![ sim_env.config.target_uniswap_v3_pool_address.parse::<Address>()?, sim_env.config.target_velodrome_v2_pool_address.parse::<Address>()?, ]).topic0(vec![ *crate::UNI_V3_SWAP_TOPIC, *crate::VELO_AERO_SWAP_TOPIC, ]);
    info!("Subscribing...");
    let ws_client = ws_provider.inner();
    let mut stream: SubscriptionStream<Ws, Log> = ws_client.subscribe_logs(&filter).await?;
    info!("Subscribed.");
    info!("Simulating external swap..."); let uni_pool_addr: Address = sim_env.config.target_uniswap_v3_pool_address.parse()?;
    if uni_pool_addr != Address::zero() {
        // FIX E0599: Temporarily comment out the call that fails due to missing ABI function
        // TODO: Uncomment this block after fixing the UniswapV3Pool.json ABI and regenerating bindings.
        /*
        let uni_pool = crate::bindings::UniswapV3Pool::new(uni_pool_addr, sim_env.http_client.clone());
        let amount_in = ethers::utils::parse_ether(0.1)?;
        let _ = trigger_v3_swap(
            &sim_env,
            uni_pool_addr,
            &uni_pool,
            sim_env.wallet_address,
            true,
            I256::from_raw(amount_in),
            U256::zero(),
            Bytes::new(),
        )
        .await
        .map_err(|e| error!("Trigger V3 swap failed: {:?}", e));
        */
        warn!("Skipping V3 swap trigger due to known ABI/binding issue (E0599).");
    } else { warn!("Skip UniV3 trigger."); }
    info!("Waiting for simulated events...");

    // FIX E0308: Corrected type hint and pattern matching for stream result
    // Type is Result<Option<Log>, Elapsed> because stream directly yields Option<Log>
    let timeout_result: Result<Option<Option<Log>>, Elapsed> = // Removed inner Result
        tokio::time::timeout(Duration::from_secs(30), stream.next()).await;

    match timeout_result {
        Ok(Some(log_option)) => { // Stream yielded Option<Log>
             // Correct pattern: Match the Option<Log> directly
             if let Some(log) = log_option {
                 info!("Received simulated log: {:?}", log.transaction_hash);
                 apply_read_latency().await;
                 warn!("Actual bot logic simulation based on event is NOT YET IMPLEMENTED.");
             } else {
                 // Stream returned None, meaning it closed gracefully.
                 warn!("Simulated event stream ended gracefully.");
             }
        }
        Ok(None) => { // Should not happen with timeout around stream.next() unless stream closes immediately
             warn!("Simulated event stream returned None immediately (already closed?).");
        }
        Err(_elapsed) => { // Timeout occurred
            warn!("Timeout waiting for simulated event.");
        }
    }

    info!("Simulation scenario finished."); Ok(())
}