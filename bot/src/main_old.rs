use ethers::{
    prelude::*,
    types::{Address, U256}, // Added Address explicitly
    utils::{format_units},
};
use eyre::Result;
use std::{env, sync::Arc, str::FromStr};
use dotenv::dotenv;

// --- Define Contract Bindings ---
abigen!(
    UniswapV3Pool,
    "./abis/UniswapV3Pool.json",
    event_derives(serde::Deserialize, serde::Serialize)
);
abigen!(
    VelodromeV2Pool,
    "./abis/VelodromeV2Pool.json",
    event_derives(serde::Deserialize, serde::Serialize)
);
abigen!(
    VelodromeRouter,
    "./abis/VelodromeRouter.json",
    event_derives(serde::Deserialize, serde::Serialize)
);
abigen!(
    BalancerVault,
    "./abis/BalancerVault.json",
    event_derives(serde::Deserialize, serde::Serialize)
);
abigen!(
    QuoterV2, // Added QuoterV2 ABI
    "./abis/QuoterV2.json",
    event_derives(serde::Deserialize, serde::Serialize)
);
abigen!(
    IERC20,
    r#"[
        function approve(address spender, uint256 amount) external returns (bool)
        function balanceOf(address account) external view returns (uint256)
        function decimals() external view returns (uint8)
    ]"#,
    event_derives(serde::Deserialize, serde::Serialize)
);

// --- Use statements for generated types ---
// Adjust these paths if your abigen! macro generates different module names
// Often they match the contract name in lowercase
use crate::quoter_v2 as quoter_v2_bindings;
use crate::velodrome_router as velo_router_bindings;

// --- Constants ---
const ARBITRAGE_THRESHOLD_PERCENTAGE: f64 = 0.1; // Example threshold
const FLASH_LOAN_FEE_RATE: f64 = 0.0000; // Example Balancer fee (0% usually on L2s)
const SIMULATION_AMOUNT_WETH: f64 = 1.0; // Amount of WETH to simulate with

// --- Helper Functions ---

// Lossy U256 -> f64 conversion
trait ToF64Lossy { fn to_f64_lossy(&self) -> f64; }
impl ToF64Lossy for U256 {
    fn to_f64_lossy(&self) -> f64 {
        match f64::from_str(&self.to_string()) { Ok(f) => f, Err(_) => f64::MAX }
    }
}

// Calculate Uniswap V3 price from sqrtPriceX96 using f64 (for initial detection only)
fn v3_price_from_sqrt(sqrt_price_x96: U256, decimals0: u8, decimals1: u8) -> Result<f64> {
    if sqrt_price_x96.is_zero() { return Ok(0.0); }
    let sqrt_price_x96_f64 = sqrt_price_x96.to_f64_lossy();
    let q96_f64: f64 = 2.0_f64.powi(96);
    if q96_f64 == 0.0 { return Err(eyre::eyre!("Q96 calculation resulted in zero")); }
    let price_ratio_f64 = (sqrt_price_x96_f64 / q96_f64).powi(2);
    let decimal_diff: i32 = (decimals0 as i32) - (decimals1 as i32);
    let adjustment = 10f64.powi(decimal_diff);
    Ok(price_ratio_f64 * adjustment)
}

// Calculate Uniswap V2/Velodrome price from reserves using f64 (for initial detection only)
fn v2_price_from_reserves(reserve0: U256, reserve1: U256, decimals0: u8, decimals1: u8) -> Result<f64> {
    if reserve0.is_zero() { return Ok(0.0); }
    let reserve0_f64 = reserve0.to_f64_lossy();
    let reserve1_f64 = reserve1.to_f64_lossy();
    if reserve0_f64.abs() < f64::EPSILON { return Ok(0.0); }
    let price_ratio_f64 = reserve1_f64 / reserve0_f64;
    let decimal_diff: i32 = (decimals0 as i32) - (decimals1 as i32);
    let adjustment = 10f64.powi(decimal_diff);
    Ok(price_ratio_f64 * adjustment)
}

// Parse f64 to U256 based on decimals
fn f64_to_wei(amount_f64: f64, decimals: u32) -> Result<U256> {
    if !amount_f64.is_finite() {
        return Err(eyre::eyre!("Cannot convert non-finite f64 to U256"));
    }
    let multiplier = 10f64.powi(decimals as i32);
    let wei_f64 = (amount_f64 * multiplier).round();
    if wei_f64 < 0.0 {
         return Err(eyre::eyre!("Cannot convert negative f64 to U256"));
    }
    // Use string conversion for large numbers to avoid precision loss
     U256::from_dec_str(&format!("{:.0}", wei_f64))
         .map_err(|e| eyre::eyre!("Failed to parse f64 '{}' to U256: {}", amount_f64, e))
}

// --- Simulation Helper ---
async fn simulate_swap(
    dex_type: &str, // "UniV3" or "VeloV2"
    token_in: Address,
    token_out: Address,
    amount_in: U256,
    // Pass necessary contracts/addresses
    velo_router: &VelodromeRouter<Provider<Http>>, // Need router instance for Velo
    quoter: &QuoterV2<Provider<Http>>,           // Need quoter instance for UniV3
    // Add flags based on pool/route if needed
    is_velo_route_stable: bool, // Flag for Velodrome stable/volatile route
    uni_pool_fee: u32,          // Uniswap Pool Fee Tier (e.g., 500 for 0.05%)
) -> Result<U256> {
    println!(
        "    Simulating Swap: {} -> {} Amount: {} on {}",
        token_in, token_out, amount_in, dex_type
    );
    match dex_type {
        "UniV3" => {
            // Call QuoterV2.quoteExactInputSingle
            let params = quoter_v2_bindings::QuoteExactInputSingleParams {
                token_in,
                token_out,
                amount_in,
                fee: uni_pool_fee, // Use the provided fee tier
                sqrt_price_limit_x96: U256::zero(), // No limit for simulation
            };

            let quote_result = quoter.quote_exact_input_single(params).call().await;

            match quote_result {
                // Result structure: (amountOut, sqrtPriceX96After, initializedTicksCrossed, gasEstimate)
                Ok(output) => {
                     println!("      -> UniV3 Quoter Result: AmountOut={}", output.0);
                     Ok(output.0) // return amountOut
                },
                Err(e) => {
                    eprintln!("      -> UniV3 Quoter simulation failed: {}", e);
                    Err(eyre::eyre!("UniV3 Quoter simulation failed: {}", e))
                },
            }
        }
        "VeloV2" => {
            // Use Velodrome Router's getAmountsOut
            let routes = vec![velo_router_bindings::Route {
                 from: token_in,
                 to: token_out,
                 stable: is_velo_route_stable,
                 // Anticipate E0063: missing field `factory` if ABI requires it
                 factory: Address::zero(), // Placeholder
             }];

            // FIX E0308: Pass 'routes' by value (remove '&')
            match velo_router.get_amounts_out(amount_in, routes).call().await {
                Ok(amounts_out) => {
                     if amounts_out.len() >= 2 {
                          println!("      -> VeloV2 getAmountsOut Result: AmountOut={}", amounts_out[1]);
                         Ok(amounts_out[1])
                     } else {
                         eprintln!("      -> VeloV2 getAmountsOut returned unexpected vector length: {:?}", amounts_out);
                         Err(eyre::eyre!("VeloV2 getAmountsOut returned unexpected vector length"))
                     }
                },
                Err(e) => {
                     eprintln!("      -> VeloV2 getAmountsOut simulation failed: {}", e);
                     Err(eyre::eyre!("VeloV2 simulation failed: {}", e))
                 },
            }
        }
        _ => Err(eyre::eyre!("Unsupported DEX type for simulation: {}", dex_type)),
    }
}


// --- Main Execution ---
#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    // --- Load Configuration ---
    println!("Loading configuration...");
    let rpc_url = env::var("LOCAL_RPC_URL").expect("LOCAL_RPC_URL must be set");
    let private_key = env::var("LOCAL_PRIVATE_KEY").expect("LOCAL_PRIVATE_KEY must be set");
    let arb_executor_address_str = env::var("ARBITRAGE_EXECUTOR_ADDRESS").expect("ARBITRAGE_EXECUTOR_ADDRESS must be set");
    let uni_v3_pool_addr_str = env::var("UNI_V3_POOL_ADDR").expect("UNI_V3_POOL_ADDR must be set");
    let velo_v2_pool_addr_str = env::var("VELO_V2_POOL_ADDR").expect("VELO_V2_POOL_ADDR must be set");
    let weth_addr_str = env::var("WETH_ADDRESS").expect("WETH_ADDRESS must be set");
    let usdc_addr_str = env::var("USDC_ADDRESS").expect("USDC_ADDRESS must be set");
    let weth_decimals: u8 = env::var("WETH_DECIMALS")?.parse()?;
    let usdc_decimals: u8 = env::var("USDC_DECIMALS")?.parse()?;
    let velo_router_addr_str = env::var("VELO_V2_ROUTER_ADDR").expect("VELO_V2_ROUTER_ADDR must be set");
    let balancer_vault_addr_str = env::var("BALANCER_VAULT_ADDRESS").expect("BALANCER_VAULT_ADDRESS must be set");
    let quoter_v2_addr_str = env::var("QUOTER_V2_ADDRESS").expect("QUOTER_V2_ADDRESS must be set"); // Load Quoter address
    println!("Configuration loaded.");

    // --- Setup Provider & Client ---
    println!("Setting up provider & client...");
    let provider = Provider::<Http>::try_from(rpc_url)?;
    let provider = Arc::new(provider);
    let chain_id = provider.get_chainid().await?.as_u64();
    println!("RPC OK. Chain ID: {}", chain_id);
    let wallet = private_key.parse::<LocalWallet>()?.with_chain_id(chain_id);
    let client = SignerMiddleware::new(provider.clone(), wallet.clone());
    let client = Arc::new(client);
    println!("Provider & client setup complete.");

    // --- Parse Addresses ---
    println!("Parsing addresses...");
    let uni_v3_pool_address = uni_v3_pool_addr_str.parse::<Address>()?;
    let velo_v2_pool_address = velo_v2_pool_addr_str.parse::<Address>()?;
    let weth_address = weth_addr_str.parse::<Address>()?;
    let usdc_address = usdc_addr_str.parse::<Address>()?;
    let arb_executor_address = arb_executor_address_str.parse::<Address>()?;
    let velo_router_address = velo_router_addr_str.parse::<Address>()?;
    let balancer_vault_address = balancer_vault_addr_str.parse::<Address>()?;
    let quoter_v2_address = quoter_v2_addr_str.parse::<Address>()?; // Parse Quoter address
    println!("Addresses parsed.");

    // --- Create Contract Instances ---
    println!("Creating contract instances...");
    let uni_v3_pool = UniswapV3Pool::new(uni_v3_pool_address, provider.clone());
    let velo_v2_pool = VelodromeV2Pool::new(velo_v2_pool_address, provider.clone());
    // Use provider for view calls, client for transactions
    let velo_router = VelodromeRouter::new(velo_router_address, provider.clone()); // Use provider for getAmountsOut
    let balancer_vault = BalancerVault::new(balancer_vault_address, client.clone()); // Use client for flashloan tx
    let quoter = QuoterV2::new(quoter_v2_address, provider.clone()); // Use provider for quote calls
    println!("Contract instances created.");

    // --- Determine Pool/Token Details ---
    println!("Fetching pool details...");
    // Velodrome Pool Details
    let velo_token0 = velo_v2_pool.token_0().call().await?;
    let velo_token1 = velo_v2_pool.token_1().call().await?;
    let velo_is_stable = velo_v2_pool.stable().call().await?; // Fetch if pool is stable
    println!("  Velo Pool Stable: {}", velo_is_stable);
    let (velo_decimals0, velo_decimals1, velo_t0_is_weth) = if velo_token0 == weth_address && velo_token1 == usdc_address {
        (weth_decimals, usdc_decimals, true)
    } else if velo_token0 == usdc_address && velo_token1 == weth_address {
        (usdc_decimals, weth_decimals, false)
    } else {
        eyre::bail!("Velo pool tokens ({:?}, {:?}) do not match WETH/USDC addresses in .env", velo_token0, velo_token1);
    };

    // Uniswap Pool Details
    let uni_token0 = uni_v3_pool.token_0().call().await?;
    let uni_token1 = uni_v3_pool.token_1().call().await?;
    let uni_fee = uni_v3_pool.fee().call().await?; // Fetch fee tier
    println!("  Uni Pool Fee: {}", uni_fee);
     if !(uni_token0 == weth_address && uni_token1 == usdc_address) && !(uni_token0 == usdc_address && uni_token1 == weth_address) {
         eyre::bail!("Uni pool tokens ({:?}, {:?}) do not match WETH/USDC addresses in .env", uni_token0, uni_token1);
     }
    // Assume WETH is token0 for price calculation consistency if needed, but simulation uses addresses directly
    let uni_decimals0 = weth_decimals;
    let uni_decimals1 = usdc_decimals;
    println!("Pool details fetched.");


    println!("\n--- Performing Single Test Run ---");

    let simulation_amount_weth_wei = f64_to_wei(SIMULATION_AMOUNT_WETH, weth_decimals as u32)?;
    println!("Simulating with {} WETH ({})", SIMULATION_AMOUNT_WETH, simulation_amount_weth_wei);


    // --- Fetch Prices (for initial detection) ---
    println!("Fetching prices...");
    let uni_price_result: Result<f64> = async {
        uni_v3_pool.slot_0().call().await
            .map_err(|e| eyre::eyre!("RPC Error fetching UniV3 slot0: {}", e))
            .and_then(|slot0_data| {
                // Ensure price is WETH/USDC regardless of pool token order
                let price_native = v3_price_from_sqrt(slot0_data.0, uni_decimals0, uni_decimals1)?;
                 if uni_token0 == weth_address { Ok(price_native) } else { if price_native.abs() < f64::EPSILON {Ok(0.0)} else {Ok(1.0 / price_native)} }
            })
    }.await;

    let velo_price_result: Result<f64> = async {
         velo_v2_pool.get_reserves().call().await
            .map_err(|e| eyre::eyre!("RPC Error fetching Velo reserves: {}", e))
            .and_then(|reserves| {
                let price = v2_price_from_reserves(reserves.0.into(), reserves.1.into(), velo_decimals0, velo_decimals1)?;
                Ok(if velo_t0_is_weth { price } else { if price.abs() < f64::EPSILON { 0.0 } else { 1.0 / price } })
         })
    }.await;
     println!("Prices fetched.");

    // --- Arbitrage Detection & Simulation ---
        match (uni_price_result, velo_price_result) {
            (Ok(p_uni), Ok(p_velo)) => {
                 println!("UniV3 Price (WETH/USDC): {:.6}", p_uni);
                 println!("VeloV2 Price (WETH/USDC): {:.6}", p_velo);
                 let price_diff = (p_uni - p_velo).abs();
                 let base_price = p_uni.min(p_velo); // Use lower price as base for spread %
                 let spread_percentage = if base_price > 1e-18 { (price_diff / base_price) * 100.0 } else { 0.0 };
                 println!("-> Spread (Abs): {:.4}%", spread_percentage);

                 if spread_percentage > ARBITRAGE_THRESHOLD_PERCENTAGE {
                    println!(">>> Arbitrage Opportunity DETECTED! Spread: {:.4}%", spread_percentage);

                    let amount_in_wei = simulation_amount_weth_wei; // Starting loan amount
                    let token_in = weth_address;  // Loan token
                    let token_out = usdc_address; // Intermediate token

                    // Determine direction
                    let (buy_dex, sell_dex, buy_dex_stable, sell_dex_stable, buy_dex_fee, sell_dex_fee) = if p_uni < p_velo {
                        // Buy UniV3 (Low), Sell VeloV2 (High)
                        ("UniV3", "VeloV2", false, velo_is_stable, uni_fee, 0u32) // Velo fee not needed for getAmountsOut
                    } else {
                        // Buy VeloV2 (Low), Sell UniV3 (High)
                        ("VeloV2", "UniV3", velo_is_stable, false, 0u32, uni_fee)
                    };
                    println!("    Direction: Buy {} (Low), Sell {} (High)", buy_dex, sell_dex);

                    // --- Accurate Simulation ---
                    // Applied Scope Fix: Returns Result<U256> for final amount
                    let simulation_result: Result<U256> = async {

                        // Simulate Swap 1 (Buy Low)
                        let amount_out_intermediate_wei = simulate_swap(
                            buy_dex,
                            token_in,  // WETH
                            token_out, // USDC
                            amount_in_wei,
                            &velo_router,
                            &quoter,
                            buy_dex_stable,
                            buy_dex_fee,
                        ).await?;
                        // Log intermediate amount here (Simplified)
                        println!("    Sim Swap 1 Out: {}", amount_out_intermediate_wei);

                        if amount_out_intermediate_wei.is_zero() {
                            eyre::bail!("Simulation Swap 1 resulted in zero output.");
                        }

                        // Simulate Swap 2 (Sell High)
                         let amount_out_final_wei = simulate_swap( // Define final amount here
                            sell_dex,
                            token_out, // USDC (Input for Swap 2)
                            token_in,  // WETH (Output for Swap 2)
                            amount_out_intermediate_wei, // Use output from Swap 1 as input
                            &velo_router,
                            &quoter,
                            sell_dex_stable,
                            sell_dex_fee,
                         ).await?;
                         // Log final amount here (Simplified)
                         println!("    Sim Swap 2 Out: {}", amount_out_final_wei);

                        // Return only final amount (Scope fix)
                        Ok(amount_out_final_wei)
                    }.await;


                    match simulation_result {
                         // Receive only final_amount here (Scope fix)
                        Ok(final_amount) => {
                            // --- Profit Calculation (using simulated amounts) ---
                            let gross_profit_wei = final_amount.saturating_sub(amount_in_wei);
                            println!("    Simulated Gross Profit (WETH): {}", format_units(gross_profit_wei, "ether")?);

                            // Estimate Gas Cost (Placeholder - To be replaced in next task)
                            let gas_price = provider.get_gas_price().await?;
                            let estimated_gas_units = U256::from(500_000); // Increased placeholder gas
                            let gas_cost_wei = gas_price * estimated_gas_units;
                            println!("    Estimated Gas Cost (Placeholder): {} Wei ({:.8} ETH)", gas_cost_wei, format_units(gas_cost_wei, "ether")?);

                            // Calculate Flash Loan Fee (if applicable)
                            let fee_numerator = U256::from((FLASH_LOAN_FEE_RATE * 10000.0) as u128);
                            let fee_denominator = U256::from(10000);
                            let flash_loan_fee_wei = amount_in_wei * fee_numerator / fee_denominator;
                             println!("    Estimated Flash Loan Fee: {} Wei ({:.8} WETH)", flash_loan_fee_wei, format_units(flash_loan_fee_wei, "ether")?);

                            let total_cost_wei = gas_cost_wei + flash_loan_fee_wei; // Costs are in native token (ETH/WETH)

                            // --- Decision ---
                            if final_amount > amount_in_wei && gross_profit_wei > total_cost_wei {
                                let net_profit_weth_wei = gross_profit_wei.saturating_sub(total_cost_wei);
                                println!("    Simulated NET Profit: {} Wei ({:.8} WETH)", net_profit_weth_wei, format_units(net_profit_weth_wei, "ether")?);
                                println!("    >>> Simulation SUCCESSFUL - Profit Expected <<<");
                                // TODO: Encode userData
                                // TODO: Estimate Gas Accurately
                                // TODO: Send flash loan tx
                            } else {
                                println!("    Simulated NET Loss/Insufficient Profit: Gross Profit {} <= Total Cost {}", gross_profit_wei, total_cost_wei);
                                println!("    >>> Simulation FAILED - Aborting Execution <<<");
                            }
                        },
                        Err(sim_err) => {
                            eprintln!("! Simulation Error: {}", sim_err);
                             println!("    >>> Simulation FAILED - Aborting Execution <<<");
                        }
                    } // End match simulation_result


                 } // End if spread > threshold
            }, // End Ok match prices
            (Err(e), _) => eprintln!("! Error Processing UniV3 Price: {}", e),
            (_, Err(e)) => eprintln!("! Error Processing VeloV2 Price: {}", e),
        } // End match prices

    println!("\n--- Run Complete ---");
    Ok(())
} // End main