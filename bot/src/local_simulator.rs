// bot/src/local_simulator.rs
#![cfg(feature = "local_simulation")]
#![allow(unexpected_cfgs)]

use crate::bindings::{VelodromeV2Pool, SwapRouter, IWETH9};
use crate::bindings::swap_router::ExactInputSingleParams;
use ethers::{
    abi::Abi,
    contract::ContractError, // Added for pattern matching
    prelude::{
        ContractFactory, Http, LocalWallet, Middleware, Provider, SignerMiddleware, StreamExt,
        Ws, *,
    },
    types::{Address, Bytes, Filter, Log, TransactionRequest, TxHash, U256},
    utils::{hex as ethers_hex, format_units, parse_ether}, // Renamed hex to ethers_hex to avoid conflict
};
use eyre::{Result, WrapErr, eyre};
use hex; // For hex::decode used in setup_simulation_environment
use std::{fs, sync::Arc, time::Duration, str::FromStr};
use tracing::{debug, error, info, instrument, warn};
use tokio::time::error::Elapsed;


// --- Simulation Configuration ---
// (Remains unchanged)
#[derive(Debug, Clone)]
pub struct SimulationConfig {
    pub anvil_http_url: &'static str,
    pub anvil_ws_url: &'static str,
    pub anvil_private_key: &'static str,
    pub target_weth_address: &'static str,
    pub target_usdc_address: &'static str,
    pub target_uniswap_v3_pool_address: &'static str,
    pub target_velodrome_v2_pool_address: &'static str,
    pub uniswap_v3_swap_router_address: &'static str,
    pub deploy_executor_in_sim: bool,
    pub executor_bytecode_path: &'static str,
    pub emulated_send_latency_ms: u64,
    pub emulated_read_latency_ms: u64,
}

pub const SIMULATION_CONFIG: SimulationConfig = SimulationConfig {
    anvil_http_url: "http://127.0.0.1:8545",
    anvil_ws_url: "ws://127.0.0.1:8545",
    anvil_private_key: "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
    target_weth_address: "0x4200000000000000000000000000000000000006", // OP WETH
    target_usdc_address: "0x7F5c764cBc14f9669B88837ca1490cCa17c31607", // OP USDC.e
    target_uniswap_v3_pool_address: "0x851492574065EDE975391E141377067943aA08eF", // OP WETH/USDC 0.05%
    target_velodrome_v2_pool_address: "0x207addb05c548f262219f6b50eadff8640ed6488", // OP WETH/USDC Stable
    uniswap_v3_swap_router_address: "0x68b3465833fb72A70ecDF485E0e4C7bD8665Fc45", // Optimism SwapRouter02
    deploy_executor_in_sim: true,
    executor_bytecode_path: "./build/ArbitrageExecutor.bin",
    emulated_send_latency_ms: 10,
    emulated_read_latency_ms: 5,
};

pub type AnvilClient = SignerMiddleware<Provider<Http>, LocalWallet>;
pub type AnvilWsProvider = Provider<Ws>;

#[derive(Debug)]
pub struct SimEnv {
    pub http_client: Arc<AnvilClient>,
    pub ws_provider: Arc<AnvilWsProvider>,
    pub config: SimulationConfig,
    pub wallet_address: Address,
    pub executor_address: Option<Address>,
}

// Make this constant public
pub const VELO_ROUTER_IMPL_ADDR_FOR_SIM: &str = "0xa062aE8A9c5e11aaA026fc2670B0D65cCc8B2858";
#[cfg(feature = "local_simulation")] 
pub const PAIR_DOES_NOT_EXIST_SELECTOR_STR: &str = "9a73ab46";


#[instrument(skip_all, name = "sim_setup")]
pub async fn setup_simulation_environment() -> Result<SimEnv> {
    info!("Setting up simulation environment...");
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
    let http_client = Arc::new(SignerMiddleware::new(http_provider.clone(), wallet));
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

        apply_send_latency().await;

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
        config: SIMULATION_CONFIG.clone(),
        wallet_address,
        executor_address,
    })
}

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
    sim_env: &SimEnv,
    pool_addr: Address,
    pool_binding: &VelodromeV2Pool<AnvilClient>,
    amount0_out: U256,
    amount1_out: U256,
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


/// Triggers a Uniswap V3 swap through the SwapRouter.
/// Wraps ETH to WETH, approves router, then calls exactInputSingle.
#[instrument(skip(sim_env), fields(amount_eth_in = %format_units(amount_eth_in, "ether").unwrap_or_default()))]
pub async fn trigger_v3_swap_via_router(
    sim_env: &SimEnv,
    amount_eth_in: U256, 
    token_out_addr: Address,
    pool_fee: u32,
    recipient: Address,
    sqrt_price_limit_x96: U256,
) -> Result<TransactionReceipt> {
    info!("Triggering V3 swap via SwapRouter: WETH -> {}...", token_out_addr);

    let client = sim_env.http_client.clone();
    let weth_addr = Address::from_str(sim_env.config.target_weth_address)?;
    let router_addr = Address::from_str(sim_env.config.uniswap_v3_swap_router_address)?;

    let weth_contract = IWETH9::new(weth_addr, client.clone());
    let router_contract = SwapRouter::new(router_addr, client.clone());

    info!("Depositing {} ETH to WETH contract {}...", format_units(amount_eth_in, "ether")?, weth_addr);
    let deposit_tx_call = weth_contract.deposit().value(amount_eth_in);
    let pending_deposit_tx = deposit_tx_call.send().await.wrap_err("WETH deposit send failed")?;
    let deposit_receipt = pending_deposit_tx.await.wrap_err("WETH deposit confirmation failed")?
        .ok_or_else(|| eyre!("WETH deposit tx not mined"))?;
    if deposit_receipt.status != Some(1.into()) {
        eyre::bail!("WETH deposit transaction reverted. Receipt: {:?}", deposit_receipt);
    }
    info!("✅ WETH deposited. Tx: {:?}", deposit_receipt.transaction_hash);
    
    let weth_balance_after_deposit = weth_contract.balance_of(sim_env.wallet_address).call().await?;
    info!("Wallet WETH balance after deposit: {}", format_units(weth_balance_after_deposit, "ether")?);
    
    let amount_weth_in = amount_eth_in;
    if weth_balance_after_deposit < amount_weth_in {
        eyre::bail!("Insufficient WETH balance after deposit. Expected at least {}, got {}", amount_weth_in, weth_balance_after_deposit);
    }

    info!("Approving SwapRouter {} to spend {} WETH from {}...", router_addr, format_units(amount_weth_in, "ether")?, sim_env.wallet_address);
    let approve_tx_call = weth_contract.approve(router_addr, amount_weth_in);
    let pending_approve_tx = approve_tx_call.send().await.wrap_err("WETH approval send failed")?;
    let approve_receipt = pending_approve_tx.await.wrap_err("WETH approval confirmation failed")?
        .ok_or_else(|| eyre!("WETH approval tx not mined"))?;
    if approve_receipt.status != Some(1.into()) {
        eyre::bail!("WETH approval for SwapRouter transaction reverted. Receipt: {:?}", approve_receipt);
    }
    info!("✅ WETH approved for SwapRouter. Tx: {:?}", approve_receipt.transaction_hash);

    let allowance_after_approve = weth_contract.allowance(sim_env.wallet_address, router_addr).call().await?;
    info!("Router WETH allowance after approval: {}", format_units(allowance_after_approve, "ether")?);
    if allowance_after_approve < amount_weth_in {
        eyre::bail!("Insufficient WETH allowance for router. Expected {}, got {}", amount_weth_in, allowance_after_approve);
    }

    let current_block = client.get_block_number().await?;
    let block_timestamp = client.get_block(current_block).await?.ok_or_else(|| eyre!("Failed to get current block"))?.timestamp;
    let deadline = block_timestamp + U256::from(600);

    let params = ExactInputSingleParams {
        token_in: weth_addr,
        token_out: token_out_addr,
        fee: pool_fee,
        recipient,
        deadline,
        amount_in: amount_weth_in,
        amount_out_minimum: U256::zero(),
        sqrt_price_limit_x96,
    };
    info!("Preparing to call SwapRouter.exactInputSingle with params: {:?}", params);

    match router_contract.exact_input_single(params.clone()).value(0).call().await {
        Ok(simulated_amount_out) => {
            info!("✅ SwapRouter.exactInputSingle CALL successful (simulated). Expected out: {}", simulated_amount_out);
        }
        Err(e) => {
            warn!("⚠️ SwapRouter.exactInputSingle CALL failed (simulated): {:?}", e);
             if let ContractError::Revert(data) = e { 
                 warn!("   Raw revert data: 0x{}", ethers_hex::encode(data)); 
             }
        }
    }
    
    let gas_limit_for_swap = U256::from(800_000);
    info!("Attempting SwapRouter.exactInputSingle with gas limit: {}", gas_limit_for_swap);

    let swap_call_tx = router_contract.exact_input_single(params.clone()).gas(gas_limit_for_swap);
    
    let pending_swap_tx = swap_call_tx.send().await.wrap_err_with(|| format!("SwapRouter.exactInputSingle send failed. Params: {:?}", params))?;
    let swap_receipt = pending_swap_tx.await.wrap_err("SwapRouter.exactInputSingle confirmation failed")?
        .ok_or_else(|| eyre!("SwapRouter.exactInputSingle tx not mined"))?;
    
    info!("✅ SwapRouter.exactInputSingle transaction sent and confirmed. Tx: {:?}", swap_receipt.transaction_hash);
    Ok(swap_receipt)
}


#[instrument(skip(sim_env))]
pub async fn fetch_simulation_data(sim_env: &SimEnv) -> Result<()> {
    info!("Fetching simulation data from Anvil...");
    apply_read_latency().await;
    let weth_addr: Address = sim_env.config.target_weth_address.parse()?;
    let weth_contract = crate::bindings::IERC20::new(weth_addr, sim_env.http_client.clone());
    let balance = weth_contract.balance_of(sim_env.wallet_address).call().await?;
    info!("Signer WETH Balance on Anvil: {}", ethers::utils::format_ether(balance));
    let pool_addr: Address = sim_env.config.target_uniswap_v3_pool_address.parse()?;
    if pool_addr != Address::zero() {
        let pool_contract = crate::bindings::UniswapV3Pool::new(pool_addr, sim_env.http_client.clone());
        let slot0 = pool_contract.slot_0().call().await?;
        info!(pool=%pool_addr, ?slot0, "Fetched UniV3 Pool slot0 from Anvil");
    } else {
        warn!("Skipping UniV3 fetch: target_uniswap_v3_pool_address is zero.");
    }
    info!("Anvil data fetch complete.");
    Ok(())
}

#[instrument(skip(sim_env))]
pub async fn run_simulation_scenario(sim_env: Arc<SimEnv>) -> Result<()> {
    info!("Starting Anvil simulation scenario...");
    fetch_simulation_data(&sim_env).await?;

    let ws_provider = sim_env.ws_provider.clone();
    let filter = Filter::new()
        .address(vec![
            sim_env.config.target_uniswap_v3_pool_address.parse::<Address>()?,
            sim_env.config.target_velodrome_v2_pool_address.parse::<Address>()?,
        ])
        .topic0(vec![
            *crate::UNI_V3_SWAP_TOPIC,
            *crate::VELO_AERO_SWAP_TOPIC,
        ]);

    info!("Subscribing to Anvil swap events...");
    let ws_client = ws_provider.inner();
    let mut stream: SubscriptionStream<Ws, Log> = ws_client.subscribe_logs(&filter).await?;
    info!("✅ Subscribed to Anvil swap events.");

    info!("Simulating an external swap on Anvil using SwapRouter...");
    let amount_to_swap_eth = parse_ether("0.01")?;
    let usdc_address = Address::from_str(sim_env.config.target_usdc_address)?;
    let pool_fee_for_swap = 500;

    match trigger_v3_swap_via_router(
        &sim_env, 
        amount_to_swap_eth,
        usdc_address,
        pool_fee_for_swap,
        sim_env.wallet_address,
        U256::zero(), 
    ).await {
        Ok(receipt) => {
            info!("✅ External swap via router successful. Tx: {:?}", receipt.transaction_hash);
        }
        Err(e) => {
            error!("Trigger V3 swap via router on Anvil failed: {:?}", e);
        }
    }


    info!("Waiting for simulated event from Anvil...");
    let timeout_result: Result<Option<Log>, Elapsed> =
        tokio::time::timeout(Duration::from_secs(30), stream.next()).await;

    match timeout_result {
        Ok(Some(log)) => {
            info!("Received simulated log from Anvil: {:?}", log.transaction_hash);
            apply_read_latency().await;
            warn!("Actual bot logic simulation based on received Anvil event is NOT YET IMPLEMENTED in run_simulation_scenario.");
        }
        Ok(None) => {
            warn!("Anvil event stream ended gracefully (or closed immediately?).");
        }
        Err(_elapsed) => {
            warn!("Timeout waiting for simulated event from Anvil.");
        }
    }

    info!("Anvil simulation scenario finished.");
    Ok(())
}