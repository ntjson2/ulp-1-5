// bot/src/config.rs

use ethers::types::Address;
use eyre::{Result, WrapErr, eyre};
use std::env;
use dotenv::dotenv;
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
pub struct Config {
    // Network & Keys
    pub ws_rpc_url: String,
    pub http_rpc_url: String,
    pub local_private_key: String,
    pub chain_id: Option<u64>, // Optional: Chain ID if needed for logic

    // Contract Addresses (Core - Optimism/Base)
    pub arb_executor_address: Option<Address>,
    pub uniswap_v3_factory_addr: Address,
    pub velodrome_v2_factory_addr: Address, // Velodrome on Optimism
    pub balancer_vault_address: Address,
    pub quoter_v2_address: Address, // UniV3 Quoter V2 address for the target chain

    // Specific DEX Routers (Optional or Chain-Specific)
    pub velo_router_addr: Address, // Velodrome Router V2 on Optimism

    // --- DEX Expansion ---
    pub aerodrome_factory_addr: Option<Address>, // Aerodrome Factory on Base
    pub aerodrome_router_addr: Option<Address>,  // Aerodrome Router on Base
    // TODO: Add addresses for Ramses (Arbitrum) etc. when implementing

    // Token Information (Required for initial WETH/USDC pair)
    pub weth_address: Address,
    pub usdc_address: Address,
    pub weth_decimals: u8,
    pub usdc_decimals: u8,

    // Deployment Options
    pub deploy_executor: bool,
    pub executor_bytecode_path: String,

    // Optimization Options
    pub min_loan_amount_weth: f64,
    pub max_loan_amount_weth: f64,
    pub optimal_loan_search_iterations: u32,
    pub fetch_timeout_secs: Option<u64>, // Timeout for individual pool state fetches
    pub enable_univ3_dynamic_sizing: bool, // <-- New flag (defaults to false)

    // Gas Pricing Options
    pub max_priority_fee_per_gas_gwei: f64,
    pub fallback_gas_price_gwei: Option<f64>, // Fallback if fetch fails
    pub gas_limit_buffer_percentage: u64,
    pub min_flashloan_gas_limit: u64,

    // Transaction Submission Options
    pub private_rpc_url: Option<String>, // Primary private relay (e.g., Flashbots Protect, MEV-Share)
    pub secondary_private_rpc_url: Option<String>, // Secondary/fallback private relay
}

// --- Parsing helpers ---
fn parse_address_env(var_name: &str) -> Result<Address> { let s = env::var(var_name)?; s.parse().map_err(|e| eyre!("Invalid address format for {}: {}", var_name, e)).wrap_err_with(|| format!("Failed to parse env var {}", var_name)) }
fn parse_optional_address_env(var_name: &str) -> Result<Option<Address>> {
    match env::var(var_name) {
        Ok(s) if s.is_empty() => Ok(None),
        // FIX E0521: Ensure error message construction doesn't capture non-'static var_name
        Ok(s) => s.parse().map(Some).map_err(|e| eyre!("Invalid optional address format for {}: {}", var_name.to_string(), e)),
        Err(env::VarError::NotPresent) => Ok(None),
        // FIX E0521: Use eyre!(e).wrap_err(...)
        Err(e) => Err(eyre!(e).wrap_err(format!("Error checking env var {}", var_name))),
    }
}
fn parse_u8_env(var_name: &str) -> Result<u8> { let s = env::var(var_name)?; s.parse().map_err(|e| eyre!("Invalid u8 format for {}: {}", var_name, e)).wrap_err_with(|| format!("Failed to parse env var {}", var_name)) }
fn parse_f64_env(var_name: &str, default: f64) -> f64 { env::var(var_name).ok().and_then(|s| s.parse().ok()).unwrap_or_else(|| { warn!("Using default f64 for {}: {}", var_name, default); default }) }
fn parse_optional_f64_env(var_name: &str) -> Result<Option<f64>> {
    match env::var(var_name) {
        Ok(s) if s.is_empty() => Ok(None),
        // FIX E0521: Ensure error message construction doesn't capture non-'static var_name
        Ok(s) => s.parse().map(Some).map_err(|e| eyre!("Invalid optional f64 format for {}: {}", var_name.to_string(), e)),
        Err(env::VarError::NotPresent) => Ok(None),
        // FIX E0521: Use eyre!(e).wrap_err(...)
        Err(e) => Err(eyre!(e).wrap_err(format!("Error checking env var {}", var_name))),
    }
}
fn parse_u32_env(var_name: &str, default: u32) -> u32 { env::var(var_name).ok().and_then(|s| s.parse().ok()).unwrap_or_else(|| { warn!("Using default u32 for {}: {}", var_name, default); default }) }
fn parse_u64_env(var_name: &str, default: u64) -> u64 { env::var(var_name).ok().and_then(|s| s.parse().ok()).unwrap_or_else(|| { warn!("Using default u64 for {}: {}", var_name, default); default }) }
fn parse_optional_u64_env(var_name: &str) -> Result<Option<u64>> {
     match env::var(var_name) {
        Ok(s) if s.is_empty() => Ok(None),
        // FIX E0521: Ensure error message construction doesn't capture non-'static var_name
        Ok(s) => s.parse().map(Some).map_err(|e| eyre!("Invalid optional u64 format for {}: {}", var_name.to_string(), e)),
        Err(env::VarError::NotPresent) => Ok(None),
        // FIX E0521: Use eyre!(e).wrap_err(...)
        Err(e) => Err(eyre!(e).wrap_err(format!("Error checking env var {}", var_name))),
    }
}
// Updated parse_bool_env to explicitly default to false if var not present or invalid
fn parse_bool_env(var_name: &str) -> bool {
    env::var(var_name)
        .map(|s| s.eq_ignore_ascii_case("true") || s == "1")
        .unwrap_or(false) // Default to false
}


pub fn load_config() -> Result<Config> {
    info!("Loading configuration..."); dotenv().ok();
    // --- Load Required Vars ---
    let ws_rpc_url = env::var("WS_RPC_URL")?; let http_rpc_url = env::var("HTTP_RPC_URL")?; let local_private_key = env::var("LOCAL_PRIVATE_KEY")?;
    let uniswap_v3_factory_addr = parse_address_env("UNISWAP_V3_FACTORY_ADDR")?; let velodrome_v2_factory_addr = parse_address_env("VELODROME_V2_FACTORY_ADDR")?;
    let weth_address = parse_address_env("WETH_ADDRESS")?; let usdc_address = parse_address_env("USDC_ADDRESS")?;
    let velo_router_addr = parse_address_env("VELO_V2_ROUTER_ADDR")?; let balancer_vault_address = parse_address_env("BALANCER_VAULT_ADDRESS")?;
    let quoter_v2_address = parse_address_env("QUOTER_V2_ADDRESS")?;
    let weth_decimals = parse_u8_env("WETH_DECIMALS")?; let usdc_decimals = parse_u8_env("USDC_DECIMALS")?;

    // --- Load Optional DEX Expansion ---
    let aerodrome_factory_addr = parse_optional_address_env("AERODROME_FACTORY_ADDR")?; let aerodrome_router_addr = parse_optional_address_env("AERODROME_ROUTER_ADDR")?;

    // --- Deployment Options ---
    let deploy_executor = parse_bool_env("DEPLOY_EXECUTOR"); let mut executor_bytecode_path = String::new(); let arb_executor_address = parse_optional_address_env("ARBITRAGE_EXECUTOR_ADDRESS")?;
    if deploy_executor { executor_bytecode_path = env::var("EXECUTOR_BYTECODE_PATH")?; } else if arb_executor_address.is_none() { return Err(eyre!("Need ARBITRAGE_EXECUTOR_ADDRESS")); }

    // --- Load Optimization & Numeric Vars ---
    let min_loan_amount_weth = parse_f64_env("MIN_LOAN_AMOUNT_WETH", 0.1); let max_loan_amount_weth = parse_f64_env("MAX_LOAN_AMOUNT_WETH", 100.0);
    let optimal_loan_search_iterations = parse_u32_env("OPTIMAL_LOAN_SEARCH_ITERATIONS", 10);
    let fetch_timeout_secs = parse_optional_u64_env("FETCH_TIMEOUT_SECS")?;
    let enable_univ3_dynamic_sizing = parse_bool_env("ENABLE_UNIV3_DYNAMIC_SIZING"); // <-- Parse new flag

    // --- Load Gas Vars ---
    let max_priority_fee_per_gas_gwei = parse_f64_env("MAX_PRIORITY_FEE_PER_GAS_GWEI", 0.01);
    let fallback_gas_price_gwei = parse_optional_f64_env("FALLBACK_GAS_PRICE_GWEI")?;
    let gas_limit_buffer_percentage = parse_u64_env("GAS_LIMIT_BUFFER_PERCENTAGE", 25); let min_flashloan_gas_limit = parse_u64_env("MIN_FLASHLOAN_GAS_LIMIT", 400_000);
    let chain_id = parse_optional_u64_env("CHAIN_ID")?;

    // --- Load Optional String Vars ---
    let private_rpc_url = env::var("PRIVATE_RPC_URL").ok(); let secondary_private_rpc_url = env::var("SECONDARY_PRIVATE_RPC_URL").ok();

    // --- Construct Config ---
    let config = Config {
        ws_rpc_url, http_rpc_url, local_private_key, chain_id, arb_executor_address,
        uniswap_v3_factory_addr, velodrome_v2_factory_addr, balancer_vault_address, quoter_v2_address,
        velo_router_addr, aerodrome_factory_addr, aerodrome_router_addr, weth_address, usdc_address,
        weth_decimals, usdc_decimals, deploy_executor, executor_bytecode_path, min_loan_amount_weth,
        max_loan_amount_weth, optimal_loan_search_iterations, fetch_timeout_secs,
        enable_univ3_dynamic_sizing, // <-- Add to struct init
        max_priority_fee_per_gas_gwei, fallback_gas_price_gwei,
        gas_limit_buffer_percentage, min_flashloan_gas_limit, private_rpc_url, secondary_private_rpc_url,
    };
    info!("✅ Config loaded."); debug!(?config); Ok(config)
}