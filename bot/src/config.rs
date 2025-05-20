// bot/src/config.rs

use ethers::types::{Address, U256};
use eyre::{Result, WrapErr, eyre};
use std::env;
use dotenv::dotenv;
use serde::Deserialize;
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    // Network & Keys
    pub ws_rpc_url: String,
    pub http_rpc_url: String,
    pub local_private_key: String,
    pub chain_id: Option<u64>, // Kept as Option from previous state
    pub arb_executor_address: Option<Address>, // Kept as Option from previous state

    // Contract Addresses (Core - Optimism/Base)
    pub weth_address: Address,
    pub usdc_address: Address,
    pub weth_decimals: Option<u8>, // Made Option based on path_optimizer changes
    pub usdc_decimals: Option<u8>, // Made Option based on path_optimizer changes

    // --- DEX Expansion ---
    pub velodrome_router_address: Option<Address>,
    pub uniswap_v3_factory_address: Option<Address>,
    pub uniswap_v3_quoter_v2_address: Option<Address>,
    pub uniswap_v3_router_address: Option<Address>,

    // Simulation & Testing Fields
    pub initial_block_history_to_scan: Option<u64>,
    pub max_block_range_per_query: Option<u64>,
    pub log_level: Option<String>,
    pub use_local_anvil_node: Option<bool>,
    pub local_anvil_port: Option<u16>,
    pub local_anvil_chain_id: Option<u64>,
    pub local_anvil_gas_limit: Option<u64>,
    pub local_anvil_block_time: Option<u64>,
    pub local_anvil_fork_url: Option<String>,
    pub local_anvil_fork_block_number: Option<u64>,

    // Profitability & Slippage Control
    pub min_profit_threshold_wei: Option<U256>,
    pub max_gas_price_gwei: Option<f64>,
    pub max_slippage_bps: Option<u16>,

    // Transaction Submission Options
    pub transaction_submission_retries: Option<usize>,
    pub gas_estimation_multiplier: Option<f64>,

    // Optimization Options
    pub optimal_loan_search_iterations: Option<u32>,

    // Dry Run Option
    pub dry_run: bool,

    // Fields from simulation.rs errors
    pub simulation_gas_price_gwei:          Option<f64>,
    pub simulation_timeout_seconds:         Option<u64>,
    pub simulation_gas_limit_default:       Option<u64>,
    pub simulation_min_gas_limit:           Option<u64>, // Renamed from min_flashloan_gas_limit
    pub dynamic_sizing_velo_percentage:     Option<u64>, // Changed from u32 to u64 as per guide
    pub local_tests_inject_fake_profit:     Option<bool>,

    // ── unit‑test helpers ─────────────────────────────────
    pub test_config_uniswap_fee:            Option<u32>,
    pub test_config_velo_stable:            Option<bool>,
    pub test_config_velo_factory:           Option<Address>,

    // ── tx submission knobs ───────────────────────────────
    pub allow_submission_zero_profit:       Option<bool>,
    pub submission_gas_limit_default:       Option<u64>,
    pub submission_gas_price_gwei_fixed:    Option<f64>,
    pub submission_timeout_duration_seconds:Option<u64>,
    pub transaction_relay_urls:             Option<Vec<String>>,

    // ── profit sharing / flat threshold ───────────────────
    pub profit_sharing_bps_for_devs:        Option<u64>, // Changed from u16 to u64
    pub min_flat_profit_weth_threshold:     Option<f64>,

    // ── new gas & relay defaults ──────────────────────────
    pub max_priority_fee_per_gas_gwei:      Option<f64>,
    pub fallback_gas_price_gwei:            Option<f64>,
    pub gas_limit_buffer_percentage:        u64, // Not an Option as per guide

    // Existing fields that might be related or were added previously
    pub min_profit_threshold_wei: Option<U256>, // Keep one declaration
    pub max_gas_price_gwei: Option<f64>, // Keep one declaration
    pub max_slippage_bps: Option<u16>, // Keep one declaration
    pub event_monitoring_poll_interval_ms: Option<u64>,
    pub transaction_submission_retries: Option<usize>, // Keep one declaration
    pub gas_estimation_multiplier: Option<f64>, // Keep one declaration
    pub optimal_loan_search_iterations: Option<u32>, // Keep one declaration
    
    pub velodrome_router_address: Option<Address>, // Keep one declaration
    
    pub uniswap_v3_factory_address: Option<Address>, // Keep one declaration
    pub uniswap_v3_quoter_v2_address: Option<Address>, // Keep one declaration
    pub uniswap_v3_router_address: Option<Address>, // Keep one declaration
    
    pub initial_block_history_to_scan: Option<u64>, // Keep one declaration
    pub max_block_range_per_query: Option<u64>, // Keep one declaration
    pub log_level: Option<String>, // Keep one declaration
    pub use_local_anvil_node: Option<bool>, // Keep one declaration
    pub local_anvil_port: Option<u16>, // Keep one declaration
    pub local_anvil_chain_id: Option<u64>, // Keep one declaration
    pub local_anvil_gas_limit: Option<u64>, // Keep one declaration
    pub local_anvil_block_time: Option<u64>, // Keep one declaration
    pub local_anvil_fork_url: Option<String>, // Keep one declaration
    pub local_anvil_fork_block_number: Option<u64>, // Keep one declaration

    // Fields for event_handler.rs
    pub velodrome_v2_factory_addr: Option<Address>, 
    pub aerodrome_factory_addr: Option<Address>,    
    pub fetch_timeout_secs: Option<u64>, // Was E0560, ensure it's here           

    // Fields for simulation.rs
    pub aerodrome_router_addr: Option<Address>, 
    pub balancer_vault_address: Option<Address>,
    pub max_loan_amount_weth: Option<f64>,
    pub min_loan_amount_weth: Option<f64>,
    pub enable_univ3_dynamic_sizing: Option<bool>, // Was E0560, ensure it's here
    
    // Fields from E0560 errors in config.rs (struct has no field named)
    pub private_rpc_url: Option<String>,
    pub secondary_private_rpc_url: Option<String>,
    pub min_profit_buffer_bps: Option<u16>,
    pub min_profit_abs_buffer_wei_str: Option<String>,
    pub critical_block_lag_seconds: Option<u64>,
    pub critical_log_lag_seconds: Option<u64>,
}

// --- Parsing helpers ---
fn parse_address_env(var_name: &str) -> Result<Address> { let s = env::var(var_name)?; s.parse().map_err(|e| eyre!("Invalid address format for {}: {}", var_name, e)).wrap_err_with(|| format!("Failed to parse env var {}", var_name)) }
fn parse_optional_address_env(var_name: &str) -> Result<Option<Address>> {
    match env::var(var_name) {
        Ok(s) if s.is_empty() => Ok(None),
        Ok(s) => s.parse().map(Some).map_err(|e| eyre!("Invalid optional address format for {}: {}", var_name.to_string(), e)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(e) => Err(eyre!(e).wrap_err(format!("Error checking env var {}", var_name))),
    }
}
fn parse_u8_env(var_name: &str) -> Result<u8> { let s = env::var(var_name)?; s.parse().map_err(|e| eyre!("Invalid u8 format for {}: {}", var_name, e)).wrap_err_with(|| format!("Failed to parse env var {}", var_name)) }
fn parse_f64_env(var_name: &str, default: f64) -> f64 { env::var(var_name).ok().and_then(|s| s.parse().ok()).unwrap_or_else(|| { warn!("Using default f64 for {}: {}", var_name, default); default }) }
fn parse_optional_f64_env(var_name: &str) -> Result<Option<f64>> {
    match env::var(var_name) {
        Ok(s) if s.is_empty() => Ok(None),
        Ok(s) => s.parse().map(Some).map_err(|e| eyre!("Invalid optional f64 format for {}: {}", var_name.to_string(), e)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(e) => Err(eyre!(e).wrap_err(format!("Error checking env var {}", var_name))),
    }
}
fn parse_u32_env(var_name: &str, default: u32) -> u32 { env::var(var_name).ok().and_then(|s| s.parse().ok()).unwrap_or_else(|| { warn!("Using default u32 for {}: {}", var_name, default); default }) }
fn parse_u64_env(var_name: &str, default: u64) -> u64 { env::var(var_name).ok().and_then(|s| s.parse().ok()).unwrap_or_else(|| { warn!("Using default u64 for {}: {}", var_name, default); default }) }
fn parse_optional_u64_env(var_name: &str) -> Result<Option<u64>> {
     match env::var(var_name) {
        Ok(s) if s.is_empty() => Ok(None),
        Ok(s) => s.parse().map(Some).map_err(|e| eyre!("Invalid optional u64 format for {}: {}", var_name.to_string(), e)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(e) => Err(eyre!(e).wrap_err(format!("Error checking env var {}", var_name))),
    }
}
// Updated parse_bool_env to explicitly default to false if var not present or invalid
fn parse_bool_env(var_name: &str) -> bool {
    env::var(var_name)
        .map(|s| s.eq_ignore_ascii_case("true") || s == "1")
        .unwrap_or(false) // Default to false
}
// Helper to parse string env var with a default
fn parse_string_env(var_name: &str, default: &str) -> String {
    env::var(var_name).unwrap_or_else(|_| {
        warn!("Using default string for {}: {}", var_name, default);
        default.to_string()
    })
}


pub fn load_config() -> Result<Config> {
    info!("Loading configuration..."); dotenv().ok();
    // --- Load Required Vars ---
    let ws_rpc_url = env::var("WS_RPC_URL")?; let http_rpc_url = env::var("HTTP_RPC_URL")?; let local_private_key = env::var("LOCAL_PRIVATE_KEY")?;
    let weth_address = parse_address_env("WETH_ADDRESS")?; let usdc_address = parse_address_env("USDC_ADDRESS")?;
    let velo_router_addr = parse_address_env("VELO_V2_ROUTER_ADDR")?; let balancer_vault_address = parse_address_env("BALANCER_VAULT_ADDRESS")?;
    let quoter_v2_address = parse_address_env("QUOTER_V2_ADDRESS")?;
    let weth_decimals = parse_u8_env("WETH_DECIMALS")?; let usdc_decimals = parse_u8_env("USDC_DECIMALS")?;

    // --- Load Optional DEX Expansion ---
    let velodrome_router_address = parse_optional_address_env("VELO_ROUTER_ADDRESS")?;
    let uniswap_v3_factory_address = parse_optional_address_env("UNISWAP_V3_FACTORY_ADDRESS")?;
    let uniswap_v3_quoter_v2_address = parse_optional_address_env("UNISWAP_V3_QUOTER_V2_ADDRESS")?;
    let uniswap_v3_router_address = parse_optional_address_env("UNISWAP_V3_ROUTER_ADDRESS")?;

    // --- Deployment Options
    let deploy_executor = parse_bool_env("DEPLOY_EXECUTOR"); let mut executor_bytecode_path = String::new(); let arb_executor_address = parse_optional_address_env("ARBITRAGE_EXECUTOR_ADDRESS")?;
    if deploy_executor { executor_bytecode_path = env::var("EXECUTOR_BYTECODE_PATH")?; } else if arb_executor_address.is_none() { return Err(eyre!("Need ARBITRAGE_EXECUTOR_ADDRESS")); }

    // --- Load Optimization & Numeric Vars ---
    let min_loan_amount_weth = parse_f64_env("MIN_LOAN_AMOUNT_WETH", 0.1); let max_loan_amount_weth = parse_f64_env("MAX_LOAN_AMOUNT_WETH", 100.0);
    let optimal_loan_search_iterations = parse_u32_env("OPTIMAL_LOAN_SEARCH_ITERATIONS", 10);
    let fetch_timeout_secs = parse_optional_u64_env("FETCH_TIMEOUT_SECS")?;
    let enable_univ3_dynamic_sizing = parse_bool_env("ENABLE_UNIV3_DYNAMIC_SIZING");

    // --- Load Gas Vars ---
    let max_priority_fee_per_gas_gwei = parse_f64_env("MAX_PRIORITY_FEE_PER_GAS_GWEI", 0.01);
    let fallback_gas_price_gwei = parse_optional_f64_env("FALLBACK_GAS_PRICE_GWEI")?;
    let gas_limit_buffer_percentage = parse_u64_env("GAS_LIMIT_BUFFER_PERCENTAGE", 25); let min_flashloan_gas_limit = parse_u64_env("MIN_FLASHLOAN_GAS_LIMIT", 400_000);
    let chain_id = parse_optional_u64_env("CHAIN_ID")?;

    // --- Load Profitability Vars ---
    let min_profit_buffer_bps = parse_u64_env("MIN_PROFIT_BUFFER_BPS", 10); // Default 10 BPS (0.10%)
    let min_profit_abs_buffer_wei_str = parse_string_env("MIN_PROFIT_ABS_BUFFER_WEI", "5000000000000"); // Default 0.000005 WETH equivalent (adjust based on typical gas costs)

    // --- Load Optional String Vars ---
    let private_rpc_url = env::var("PRIVATE_RPC_URL").ok(); let secondary_private_rpc_url = env::var("SECONDARY_PRIVATE_RPC_URL").ok();

    // --- Load Health Check Vars --- Added
    let critical_block_lag_seconds = parse_u64_env("CRITICAL_BLOCK_LAG_SECONDS", 300); // Default 300s
    let critical_log_lag_seconds = parse_u64_env("CRITICAL_LOG_LAG_SECONDS", 300); // Default 300s

    // --- Load Dry Run Option ---
    let dry_run = env::var("DRY_RUN")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    // --- Construct Config ---
    let config = Config {
        ws_rpc_url, http_rpc_url, local_private_key, chain_id, arb_executor_address,
        weth_address, usdc_address, weth_decimals, usdc_decimals,
        velodrome_router_address, uniswap_v3_factory_address, uniswap_v3_quoter_v2_address, uniswap_v3_router_address,
        min_profit_threshold_wei, max_gas_price_gwei, max_slippage_bps,
        transaction_submission_retries, gas_estimation_multiplier, optimal_loan_search_iterations,
        fetch_timeout_secs, enable_univ3_dynamic_sizing,
        max_priority_fee_per_gas_gwei, fallback_gas_price_gwei,
        gas_limit_buffer_percentage, min_flashloan_gas_limit, private_rpc_url, secondary_private_rpc_url,
        min_profit_buffer_bps, min_profit_abs_buffer_wei_str,
        critical_block_lag_seconds, critical_log_lag_seconds, // Added fields
        dry_run,
    };
    info!("✅ Config loaded."); debug!(?config); Ok(config)
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ws_rpc_url: "ws://localhost:8545".to_string(),
            http_rpc_url: "http://localhost:8545".to_string(),
            local_private_key: "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80".to_string(),
            chain_id: Some(31337),
            arb_executor_address: Some(Address::zero()),
            weth_address: Address::zero(),
            usdc_address: Address::zero(),
            weth_decimals: Some(18),
            usdc_decimals: Some(6),
            simulation_gas_price_gwei: Some(1.0),
            simulation_timeout_seconds: Some(10),
            simulation_gas_limit_default: Some(1_000_000),
            simulation_min_gas_limit: Some(100_000),
            dynamic_sizing_velo_percentage: Some(10),
            local_tests_inject_fake_profit: Some(false),
            test_config_uniswap_fee: Some(3000),
            test_config_velo_stable: Some(false),
            test_config_velo_factory: Some(Address::zero()),
            allow_submission_zero_profit: Some(false),
            submission_gas_limit_default: Some(1_500_000),
            submission_gas_price_gwei_fixed: Some(1.0),
            submission_timeout_duration_seconds: Some(60),
            transaction_relay_urls: None,
            profit_sharing_bps_for_devs: Some(0),
            min_flat_profit_weth_threshold: Some(0.0001),
            max_priority_fee_per_gas_gwei: Some(1.5),
            fallback_gas_price_gwei: Some(1.0),
            gas_limit_buffer_percentage: 20,
            min_profit_threshold_wei: Some(U256::from(0)),
            max_gas_price_gwei: Some(100.0),
            max_slippage_bps: Some(50),
            event_monitoring_poll_interval_ms: Some(1000),
            transaction_submission_retries: Some(3),
            gas_estimation_multiplier: Some(1.2),
            optimal_loan_search_iterations: Some(20),
            velodrome_router_address: Some(Address::zero()),
            uniswap_v3_factory_address: Some(Address::zero()),
            uniswap_v3_quoter_v2_address: Some(Address::zero()),
            uniswap_v3_router_address: Some(Address::zero()),
            velodrome_v2_factory_addr: Some(Address::zero()),
            aerodrome_factory_addr: Some(Address::zero()),
            aerodrome_router_addr: Some(Address::zero()),
            balancer_vault_address: Some(Address::zero()),
            initial_block_history_to_scan: Some(100),
            max_block_range_per_query: Some(10000),
            log_level: Some("info".to_string()),
            use_local_anvil_node: Some(true),
            local_anvil_port: Some(8545),
            local_anvil_chain_id: Some(31337),
            local_anvil_gas_limit: Some(30_000_000),
            local_anvil_block_time: Some(1),
            local_anvil_fork_url: None,
            local_anvil_fork_block_number: None,
            fetch_timeout_secs: Some(10),
            max_loan_amount_weth: Some(100.0),
            min_loan_amount_weth: Some(0.1),
            enable_univ3_dynamic_sizing: Some(false),
            private_rpc_url: None,
            secondary_private_rpc_url: None,
            min_profit_buffer_bps: Some(10), 
            min_profit_abs_buffer_wei_str: Some("100000000000000".to_string()),
            critical_block_lag_seconds: Some(120),
            critical_log_lag_seconds: Some(120),
            min_flashloan_gas_limit: Some(300_000),
        }
    }
}