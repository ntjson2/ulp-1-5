// bot/src/transaction.rs

use crate::bindings::BalancerVault;
use crate::config::Config;
use crate::gas::{fetch_gas_price as fetch_gas_price_internal}; // Removed InternalGasInfo (unused warning)
use crate::path_optimizer::RouteCandidate;
use crate::state::AppState;
use ethers::utils::{format_units, parse_units, hex::FromHex}; 

use ethers::{
    core::types::{
        transaction::eip2718::TypedTransaction, Address, Bytes, TransactionReceipt, Eip1559TransactionRequest,
        U256, U64, I256, TxHash, // Removed H160 (unused warning)
    },
    middleware::SignerMiddleware,
    providers::{Http, Provider, Middleware},
    signers::{LocalWallet, Signer},
};
use eyre::{eyre, Result, WrapErr};
use rand;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use std::error::Error as StdError;
use tokio::sync::Mutex;
use tokio::time::timeout;
use tracing::{debug, error, info, instrument, warn};

const GAS_LIMIT_DEFAULT: u64 = 500_000;

// --- Constants ---
const TX_SUBMISSION_TIMEOUT_SECS: u64 = 15;
const TX_CONFIRMATION_TIMEOUT_SECS: u64 = 90;
const TX_POLLING_INTERVAL_MS: u64 = 5000;
const TX_STALLED_POLL_COUNT: u32 = 6;
const TX_SUCCESS_STATUS: U64 = U64([1]);
const GAS_ESTIMATION_TIMEOUT_SECS: u64 = 20; // Timeout for gas estimation step

// --- Structs ---
#[derive(Debug, Clone, Copy)] pub struct GasInfo { pub max_fee_per_gas: U256, pub max_priority_fee_per_gas: U256 }
#[derive(Debug)] pub struct NonceManager { current_nonce: Mutex<Option<U256>>, wallet_address: Address }
#[derive(Serialize, Debug)] 
pub struct AlchemyPrivateTxParams<'a> { tx: &'a str }

// --- NonceManager Impl ---
// (remains unchanged)
impl NonceManager {
    pub fn new(wallet_address: Address) -> Self {
        Self {
            current_nonce: Mutex::new(None),
            wallet_address,
        }
    }

    pub async fn get_next_nonce<M: Middleware>(&self, client: Arc<M>) -> Result<U256>
    where <M as Middleware>::Error: StdError + Send + Sync + 'static { // StdError path corrected
        let mut nonce_guard = self.current_nonce.lock().await;
        if let Some(nonce) = *nonce_guard {
            let next_nonce = nonce + U256::one();
            *nonce_guard = Some(next_nonce);
            Ok(next_nonce)
        } else {
            match client.get_transaction_count(self.wallet_address, None).await {
                Ok(initial_nonce) => {
                    *nonce_guard = Some(initial_nonce);
                    Ok(initial_nonce)
                }
                Err(e) => {
                    Err(eyre!(e).wrap_err("Failed to fetch initial nonce"))
                }
            }
        }
    }

    pub async fn confirm_nonce_used(&self, _used_nonce: U256) {
        // Logic to confirm nonce, potentially adjust current_nonce if it was higher
        // For now, this is a placeholder. A robust implementation might involve
        // re-fetching from network if there's a mismatch.
        // let mut nonce_guard = self.current_nonce.lock().await;
        // if let Some(current) = *nonce_guard {
        //     if used_nonce >= current {
        //         *nonce_guard = Some(used_nonce + U256::one());
        //     }
        // }
        debug!(nonce = %_used_nonce, "Nonce confirmed as used.");
    }

    pub async fn handle_nonce_error(&self) {
        warn!("Nonce error detected. Resetting internal nonce to re-fetch from network.");
        let mut nonce_guard = self.current_nonce.lock().await;
        *nonce_guard = None; // Reset to force re-fetch on next get_next_nonce
    }
}


// --- fetch_gas_price function ---
// (remains unchanged)
#[instrument(skip(client, config), level = "debug")]
pub async fn fetch_gas_price(client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, config: &Config) -> Result<GasInfo> {
    debug!("Fetching EIP-1559 gas prices...");
    let max_prio_gwei_str = config.max_priority_fee_per_gas_gwei.to_string();
    let fallback_prio_gwei_str = config.fallback_gas_price_gwei.unwrap_or(config.max_priority_fee_per_gas_gwei).to_string();
    let max_prio_wei: U256 = ethers::utils::parse_units(&max_prio_gwei_str, "gwei")?.into();
    let fallback_prio_wei: U256 = ethers::utils::parse_units(&fallback_prio_gwei_str, "gwei")?.into();
    match client.estimate_eip1559_fees(None).await {
         Ok((max_fee, max_priority_fee)) => {
            let final_max_priority_fee = max_priority_fee.min(max_prio_wei);
            let current_base_fee = client.get_gas_price().await.unwrap_or(max_fee);
            let required_max_fee = current_base_fee + final_max_priority_fee;
            let final_max_fee = max_fee.max(required_max_fee);
            debug!(%final_max_fee, %final_max_priority_fee, "EIP-1559 fees estimated.");
            Ok(GasInfo { max_fee_per_gas: final_max_fee, max_priority_fee_per_gas: final_max_priority_fee })
        }
        Err(e) => {
            warn!(error = ?e, "EIP-1559 fee estimation failed, attempting fallback.");
             match client.get_gas_price().await {
                 Ok(legacy_price) => {
                     let final_max_priority_fee = fallback_prio_wei.min(max_prio_wei);
                     let final_max_fee = legacy_price + final_max_priority_fee;
                     debug!(%final_max_fee, %final_max_priority_fee, "Using legacy price fallback gas prices.");
                     Ok(GasInfo { max_fee_per_gas: final_max_fee, max_priority_fee_per_gas: final_max_priority_fee })
                 }
                 Err(e_legacy) => {
                    error!(error_eip1559=?e, error_legacy=?e_legacy, "ALERT: Both EIP-1559 and legacy gas price fetch failed.");
                     let final_max_priority_fee = fallback_prio_wei.min(max_prio_wei);
                     let final_max_fee = final_max_priority_fee * 2;
                     warn!(%final_max_fee, %final_max_priority_fee, "Using purely config-based fallback gas prices. Risk of underpricing.");
                     Ok(GasInfo { max_fee_per_gas: final_max_fee, max_priority_fee_per_gas: final_max_priority_fee })
                 }
             }
        }
    }
}


/// Calculates the minimum profit required for the transaction to be considered successful on-chain.
// (remains unchanged)
#[instrument(level = "debug", skip(config))]
fn calculate_min_profit_threshold(
    simulated_net_profit_wei: I256,
    config: &Config,
) -> Result<I256> {
    // Corrected field name: e.g., profit_sharing_bps_for_devs
    let profit_sharing_percentage_val = config.profit_sharing_bps_for_devs.unwrap_or(0); 
    let min_profit_after_sharing = simulated_net_profit_wei * I256::from(profit_sharing_percentage_val) / I256::from(10000); 
    
    // Corrected field name: e.g., min_flat_profit_weth_threshold
    let min_flat_profit_weth_val = config.min_flat_profit_weth_threshold.unwrap_or(0.0001); 
    let min_flat_profit_wei_str = min_flat_profit_weth_val.to_string();
    let min_flat_profit_wei = I256::try_from(parse_units(&min_flat_profit_wei_str, config.weth_decimals as u32)?)
        .map_err(|_| eyre!("Failed to convert min_flat_profit_weth to I256"))?;

    Ok(std::cmp::max(min_profit_after_sharing, min_flat_profit_wei))
}


/// Constructs, submits, and monitors the arbitrage transaction using polling.
#[instrument(skip_all, fields(
    route_id = %route.id(),
    loan_amount_wei = %loan_amount_wei,
    // Assuming weth_decimals is available in config
    loan_eth = %format_units(loan_amount_wei, app_state.config.weth_decimals as i32).unwrap_or_default(),
    profit_wei = %min_profit_wei, 
    profit_eth = %format_units(min_profit_wei, app_state.config.weth_decimals as i32).unwrap_or_default()
))]
pub async fn submit_arbitrage_transaction(
    route: &RouteCandidate,
    loan_amount_wei: U256,
    min_profit_wei: U256,
    executor_address: Address,
    app_state: Arc<AppState>,
) -> Result<TransactionReceipt> {
    let config = &app_state.config;
    let client = &app_state.client;
    let nonce_manager = &app_state.nonce_manager;

    info!(target_profit_wei = %min_profit_wei, "Attempting to submit arbitrage transaction.");
    debug!(initial_sim_profit_wei = %min_profit_wei, initial_loan_amount_wei = %loan_amount_wei, "Initial simulation results for submission");

    let gas_info = fetch_gas_price_internal(client.clone(), config).await.wrap_err("ALERT: Failed gas price fetch pre-submission")?; // Use renamed import
    
    let simulated_profit_i256 = I256::try_from(min_profit_wei)
        .map_err(|_| eyre!("Failed to convert U256 min_profit_wei to I256"))?;

    let min_profit_threshold_wei_i256 = calculate_min_profit_threshold(simulated_profit_i256, config)
        .wrap_err("Failed to calculate minimum profit threshold")?;
    
    let min_profit_threshold_for_check = if min_profit_threshold_wei_i256.is_negative() {
        U256::zero() 
    } else {
        min_profit_threshold_wei_i256.into_raw()
    };

    debug!(calculated_threshold_profit_wei = %min_profit_threshold_for_check, "Calculated minimum profit threshold for transaction");

    // Corrected field name: e.g., allow_submission_zero_profit
    if min_profit_threshold_for_check == U256::zero() && !config.allow_submission_zero_profit.unwrap_or(false) { 
        warn!(sim_profit = %min_profit_wei, "Simulated profit is zero or below threshold, and zero profit submission is disallowed. Skipping.");
        return Err(eyre!("Simulated profit is zero or below threshold, skipping submission."));
    }
    // Ensure config.max_loan_amount_weth is U256 or convert loan_amount_wei to f64 for comparison
    let max_loan_wei = parse_units(config.max_loan_amount_weth, config.weth_decimals as u32)?.into(); // Cast u8 to u32
    if loan_amount_wei > max_loan_wei {
        warn!(loan_amount = %loan_amount_wei, max_loan = %max_loan_wei, "Loan amount exceeds maximum allowed. Skipping.");
        return Err(eyre!("Loan amount exceeds maximum."));
    }

    let salt_bytes: [u8; 32] = rand::random();
    let salt = U256::from_big_endian(&salt_bytes);

    let user_data = crate::encoding::encode_user_data(
        route.buy_pool_addr,
        route.sell_pool_addr,
        config.usdc_address, // Assuming H160
        route.zero_for_one_a,
        route.buy_dex_type.is_velo_style(),
        route.sell_dex_type.is_velo_style(),
        config.arb_executor_address.ok_or_else(|| eyre!("Arb executor address not configured for userdata"))?,
        min_profit_threshold_for_check,
        salt
    )?;

    let flash_loan_args = (
        executor_address, 
        vec![config.weth_address], 
        vec![loan_amount_wei],
        user_data.clone(),
    );
    
    let nonce = nonce_manager.get_next_nonce(client.clone()).await.wrap_err("ALERT: Nonce fetch failed pre-submission")?;
    
    let balancer_contract = BalancerVault::new(config.balancer_vault_address, client.clone()); // Assuming H160
    let calldata = balancer_contract
        .flash_loan(
            flash_loan_args.0,
            flash_loan_args.1.clone(),
            flash_loan_args.2.clone(),
            flash_loan_args.3,
        )
        .calldata()
        .ok_or_else(|| eyre!("ALERT: Calldata generation failed"))?;

    let chain_id_u256 = client.inner().provider().get_chainid().await?;
    let chain_id_u64 = U64::from(chain_id_u256.as_u64()); // Correct conversion
    let mut tx_request = Eip1559TransactionRequest::new()
        .to(config.balancer_vault_address) // Assuming H160
        .data(calldata.clone())
        .nonce(nonce)
        .chain_id(chain_id_u64); 

    // Corrected field name: e.g., submission_gas_limit_default
    tx_request.gas = Some(config.submission_gas_limit_default.unwrap_or(GAS_LIMIT_DEFAULT).into()); 
    tx_request.max_fee_per_gas = Some(gas_info.max_fee_per_gas);
    tx_request.max_priority_fee_per_gas = Some(gas_info.max_priority_fee_per_gas);
    
    // Corrected field name: e.g., submission_gas_price_gwei_fixed
    if let Some(gas_price_override_gwei) = config.submission_gas_price_gwei_fixed { 
        let gas_price_override_wei = parse_units(gas_price_override_gwei, "gwei")?.into();
        tx_request = tx_request.max_fee_per_gas(gas_price_override_wei).max_priority_fee_per_gas(gas_price_override_wei);
        warn!("gas_price_gwei_override is set, using it for both max_fee and max_priority_fee.");
    }

    let typed_tx: TypedTransaction = tx_request.into();

    let signature = client.signer().sign_transaction(&typed_tx).await.wrap_err("ALERT: Signing failed pre-submission")?;
    let rlp_signed = typed_tx.rlp_signed(&signature);
    let rlp_hex_str = hex::encode(rlp_signed.as_ref()); 

    // Corrected field name: e.g., submission_timeout_duration_seconds
    match timeout(Duration::from_secs(config.submission_timeout_duration_seconds.unwrap_or(60)), submit_sequentially( config, client.provider(), client.clone(), &rlp_hex_str, &rlp_signed )).await { 
        Ok(Ok(receipt_opt)) => {
            if let Some(receipt) = receipt_opt { // receipt is TransactionReceipt
                if receipt.status == Some(U64::from(1)) {
                    info!(tx_hash=?receipt.transaction_hash, "✅ Arbitrage transaction successful!");
                    nonce_manager.confirm_nonce_used(nonce).await;
                    return Ok(receipt);
                } else {
                    error!(tx_hash=?receipt.transaction_hash, "❌ Arbitrage transaction failed on-chain (status 0).");
                    nonce_manager.confirm_nonce_used(nonce).await;
                    return Err(eyre!("Transaction {:?} failed on-chain (status 0)", receipt.transaction_hash));
                }
            } else {
                 warn!("Transaction submission via all relays failed or timed out without a definitive receipt. Nonce status uncertain.");
                 return Err(eyre!("Transaction submission failed via all relays."));
            }
        }
        Ok(Err(e)) => {
            error!("Error during transaction submission sequence: {:?}", e);
            if e.to_string().contains("nonce too low") || e.to_string().contains("replacement transaction underpriced") {
                nonce_manager.handle_nonce_error().await;
            }
            return Err(e.wrap_err("Transaction submission sequence failed"));
        }
        Err(_) => {
            // Corrected field name
            error!("Transaction submission timed out after {} seconds.", config.submission_timeout_duration_seconds.unwrap_or(60)); 
            nonce_manager.handle_nonce_error().await;
            return Err(eyre!("Transaction submission timed out overall."));
        }
    }
}

pub async fn submit_sequentially<P: Middleware>(
    config: &Config,
    provider: &P,
    client_for_receipt: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    rlp_hex: &str, // Pass as string
    _rlp_bytes: &Bytes 
) -> Result<Option<TransactionReceipt>> 
where <P as Middleware>::Error: StdError + Send + Sync + 'static {
    // Corrected field name: e.g., transaction_relay_urls
    let configured_relays = config.transaction_relay_urls.clone().unwrap_or_default(); 
    if configured_relays.is_empty() {
        info!("No relays configured. Submitting directly to provider.");
        // rlp_hex already includes "0x" if it's from hex::encode and formatted that way.
        // If not, Bytes::from_hex expects no "0x" prefix.
        let bytes_to_send = Bytes::from_hex(rlp_hex.strip_prefix("0x").unwrap_or(rlp_hex))
            .map_err(|e| eyre!("Failed to decode RLP hex for direct submission: {}", e))?;

        match provider.send_raw_transaction(bytes_to_send).await {
            Ok(pending_tx) => {
                let tx_hash = pending_tx.tx_hash();
                info!(?tx_hash, "Transaction submitted directly, awaiting receipt...");
                // Wait for receipt
                match client_for_receipt.get_transaction_receipt(tx_hash).await.wrap_err("Failed to get receipt for direct submission") {
                    Ok(Some(receipt)) => return Ok(Some(receipt)),
                    Ok(None) => {
                        warn!(?tx_hash, "Direct submission: No receipt after waiting (tx might be pending or dropped).");
                        return Ok(None); // Or Err if no receipt is unacceptable
                    }
                    Err(e) => {
                        error!(?tx_hash, error=?e, "Direct submission: Error waiting for receipt.");
                        return Err(e);
                    }
                }
            }
            Err(e) => {
                error!(error=?e, "Direct transaction submission failed.");
                return Err(eyre!(e).wrap_err("Direct transaction submission failed"));
            }
        }
    }

    for relay_url in &configured_relays { // Use corrected variable
        info!("Attempting submission via relay: {}", relay_url);
        // This part needs a proper RPC client for eth_sendRawTransactionJsonRpc
        // For simplicity, assuming a generic HTTP client or a specialized RPC client
        // Placeholder for actual relay submission logic:
        // let response = http_client.post(relay_url).json({"jsonrpc":"2.0","method":"eth_sendRawTransaction","params":[rlp_hex],"id":1}).send().await;
        // match response { Ok(res) if res.status().is_success() => { let tx_hash = ...; return Ok(tx_hash_from_relay_response) } ... }
        warn!("Relay submission logic is a placeholder for URL: {}", relay_url);
    }
    Ok(None) // If all relays fail
}

async fn send_alchemy_private_tx( provider: &Provider<Http>, rlp_hex: &str ) -> Result<TxHash> {
    let method="alchemy_sendPrivateTransaction";
    let params=AlchemyPrivateTxParams{tx:rlp_hex};
    provider.request(method, [serde_json::to_value(params)?]).await
        .map_err(|e|eyre!("Alchemy RPC error: {}", e.to_string()))
}
async fn send_flashbots_private_tx( provider: &Provider<Http>, rlp_hex: &str ) -> Result<TxHash> {
    let method="eth_sendPrivateRawTransaction";
    let params=[rlp_hex];
    provider.request(method,params).await
        .map_err(|e|eyre!("Flashbots RPC error: {}", e.to_string()))
}