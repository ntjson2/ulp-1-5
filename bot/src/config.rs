// bot/src/config.rs

use ethers::types::Address;
use eyre::Result;
use std::env;
use dotenv::dotenv;

#[derive(Debug, Clone)]
pub struct Config {
    // Network & Keys
    pub ws_rpc_url: String,
    pub http_rpc_url: String,
    pub local_private_key: String,

    // Contract Addresses
    pub arb_executor_address: Option<Address>,
    pub uniswap_v3_factory_addr: Address,
    pub velodrome_v2_factory_addr: Address,
    pub target_token_a: Option<Address>,
    pub target_token_b: Option<Address>,
    pub velo_router_addr: Address,
    pub balancer_vault_address: Address,
    pub quoter_v2_address: Address,

    // Token Decimals
    pub target_token_a_decimals: Option<u8>,
    pub target_token_b_decimals: Option<u8>,

    // Deployment Options
    pub deploy_executor: bool,
    pub executor_bytecode_path: String,

    // Optimization Options
    pub min_loan_amount_weth: f64,
    pub max_loan_amount_weth: f64,
    pub optimal_loan_search_iterations: u32,

    // Gas Pricing Options
    pub max_priority_fee_per_gas_gwei: f64,
    pub gas_limit_buffer_percentage: u64,
    pub min_flashloan_gas_limit: u64,
}

pub fn load_config() -> Result<Config> {
    println!("Loading configuration from .env file...");
    dotenv().ok();

    // FIX E0308: Restore correct closure bodies
    let parse_bool_env = |var_name: &str| -> bool {
        env::var(var_name).map(|s| s.eq_ignore_ascii_case("true") || s == "1").unwrap_or(false)
    };
    let parse_f64_env = |var_name: &str, default: f64| -> f64 {
        env::var(var_name).ok().and_then(|s| s.parse::<f64>().ok()).unwrap_or(default)
    };
    let parse_u32_env = |var_name: &str, default: u32| -> u32 {
        env::var(var_name).ok().and_then(|s| s.parse::<u32>().ok()).unwrap_or(default)
    };
    let parse_u64_env = |var_name: &str, default: u64| -> u64 {
        env::var(var_name).ok().and_then(|s| s.parse::<u64>().ok()).unwrap_or(default)
    };
    let parse_optional_address = |var_name: &str| -> Result<Option<Address>> {
        match env::var(var_name) {
            Ok(addr_str) if !addr_str.is_empty() => Ok(Some(addr_str.parse::<Address>()?)),
            _ => Ok(None),
        }
    };
    let parse_optional_u8 = |var_name: &str| -> Result<Option<u8>> {
         match env::var(var_name) {
            Ok(val_str) if !val_str.is_empty() => Ok(Some(val_str.parse::<u8>()?)),
            _ => Ok(None),
        }
    };

    // --- Load vars ---
    let ws_rpc_url = env::var("WS_RPC_URL")?;
    let http_rpc_url = env::var("HTTP_RPC_URL")?;
    let local_private_key = env::var("LOCAL_PRIVATE_KEY")?;
    let uniswap_v3_factory_addr = env::var("UNISWAP_V3_FACTORY_ADDR")?.parse::<Address>()?;
    let velodrome_v2_factory_addr = env::var("VELODROME_V2_FACTORY_ADDR")?.parse::<Address>()?;
    let target_token_a = parse_optional_address("TARGET_TOKEN_A")?;
    let target_token_b = parse_optional_address("TARGET_TOKEN_B")?;
    let target_token_a_decimals = parse_optional_u8("TARGET_TOKEN_A_DECIMALS")?;
    let target_token_b_decimals = parse_optional_u8("TARGET_TOKEN_B_DECIMALS")?;
    if target_token_a.is_some() && target_token_a_decimals.is_none() { eyre::bail!("TOKEN_A_DECIMALS required if TOKEN_A set"); }
    if target_token_b.is_some() && target_token_b_decimals.is_none() { eyre::bail!("TOKEN_B_DECIMALS required if TOKEN_B set"); }
    let velo_router_addr = env::var("VELO_V2_ROUTER_ADDR")?.parse::<Address>()?;
    let balancer_vault_address = env::var("BALANCER_VAULT_ADDRESS")?.parse::<Address>()?;
    let quoter_v2_address = env::var("QUOTER_V2_ADDRESS")?.parse::<Address>()?;
    let deploy_executor = parse_bool_env("DEPLOY_EXECUTOR");
    let mut executor_bytecode_path = String::new();
    let arb_executor_address = env::var("ARBITRAGE_EXECUTOR_ADDRESS").ok().and_then(|s| s.parse::<Address>().ok());
    if deploy_executor { executor_bytecode_path = env::var("EXECUTOR_BYTECODE_PATH")?; } else if arb_executor_address.is_none() { panic!("Executor address needed"); }
    let min_loan_amount_weth = parse_f64_env("MIN_LOAN_AMOUNT_WETH", 0.1);
    let max_loan_amount_weth = parse_f64_env("MAX_LOAN_AMOUNT_WETH", 100.0);
    let optimal_loan_search_iterations = parse_u32_env("OPTIMAL_LOAN_SEARCH_ITERATIONS", 10);
    let max_priority_fee_per_gas_gwei = parse_f64_env("MAX_PRIORITY_FEE_PER_GAS_GWEI", 1.0);
    let gas_limit_buffer_percentage = parse_u64_env("GAS_LIMIT_BUFFER_PERCENTAGE", 20);
    let min_flashloan_gas_limit = parse_u64_env("MIN_FLASHLOAN_GAS_LIMIT", 200_000);

    let config = Config {
        ws_rpc_url, http_rpc_url, local_private_key, arb_executor_address,
        uniswap_v3_factory_addr, velodrome_v2_factory_addr, target_token_a, target_token_b,
        velo_router_addr, balancer_vault_address, quoter_v2_address,
        target_token_a_decimals, target_token_b_decimals,
        deploy_executor, executor_bytecode_path, min_loan_amount_weth, max_loan_amount_weth,
        optimal_loan_search_iterations, max_priority_fee_per_gas_gwei,
        gas_limit_buffer_percentage, min_flashloan_gas_limit,
    };

    println!("✅ Configuration loaded successfully.");
    Ok(config)
}
// END OF FILE: bot/src/config.rs