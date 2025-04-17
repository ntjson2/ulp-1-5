// src/config.rs

use ethers::types::Address;
use eyre::Result;
use std::env;
use dotenv::dotenv;

#[derive(Debug, Clone)]
pub struct Config {
    // Network & Keys
    pub local_rpc_url: String,
    pub local_private_key: String,

    // Contract & Token Addresses
    pub arb_executor_address: Option<Address>,
    pub uni_v3_pool_addr: Address,
    pub velo_v2_pool_addr: Address,
    pub weth_address: Address,
    pub usdc_address: Address,
    pub velo_router_addr: Address,
    pub balancer_vault_address: Address,
    pub quoter_v2_address: Address,

    // Token Decimals
    pub weth_decimals: u8,
    pub usdc_decimals: u8,

    // Deployment Options
    pub deploy_executor: bool,
    pub executor_bytecode_path: String,

    // Optimization Options
    pub min_loan_amount_weth: f64,
    pub max_loan_amount_weth: f64,
    pub optimal_loan_search_iterations: u32,

    // Gas Pricing Options
    pub max_priority_fee_per_gas_gwei: f64,
    pub gas_limit_buffer_percentage: u64, // NEW: Gas limit buffer %
    pub min_flashloan_gas_limit: u64,     // NEW: Minimum expected gas limit

}

pub fn load_config() -> Result<Config> {
    println!("Loading configuration from .env file...");
    dotenv().ok();

    let parse_bool_env = |var_name: &str| -> bool { /* ... */ };
    let parse_f64_env = |var_name: &str, default: f64| -> f64 { /* ... */ };
    let parse_u32_env = |var_name: &str, default: u32| -> u32 { /* ... */ };
     // Helper to parse u64 with default
    let parse_u64_env = |var_name: &str, default: u64| -> u64 {
        env::var(var_name).ok().and_then(|s| s.parse::<u64>().ok()).unwrap_or(default)
    };


    let local_rpc_url = env::var("LOCAL_RPC_URL")?;
    let local_private_key = env::var("LOCAL_PRIVATE_KEY")?;
    let uni_v3_pool_addr = env::var("UNI_V3_POOL_ADDR")?.parse::<Address>()?;
    let velo_v2_pool_addr = env::var("VELO_V2_POOL_ADDR")?.parse::<Address>()?;
    let weth_address = env::var("WETH_ADDRESS")?.parse::<Address>()?;
    let usdc_address = env::var("USDC_ADDRESS")?.parse::<Address>()?;
    let weth_decimals = env::var("WETH_DECIMALS")?.parse::<u8>()?;
    let usdc_decimals = env::var("USDC_DECIMALS")?.parse::<u8>()?;
    let velo_router_addr = env::var("VELO_V2_ROUTER_ADDR")?.parse::<Address>()?;
    let balancer_vault_address = env::var("BALANCER_VAULT_ADDRESS")?.parse::<Address>()?;
    let quoter_v2_address = env::var("QUOTER_V2_ADDRESS")?.parse::<Address>()?;

    let deploy_executor = parse_bool_env("DEPLOY_EXECUTOR");
    let mut executor_bytecode_path = String::new();
    let arb_executor_address = env::var("ARBITRAGE_EXECUTOR_ADDRESS").ok().and_then(|s| s.parse::<Address>().ok());

    if deploy_executor {
        executor_bytecode_path = env::var("EXECUTOR_BYTECODE_PATH")?;
        if arb_executor_address.is_some() { println!("⚠️ WARNING: ARBITRAGE_EXECUTOR_ADDRESS is set in .env but DEPLOY_EXECUTOR is true."); }
    } else if arb_executor_address.is_none() { panic!("❌ ARBITRAGE_EXECUTOR_ADDRESS must be set if DEPLOY_EXECUTOR is not true."); }

    let min_loan_amount_weth = parse_f64_env("MIN_LOAN_AMOUNT_WETH", 0.1);
    let max_loan_amount_weth = parse_f64_env("MAX_LOAN_AMOUNT_WETH", 100.0);
    let optimal_loan_search_iterations = parse_u32_env("OPTIMAL_LOAN_SEARCH_ITERATIONS", 10);
    let max_priority_fee_per_gas_gwei = parse_f64_env("MAX_PRIORITY_FEE_PER_GAS_GWEI", 1.0);
    // Load new gas limit params
    let gas_limit_buffer_percentage = parse_u64_env("GAS_LIMIT_BUFFER_PERCENTAGE", 20); // Default 20% buffer
    let min_flashloan_gas_limit = parse_u64_env("MIN_FLASHLOAN_GAS_LIMIT", 200_000); // Default 200k minimum


    let config = Config {
        local_rpc_url, local_private_key, arb_executor_address, uni_v3_pool_addr,
        velo_v2_pool_addr, weth_address, usdc_address, weth_decimals, usdc_decimals,
        velo_router_addr, balancer_vault_address, quoter_v2_address, deploy_executor,
        executor_bytecode_path, min_loan_amount_weth, max_loan_amount_weth,
        optimal_loan_search_iterations, max_priority_fee_per_gas_gwei,
        gas_limit_buffer_percentage, // Add new field
        min_flashloan_gas_limit,   // Add new field
    };

    println!("✅ Configuration loaded successfully.");
    Ok(config)
}
// END OF FILE: src/config.rs