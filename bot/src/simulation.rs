// src/simulation.rs

use crate::bindings::{
    quoter_v2 as quoter_v2_bindings, velodrome_router as velo_router_bindings,
    AerodromeRouter, QuoterV2, VelodromeRouter, UniswapV3Pool, // Added UniV3Pool for potential future observe call
};
use crate::config::Config; // Import Config struct
use crate::encoding::encode_user_data;
use crate::gas::estimate_flash_loan_gas;
use crate::state::{AppState, DexType, PoolSnapshot, PoolState};
use crate::path_optimizer::RouteCandidate;
use crate::utils::{f64_to_wei, ToF64Lossy, v2_price_from_reserves, v3_price_from_sqrt};
use ethers::{
    prelude::{Http, LocalWallet, Middleware, Provider, SignerMiddleware},
    types::{Address, I256, U256},
    utils::{format_units, parse_units},
};
use eyre::{eyre, Result, WrapErr};
use std::{str::FromStr, sync::Arc};
use tracing::{debug, error, info, instrument, trace, warn};

// Percentage of pool reserve to consider as max loan size for V2/Aero pools
const V2_RESERVE_PERCENTAGE_LIMIT: u64 = 5;

// --- simulate_swap function (unchanged) ---
#[allow(clippy::too_many_arguments)]
#[instrument(skip(app_state, client), level = "trace", fields(dex = dex_type_str, amount_in = %amount_in))]
pub async fn simulate_swap( app_state: Arc<AppState>, client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, dex_type_str: &str, token_in: Address, token_out: Address, amount_in: U256, is_stable_route: bool, uni_pool_fee: u32, ) -> Result<U256> { /* ... implementation unchanged ... */
    trace!("Simulating swap..."); match dex_type_str {
        "UniV3" => { let q_addr=app_state.uni_quoter_addr.ok_or_else(||eyre!("Quoter missing"))?; let q=QuoterV2::new(q_addr, client); let p = quoter_v2_bindings::QuoteExactInputSingleParams{token_in, token_out, amount_in, fee:uni_pool_fee, sqrt_price_limit_x96: U256::zero()}; q.quote_exact_input_single(p).call().await.map(|o|o.0).map_err(|e|eyre!(e)) },
        "VeloV2"|"Aero" => { let r_addr = if dex_type_str=="VeloV2"{app_state.velo_router_addr.ok_or_else(||eyre!("Velo R missing"))?} else {app_state.aero_router_addr.ok_or_else(||eyre!("Aero R missing"))?}; let r=VelodromeRouter::new(r_addr, client); let routes = vec![velo_router_bindings::Route{from:token_in, to:token_out, stable:is_stable_route, factory: Address::zero()}]; match r.get_amounts_out(amount_in, routes).await { Ok(a) if a.len()>=2 => Ok(a[1]), Ok(a)=>Err(eyre!("Invalid len")), Err(e)=>Err(eyre!(e))} },
        _ => Err(eyre!("Unsupported DEX: {}", dex_type_str)),
    }
}

// --- Profit Calculation Helper (unchanged) ---
#[allow(clippy::too_many_arguments)]
#[instrument(skip_all, level = "debug", fields( amount_in_wei = %amount_in_wei ))]
pub async fn calculate_net_profit( app_state: Arc<AppState>, client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, route: &RouteCandidate, amount_in_wei: U256, gas_price_gwei: f64, gas_limit_buffer_percentage: u64, min_flashloan_gas_limit: u64, ) -> Result<I256> { /* ... implementation unchanged ... */
    let config=&app_state.config; let token_in=route.token_in; let token_out=route.token_out; trace!("Calculating net profit...");
    let buy_dex_str = route.buy_dex_type.to_string(); let amount_out_inter = simulate_swap( app_state.clone(), client.clone(), &buy_dex_str, token_in, token_out, amount_in_wei, route.buy_pool_stable.unwrap_or(false), route.buy_pool_fee.unwrap_or(0), ).await?; if amount_out_inter.is_zero(){return Ok(I256::min_value());}
    let sell_dex_str = route.sell_dex_type.to_string(); let final_amount_out = simulate_swap( app_state.clone(), client.clone(), &sell_dex_str, token_out, token_in, amount_out_inter, route.sell_pool_stable.unwrap_or(false), route.sell_pool_fee.unwrap_or(0), ).await?;
    let gross_profit = I256::from_raw(final_amount_out) - I256::from_raw(amount_in_wei); if gross_profit <= I256::zero() { return Ok(gross_profit); }
    let gas_price_wei = parse_units(gas_price_gwei, "gwei")?; let effective_router = if route.buy_dex_type.is_velo_style()||route.sell_dex_type.is_velo_style(){if route.buy_dex_type==DexType::Aerodrome||route.sell_dex_type==DexType::Aerodrome{config.aerodrome_router_addr.ok_or_else(||eyre!("Aero router needed"))?}else{config.velo_router_addr}}else{config.velo_router_addr}; let user_data=encode_user_data(route.buy_pool_addr,route.sell_pool_addr,token_out,route.zero_for_one_a,route.buy_dex_type.is_velo_style(),route.sell_dex_type.is_velo_style(),effective_router,U256::zero(),U256::zero())?; let gas_est = estimate_flash_loan_gas(client.clone(),config.balancer_vault_address,config.arb_executor_address.unwrap(),token_in,amount_in_wei,user_data).await?;
    let gas_limit = std::cmp::max(gas_est*(100+gas_limit_buffer_percentage)/100, U256::from(min_flashloan_gas_limit)); let gas_cost=gas_price_wei*gas_limit;
    let total_cost=gas_cost; let net_profit=gross_profit-I256::from_raw(total_cost); debug!(net_profit = %net_profit, "Net profit calculated"); Ok(net_profit)
}


// --- Optimal Loan Amount Search Function (unchanged) ---
#[allow(clippy::too_many_arguments)]
#[instrument(skip_all, level = "info", fields( route = ?route ))]
pub async fn find_optimal_loan_amount( client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, app_state: Arc<AppState>, route: &RouteCandidate, buy_pool_snapshot: Option<&PoolSnapshot>, sell_pool_snapshot: Option<&PoolSnapshot>, gas_price_gwei: f64, ) -> Result<Option<(U256, I256)>> { /* ... implementation unchanged ... */
    info!("Searching optimal loan amount..."); let config=&app_state.config; let mut best_amt=U256::zero(); let mut max_profit=I256::min_value();
    let min_wei = f64_to_wei(config.min_loan_amount_weth, app_state.weth_decimals as u32)?; let max_wei = f64_to_wei(config.max_loan_amount_weth, app_state.weth_decimals as u32)?;
    let dyn_max_wei = calculate_dynamic_max_loan(max_wei, buy_pool_snapshot, sell_pool_snapshot, route.token_in, config); let dyn_max_eth = format_units(dyn_max_wei, app_state.weth_decimals)?.parse::<f64>().unwrap_or(config.max_loan_amount_weth); info!(config_max=config.max_loan_amount_weth, dynamic_max=dyn_max_eth, "Dynamic max loan");
    let eff_max_wei = std::cmp::min(max_wei, dyn_max_wei); let eff_max_eth = dyn_max_eth.min(config.max_loan_amount_weth);
    let search_min = config.min_loan_amount_weth; let search_max = eff_max_eth; let iter = config.optimal_loan_search_iterations;
    if min_wei>=eff_max_wei || iter<1 || search_min<=0.0 { warn!("Invalid search range"); return Ok(None); }
    let mut tasks = vec![]; debug!(tasks=iter,"Spawning tasks...");
    for i in 0..iter { let r=if iter<=1{0.5}else{i as f64 / (iter-1)as f64}; let amt_f64=search_min+(search_max-search_min)*r; let amt_wei=match f64_to_wei(amt_f64,app_state.weth_decimals as u32){Ok(a)=>a,Err(_)=>continue}; if amt_wei<min_wei||amt_wei>eff_max_wei||amt_wei.is_zero(){continue;} let tc=client.clone(); let tas=app_state.clone(); let tr=route.clone(); tasks.push(tokio::spawn(async move{ let p=calculate_net_profit(tas.clone(), tc.clone(), &tr, amt_wei, gas_price_gwei, tas.config.gas_limit_buffer_percentage, tas.config.min_flashloan_gas_limit).await; (amt_wei, p) })); }
    let results = futures_util::future::join_all(tasks).await; debug!("Collected results: {}",results.len());
    for res in results { if let Ok((amt, Ok(p))) = res { if p > max_profit { max_profit = p; best_amt = amt; }}}
    if max_profit > I256::zero() { let best_eth = format_units(best_amt, "ether")?; let profit_eth = format_units(max_profit.into_raw(), "ether")?; info!(%best_eth, %profit_eth, "Optimal found."); Ok(Some((best_amt, max_profit))) } else { info!("No profitable amount found."); Ok(None) }
}


/// Calculates dynamic max loan. Checks config flag for UniV3 placeholder.
#[instrument(level="debug", skip(buy_pool_snapshot, sell_pool_snapshot, config))]
fn calculate_dynamic_max_loan(
    config_max_loan_wei: U256,
    buy_pool_snapshot: Option<&PoolSnapshot>,
    _sell_pool_snapshot: Option<&PoolSnapshot>, // Mark unused
    loan_token: Address,
    config: &Config, // <-- Pass Config reference
) -> U256 {
    trace!("Calculating dynamic max loan based on pool depth...");
    let mut dynamic_max = config_max_loan_wei;

    if let Some(buy_snap) = buy_pool_snapshot {
        match buy_snap.dex_type {
            DexType::VelodromeV2 | DexType::Aerodrome => {
                let reserve_option = if buy_snap.token0 == loan_token { buy_snap.reserve0 }
                                     else if buy_snap.token1 == loan_token { buy_snap.reserve1 }
                                     else { None };
                if let Some(reserve) = reserve_option {
                    if !reserve.is_zero() {
                         let limit = reserve * U256::from(V2_RESERVE_PERCENTAGE_LIMIT) / U256::from(100);
                         dynamic_max = std::cmp::min(dynamic_max, limit);
                         trace!(pool = %buy_snap.pool_address, dex=%buy_snap.dex_type.to_string(), limit = %limit, "Applied V2/Aero depth limit");
                    } else { dynamic_max = U256::zero(); trace!("Reserve zero"); }
                } else { warn!(pool = %buy_snap.pool_address, "Loan token mismatch"); dynamic_max = U256::zero(); }
            }
            DexType::UniswapV3 => {
                // --- Check Config Flag for UniV3 ---
                if config.enable_univ3_dynamic_sizing { // <-- Check the flag
                    // Flag is enabled, proceed with placeholder logic
                    warn!(pool = %buy_snap.pool_address, "UniV3 dynamic loan sizing ENABLED but NOT IMPLEMENTED. Using config max as limit.");
                    trace!("Accurate UniV3 sizing requires tick liquidity analysis. TODO.");
                    // Effectively uses config_max_loan_wei by not changing dynamic_max further here
                    // Apply min just in case V2 logic somehow increased it above config max (unlikely)
                    dynamic_max = std::cmp::min(dynamic_max, config_max_loan_wei);
                } else {
                    // Flag is disabled (default), explicitly use config max and log at trace level
                    trace!(pool = %buy_snap.pool_address, "UniV3 dynamic sizing disabled by config. Using config max.");
                    dynamic_max = std::cmp::min(dynamic_max, config_max_loan_wei);
                }
            }
            DexType::Unknown => { warn!(pool = %buy_snap.pool_address, "Unknown DEX type"); }
        }
    } else { warn!("Buy pool snapshot missing for dynamic sizing."); }

    // Final check ensures <= config max
    let final_dynamic_max = std::cmp::min(dynamic_max, config_max_loan_wei);
    if final_dynamic_max < config_max_loan_wei { debug!(dynamic_max_wei = %final_dynamic_max, "Dynamic depth limit applied."); }
    else { trace!(dynamic_max_wei = %final_dynamic_max, "Using config max (or less) as limit."); }
    final_dynamic_max
}

// --- Trait Implementations (Unchanged) ---
impl ToString for DexType { fn to_string(&self) -> String { format!("{:?}", self) } }
impl FromStr for DexType { type Err = eyre::Report; fn from_str(s: &str)->Result<Self,Self::Err>{ /* ... */ match s.to_lowercase().as_str(){ "univ3"|"uniswapv3"=>Ok(DexType::UniswapV3), "velov2"|"velodrome"|"velodromev2"=>Ok(DexType::VelodromeV2), "aero"|"aerodrome"=>Ok(DexType::Aerodrome), _=>Err(eyre!("Unknown DEX: {}",s)),}}}
impl DexType { pub fn is_velo_style(&self) -> bool { matches!(self, DexType::VelodromeV2 | DexType::Aerodrome) } }


// END OF FILE: bot/src/simulation.rs