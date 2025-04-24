// bot/src/config.rs

use ethers::types::Address;
use eyre::{Result, WrapErr}; // Use WrapErr for better context on errors
use std::env;
use dotenv::dotenv;
use tracing::{info, warn}; // Use tracing for logging

#[derive(Debug, Clone)]
pub struct Config {
    // Network & Keys
    pub ws_rpc_url: String,
    pub http_rpc_url: String,
    pub local_private_key: String,

    // Contract Addresses
    pub arb_executor_address: Option<Address>, // Optional: Used if deploy_executor is false
    pub uniswap_v3_factory_addr: Address,
    pub velodrome_v2_factory_addr: Address,
    pub weth_address: Address, // Explicitly define WETH
    pub usdc_address: Address, // Explicitly define USDC (check which version on target chain)
    pub velo_router_addr: Address,
    pub balancer_vault_address: Address,
    pub quoter_v2_address: Address,

    // Token Decimals (Must be provided for WETH/USDC)
    pub weth_decimals: u8,
    pub usdc_decimals: u8,

    // Deployment Options
    pub deploy_executor: bool,
    pub executor_bytecode_path: String, // Required if deploy_executor is true

    // Optimization Options
    pub min_loan_amount_weth: f64,
    pub max_loan_amount_weth: f64,
    pub optimal_loan_search_iterations: u32,

    // Gas Pricing Options
    pub max_priority_fee_per_gas_gwei: f64, // Max *priority* fee for EIP-1559 txs
    pub gas_limit_buffer_percentage: u64, // Percentage to add to gas estimate
    pub min_flashloan_gas_limit: u64,     // Minimum gas limit to use for flashloan tx
}

// Helper function to parse an environment variable into an Address
fn parse_address_env(var_name: &str) -> Result<Address> {
    let value_str = env::var(var_name)
        .map_err(|e| eyre::eyre!("Missing env variable: {}", var_name).with_source(e))?;
    value_str.parse::<Address>()
        .map_err(|e| eyre::eyre!("Invalid address format for {}: {}", var_name, value_str).with_source(e))
}

// Helper function to parse an environment variable into a u8
fn parse_u8_env(var_name: &str) -> Result<u8> {
    let value_str = env::var(var_name)
        .map_err(|e| eyre::eyre!("Missing env variable: {}", var_name).with_source(e))?;
    value_str.parse::<u8>()
        .map_err(|e| eyre::eyre!("Invalid u8 format for {}: {}", var_name, value_str).with_source(e))
}

// Helper function to parse an environment variable into f64, with default
fn parse_f64_env(var_name: &str, default: f64) -> f64 {
    match env::var(var_name) {
        Ok(val_str) => match val_str.parse::<f64>() {
            Ok(val) => val,
            Err(_) => {
                warn!(
                    "Invalid f64 format for {}, using default {}: {}",
                    var_name, default, val_str
                );
                default
            }
        },
        Err(_) => default, // Use default if var is not present
    }
}

// Helper function to parse an environment variable into u32, with default
fn parse_u32_env(var_name: &str, default: u32) -> u32 {
     match env::var(var_name) {
        Ok(val_str) => match val_str.parse::<u32>() {
            Ok(val) => val,
            Err(_) => {
                warn!(
                    "Invalid u32 format for {}, using default {}: {}",
                    var_name, default, val_str
                );
                default
            }
        },
        Err(_) => default,
    }
}

// Helper function to parse an environment variable into u64, with default
fn parse_u64_env(var_name: &str, default: u64) -> u64 {
     match env::var(var_name) {
        Ok(val_str) => match val_str.parse::<u64>() {
            Ok(val) => val,
            Err(_) => {
                warn!(
                    "Invalid u64 format for {}, using default {}: {}",
                    var_name, default, val_str
                );
                default
            }
        },
        Err(_) => default,
    }
}

// Helper function to parse boolean environment variable
fn parse_bool_env(var_name: &str) -> bool {
    env::var(var_name)
        .map(|s| s.eq_ignore_ascii_case("true") || s == "1")
        .unwrap_or(false) // Default to false if not present or invalid
}


pub fn load_config() -> Result<Config> {
    info!("Loading configuration from .env file...");
    dotenv().ok(); // Load .env file, ignore errors if not found

    // --- Load Required String Vars ---
    let ws_rpc_url = env::var("WS_RPC_URL").wrap_err("Missing env variable: WS_RPC_URL")?;
    let http_rpc_url = env::var("HTTP_RPC_URL").wrap_err("Missing env variable: HTTP_RPC_URL")?;
    let local_private_key = env::var("LOCAL_PRIVATE_KEY").wrap_err("Missing env variable: LOCAL_PRIVATE_KEY")?;

    // --- Load Required Addresses ---
    let uniswap_v3_factory_addr = parse_address_env("UNISWAP_V3_FACTORY_ADDR")?;
    let velodrome_v2_factory_addr = parse_address_env("VELODROME_V2_FACTORY_ADDR")?;
    let weth_address = parse_address_env("WETH_ADDRESS")?;
    let usdc_address = parse_address_env("USDC_ADDRESS")?;
    let velo_router_addr = parse_address_env("VELO_V2_ROUTER_ADDR")?;
    let balancer_vault_address = parse_address_env("BALANCER_VAULT_ADDRESS")?;
    let quoter_v2_address = parse_address_env("QUOTER_V2_ADDRESS")?;

    // --- Load Required Decimals ---
    let weth_decimals = parse_u8_env("WETH_DECIMALS")?;
    let usdc_decimals = parse_u8_env("USDC_DECIMALS")?;

    // --- Deployment Options ---
    let deploy_executor = parse_bool_env("DEPLOY_EXECUTOR");
    let mut executor_bytecode_path = String::new(); // Init empty
    let arb_executor_address: Option<Address> = env::var("ARBITRAGE_EXECUTOR_ADDRESS")
        .ok() // Optional variable
        .and_then(|s| s.parse::<Address>().ok()); // Parse if present

    // Validate deployment config
    if deploy_executor {
        executor_bytecode_path = env::var("EXECUTOR_BYTECODE_PATH")
            .wrap_err("EXECUTOR_BYTECODE_PATH must be set if DEPLOY_EXECUTOR is true")?;
        info!(path = %executor_bytecode_path, "Executor deployment enabled.");
    } else if arb_executor_address.is_none() {
        // If not deploying and no address provided, it's an error
        return Err(eyre::eyre!("ARBITRAGE_EXECUTOR_ADDRESS must be set if DEPLOY_EXECUTOR is false"));
    } else {
        info!(address = ?arb_executor_address.unwrap(), "Using pre-deployed executor."); // Safe unwrap due to check above
    }

     // --- Load Optional Numeric Vars with Defaults ---
    let min_loan_amount_weth = parse_f64_env("MIN_LOAN_AMOUNT_WETH", 0.1);
    let max_loan_amount_weth = parse_f64_env("MAX_LOAN_AMOUNT_WETH", 50.0); // Reduced default based on example
    let optimal_loan_search_iterations = parse_u32_env("OPTIMAL_LOAN_SEARCH_ITERATIONS", 10);
    let max_priority_fee_per_gas_gwei = parse_f64_env("MAX_PRIORITY_FEE_PER_GAS_GWEI", 0.01); // L2 default
    let gas_limit_buffer_percentage = parse_u64_env("GAS_LIMIT_BUFFER_PERCENTAGE", 25);
    let min_flashloan_gas_limit = parse_u64_env("MIN_FLASHLOAN_GAS_LIMIT", 400_000); // L2 default

    // --- Construct Config ---
    let config = Config {
        ws_rpc_url,
        http_rpc_url,
        local_private_key,
        arb_executor_address,
        uniswap_v3_factory_addr,
        velodrome_v2_factory_addr,
        weth_address,
        usdc_address,
        velo_router_addr,
        balancer_vault_address,
        quoter_v2_address,
        weth_decimals,
        usdc_decimals,
        deploy_executor,
        executor_bytecode_path,
        min_loan_amount_weth,
        max_loan_amount_weth,
        optimal_loan_search_iterations,
        max_priority_fee_per_gas_gwei,
        gas_limit_buffer_percentage,
        min_flashloan_gas_limit,
    };

    info!("✅ Configuration loaded successfully.");
    debug!(config = ?config, "Full configuration values"); // Log full config only at debug level
    Ok(config)
}
// END OF FILE: bot/src/config.rs