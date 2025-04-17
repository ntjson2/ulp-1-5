// src/config.rs

use ethers::types::Address;
use eyre::Result;
use std::env; // Keep only env
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

    // Optimization Options (NEW)
    pub min_loan_amount_weth: f64,
    pub max_loan_amount_weth: f64,
    pub optimal_loan_search_iterations: u32,

    // Simulation Options (Future)
    // pub manipulate_uni_price: bool,
    // pub manipulation_percentage: f64,
}

pub fn load_config() -> Result<Config> {
    println!("Loading configuration from .env file...");
    dotenv().ok();

    let parse_bool_env = |var_name: &str| -> bool {
        env::var(var_name).map(|s| s.eq_ignore_ascii_case("true") || s == "1").unwrap_or(false)
    };
    // Helper to parse f64 with default
    let parse_f64_env = |var_name: &str, default: f64| -> f64 {
        env::var(var_name).ok().and_then(|s| s.parse::<f64>().ok()).unwrap_or(default)
    };
     // Helper to parse u32 with default
    let parse_u32_env = |var_name: &str, default: u32| -> u32 {
        env::var(var_name).ok().and_then(|s| s.parse::<u32>().ok()).unwrap_or(default)
    };


    let local_rpc_url = env::var("LOCAL_RPC_URL")
        .expect("❌ Environment variable LOCAL_RPC_URL must be set");
    let local_private_key = env::var("LOCAL_PRIVATE_KEY")
        .expect("❌ Environment variable LOCAL_PRIVATE_KEY must be set");
    let uni_v3_pool_addr = env::var("UNI_V3_POOL_ADDR")
        .expect("❌ Environment variable UNI_V3_POOL_ADDR must be set")
        .parse::<Address>()?;
    let velo_v2_pool_addr = env::var("VELO_V2_POOL_ADDR")
        .expect("❌ Environment variable VELO_V2_POOL_ADDR must be set")
        .parse::<Address>()?;
    let weth_address = env::var("WETH_ADDRESS")
        .expect("❌ Environment variable WETH_ADDRESS must be set")
        .parse::<Address>()?;
    let usdc_address = env::var("USDC_ADDRESS")
        .expect("❌ Environment variable USDC_ADDRESS must be set")
        .parse::<Address>()?;
    let weth_decimals = env::var("WETH_DECIMALS")?
        .parse::<u8>()
        .expect("❌ WETH_DECIMALS must be a valid number (0-255)");
    let usdc_decimals = env::var("USDC_DECIMALS")?
        .parse::<u8>()
        .expect("❌ USDC_DECIMALS must be a valid number (0-255)");
    let velo_router_addr = env::var("VELO_V2_ROUTER_ADDR")
        .expect("❌ Environment variable VELO_V2_ROUTER_ADDR must be set")
        .parse::<Address>()?;
    let balancer_vault_address = env::var("BALANCER_VAULT_ADDRESS")
        .expect("❌ Environment variable BALANCER_VAULT_ADDRESS must be set")
        .parse::<Address>()?;
    let quoter_v2_address = env::var("QUOTER_V2_ADDRESS")
        .expect("❌ Environment variable QUOTER_V2_ADDRESS must be set")
        .parse::<Address>()?;

    let deploy_executor = parse_bool_env("DEPLOY_EXECUTOR");
    let mut executor_bytecode_path = String::new();

    let arb_executor_address = env::var("ARBITRAGE_EXECUTOR_ADDRESS")
        .ok()
        .and_then(|s| s.parse::<Address>().ok());

    if deploy_executor {
        executor_bytecode_path = env::var("EXECUTOR_BYTECODE_PATH")
            .expect("❌ EXECUTOR_BYTECODE_PATH must be set if DEPLOY_EXECUTOR is true");
        if arb_executor_address.is_some() {
            println!("⚠️ WARNING: ARBITRAGE_EXECUTOR_ADDRESS is set in .env but DEPLOY_EXECUTOR is true. The deployed address will be used.");
        }
    } else if arb_executor_address.is_none() {
        panic!("❌ ARBITRAGE_EXECUTOR_ADDRESS must be set in .env if DEPLOY_EXECUTOR is not true.");
    }

    // Load new optimization params
    let min_loan_amount_weth = parse_f64_env("MIN_LOAN_AMOUNT_WETH", 0.1); // Default 0.1 WETH
    let max_loan_amount_weth = parse_f64_env("MAX_LOAN_AMOUNT_WETH", 100.0); // Default 100 WETH
    let optimal_loan_search_iterations = parse_u32_env("OPTIMAL_LOAN_SEARCH_ITERATIONS", 10); // Default 10 iterations


    let config = Config {
        local_rpc_url,
        local_private_key,
        arb_executor_address,
        uni_v3_pool_addr,
        velo_v2_pool_addr,
        weth_address,
        usdc_address,
        weth_decimals,
        usdc_decimals,
        velo_router_addr,
        balancer_vault_address,
        quoter_v2_address,
        deploy_executor,
        executor_bytecode_path,
        // Add new fields
        min_loan_amount_weth,
        max_loan_amount_weth,
        optimal_loan_search_iterations,
    };

    println!("✅ Configuration loaded successfully.");
    Ok(config)
}