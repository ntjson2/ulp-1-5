// bot/src/transaction.rs

use crate::bindings::BalancerVault;
use crate::config::Config;
use crate::encoding::encode_user_data;
use crate::gas::estimate_flash_loan_gas;
use crate::state::{AppState, DexType};
use crate::path_optimizer::RouteCandidate;
use ethers::{
    prelude::*,
    providers::{MiddlewareError, ProviderError},
    types::{
        transaction::eip2718::TypedTransaction, Address, Bytes, Eip1559TransactionRequest, U256,
        U64, I256, TxHash,
    },
    utils::format_units,
};
use eyre::{eyre, Result, WrapErr};
use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use tokio::time::timeout;
use tracing::{debug, error, info, instrument, warn};

// --- Constants ---
const TX_CONFIRMATION_TIMEOUT_SECS: u64 = 60;
const TX_SUCCESS_STATUS: U64 = U64([1]);
const MIN_PROFIT_BUFFER_BPS: u64 = 10;
const MIN_PROFIT_ABS_BUFFER_WEI: u128 = 5_000_000_000_000;

// --- Structs ---
#[derive(Debug, Clone, Copy)] pub struct GasInfo { pub max_fee_per_gas: U256, pub max_priority_fee_per_gas: U256 }
#[derive(Debug)] pub struct NonceManager { current_nonce: Mutex<Option<U256>>, wallet_address: Address }
#[derive(Serialize, Debug)] #[serde(rename_all = "camelCase")] struct AlchemyPrivateTxParams<'a> { tx: &'a str }

// --- NonceManager Impl ---
impl NonceManager {
    pub fn new(wallet_address: Address) -> Self { Self { current_nonce: Mutex::new(None), wallet_address } }
    pub async fn get_next_nonce<M: Middleware>(&self, client: Arc<M>) -> Result<U256> where M::Error: 'static+Send+Sync {
        let mut g = self.current_nonce.lock().await;
        let n = match *g {
            Some(c) => c + U256::one(),
            None => client.get_transaction_count(self.wallet_address, Some(BlockNumber::Pending.into())).await?
        };
        *g = Some(n);
        Ok(n)
    }
    pub async fn handle_nonce_error(&self) { let mut n = self.current_nonce.lock().await; *n = None; }
    pub async fn confirm_nonce_used(&self, used_nonce: U256) {
        let mut g = self.current_nonce.lock().await;
        if *g == Some(used_nonce) {
            *g = Some(used_nonce + U256::one());
        }
    }
}

// --- fetch_gas_price function ---
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
            let final_max_fee = max_fee.max(final_max_priority_fee);
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
                     warn!(%final_max_fee, %final_max_priority_fee, "Using purely config-based fallback gas prices.");
                     Ok(GasInfo { max_fee_per_gas: final_max_fee, max_priority_fee_per_gas: final_max_priority_fee })
                 }
             }
        }
    }
}


/// Constructs, submits, and monitors the arbitrage transaction. Includes enhanced ALERT logs.
#[instrument(skip_all, level = "info", fields(
    buy_pool = %route.buy_pool_addr,
    sell_pool = %route.sell_pool_addr,
    loan_eth = %format_units(loan_amount_wei, app_state.weth_decimals as i32).unwrap_or_default(),
    sim_profit_wei = %simulated_net_profit_wei,
    tx_hash = tracing::field::Empty
))]
pub async fn submit_arbitrage_transaction(
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    app_state: Arc<AppState>,
    route: RouteCandidate,
    loan_amount_wei: U256,
    simulated_net_profit_wei: I256,
    nonce_manager: Arc<NonceManager>,
) -> Result<TxHash> {
    info!("Attempting submission & monitoring");
    let config = &app_state.config;
    let tx_hash: Option<TxHash>;

    // --- Steps 1-8 (Prepare Tx Data & Sign) ---
    let gas_info = fetch_gas_price(client.clone(), config).await.wrap_err("ALERT: Failed gas price fetch pre-submission")?;
    let bps_buffer = simulated_net_profit_wei.abs() * I256::from(MIN_PROFIT_BUFFER_BPS) / I256::from(10000); let abs_buffer = I256::from(MIN_PROFIT_ABS_BUFFER_WEI); let eff_buffer = std::cmp::max(bps_buffer, abs_buffer); let final_buffer = if simulated_net_profit_wei > I256::zero() { std::cmp::min(eff_buffer, simulated_net_profit_wei - I256::one()) } else { I256::zero() }; let min_profit_wei = simulated_net_profit_wei - final_buffer; let min_profit_wei_u256 = if min_profit_wei > I256::zero() { min_profit_wei.into_raw() } else { U256::one() }; debug!(%min_profit_wei_u256);
    let salt = U256::from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos());
    let effective_router_addr = if route.buy_dex_type.is_velo_style() || route.sell_dex_type.is_velo_style() { if route.buy_dex_type == DexType::Aerodrome { config.aerodrome_router_addr.ok_or_else(|| eyre!("Aero router needed"))? } else { config.velo_router_addr } } else { config.velo_router_addr };
    let user_data = encode_user_data( route.buy_pool_addr, route.sell_pool_addr, app_state.usdc_address, route.zero_for_one_a, route.buy_dex_type.is_velo_style(), route.sell_dex_type.is_velo_style(), effective_router_addr, min_profit_wei_u256, salt )?;
    let estimated_gas_limit = estimate_flash_loan_gas( client.clone(), config.balancer_vault_address, config.arb_executor_address.unwrap(), app_state.weth_address, loan_amount_wei, user_data.clone(), ).await.wrap_err("ALERT: Gas estimation failed pre-submission")?;
    let final_gas_limit = std::cmp::max(estimated_gas_limit * (100 + config.gas_limit_buffer_percentage) / 100, U256::from(config.min_flashloan_gas_limit));
    let nonce = nonce_manager.get_next_nonce(client.clone()).await.wrap_err("ALERT: Nonce fetch failed pre-submission")?;
    let balancer_contract = BalancerVault::new(config.balancer_vault_address, client.clone());
    let calldata = balancer_contract.flash_loan( config.arb_executor_address.unwrap(), vec![app_state.weth_address], vec![loan_amount_wei], user_data, ).calldata().ok_or_else(|| eyre!("Calldata build failed"))?;
    let tx_request = Eip1559TransactionRequest::new().to(config.balancer_vault_address).value(U256::zero()).data(calldata).gas(final_gas_limit).max_fee_per_gas(gas_info.max_fee_per_gas).max_priority_fee_per_gas(gas_info.max_priority_fee_per_gas).nonce(nonce).chain_id(client.signer().chain_id());
    info!(nonce = %nonce, gas_limit = %final_gas_limit, max_fee = %gas_info.max_fee_per_gas, max_prio = %gas_info.max_priority_fee_per_gas, "Constructed Tx Request");
    let typed_tx: TypedTransaction = tx_request.clone().into();
    let signature = client.signer().sign_transaction(&typed_tx).await.wrap_err("ALERT: Signing failed pre-submission")?;
    let rlp_signed = typed_tx.rlp_signed(&signature);
    let rlp_hex = format!("0x{}", hex::encode(rlp_signed.as_ref()));

    // --- Step 9 (Attempt Submissions Sequentially) ---
    let pending_tx_result = submit_sequentially( &config, client.provider(), client.clone(), &rlp_hex, &rlp_signed ).await;
    let pending_tx = match pending_tx_result {
        Ok(ptx) => {
            let current_tx_hash = ptx.tx_hash();
            tx_hash = Some(current_tx_hash);
            tracing::Span::current().record("tx_hash", tracing::field::debug(current_tx_hash));
            ptx
        },
        Err(submission_error) => {
            error!(error = ?submission_error, route = ?route, "ALERT: All transaction submission attempts failed.");
            if submission_error.to_string().to_lowercase().contains("nonce") { nonce_manager.handle_nonce_error().await; }
            return Err(submission_error);
        }
    };

    // --- Step 10 (Monitor Submitted Transaction) ---
    let submitted_tx_hash = tx_hash.unwrap();
    info!(%submitted_tx_hash, "Submitted, awaiting confirmation (timeout: {}s)...", TX_CONFIRMATION_TIMEOUT_SECS);
    match timeout(Duration::from_secs(TX_CONFIRMATION_TIMEOUT_SECS), pending_tx).await {
        Err(_) => {
            warn!(%submitted_tx_hash, timeout_secs = TX_CONFIRMATION_TIMEOUT_SECS, route = ?route, "ALERT: Timeout waiting for transaction confirmation.");
            Err(eyre!("Timeout confirmation for {}", submitted_tx_hash))
        }
        Ok(Ok(Some(receipt))) => {
            let gas_used = receipt.gas_used.unwrap_or_default();
            let effective_gas_price = receipt.effective_gas_price.unwrap_or_default();
            let gas_cost_eth = format_units(gas_used * effective_gas_price, "ether").unwrap_or_default();

            if receipt.status == Some(TX_SUCCESS_STATUS) {
                 info!(tx_hash = %receipt.transaction_hash, block = %receipt.block_number.unwrap_or_default(), gas_used = %gas_used, gas_cost_eth = %gas_cost_eth, route = ?route, "ALERT: ✅✅✅ Tx Confirmed & Succeeded!");
                 nonce_manager.confirm_nonce_used(nonce).await;
                 Ok(submitted_tx_hash)
            } else {
                 error!(tx_hash = %submitted_tx_hash, status = ?receipt.status, block = %receipt.block_number.unwrap_or_default(), gas_used = %gas_used, gas_cost_eth = %gas_cost_eth, route = ?route, "ALERT: ❌ Tx Confirmed but REVERTED on-chain!");
                 nonce_manager.confirm_nonce_used(nonce).await;
                 Err(eyre!("Transaction reverted: {}", submitted_tx_hash))
            }
        }
        Ok(Ok(None)) => {
             warn!(%submitted_tx_hash, route = ?route, "ALERT: Transaction dropped/replaced (receipt not found).");
             nonce_manager.handle_nonce_error().await;
             Err(eyre!("Transaction dropped/replaced: {}", submitted_tx_hash))
        }
        Ok(Err(e)) => {
             error!(%submitted_tx_hash, error = ?e, route = ?route, "ALERT: Error monitoring transaction");
             if e.to_string().contains("nonce") { nonce_manager.handle_nonce_error().await; }
             Err(eyre!(e).wrap_err(format!("Error monitoring tx {}", submitted_tx_hash)))
        }
    }
}


// --- Helper: send_alchemy_private_tx ---
async fn send_alchemy_private_tx( provider: &Provider<Http>, rlp_hex: &str ) -> Result<TxHash> {
    let method="alchemy_sendPrivateTransaction";
    let params=AlchemyPrivateTxParams{tx:rlp_hex};
    provider.inner().request(method,[serde_json::to_value(params)?]).await.map_err(|e|eyre!(e.to_string()))
}
// --- Helper: send_flashbots_private_tx ---
async fn send_flashbots_private_tx( provider: &Provider<Http>, rlp_hex: &str ) -> Result<TxHash> {
    let method="eth_sendPrivateRawTransaction";
    let params=[rlp_hex];
    provider.inner().request(method,params).await.map_err(|e|eyre!(e.to_string()))
}
// --- Helper: submit_sequentially ---
async fn submit_sequentially( config: &Config, _provider: &Provider<Http>, client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, rlp_hex: &str, rlp_signed: &Bytes ) -> Result<PendingTransaction<'static, Http>> {
    let mut last_error: Option<eyre::Report> = None;

    // 1. Try Primary Private Relay
    if let Some(url) = &config.private_rpc_url {
        info!("Attempting submission via Primary Private Relay: {}", url);
        match Provider::<Http>::try_from(url.clone()) {
            Ok(relay_provider) => {
                let result = if url.contains("alchemy") {
                    send_alchemy_private_tx(&relay_provider, rlp_hex).await
                } else {
                    send_flashbots_private_tx(&relay_provider, rlp_hex).await
                };
                match result {
                    Ok(tx_hash) => {
                        info!(%tx_hash, relay = url, "Submitted via Primary Private Relay.");
                        return Ok(PendingTransaction::new(tx_hash, client.provider()));
                    }
                    Err(e) => {
                        warn!(error = ?e, relay = url, "Primary Private Relay submission failed.");
                        last_error = Some(e.wrap_err("Primary Private Relay failed"));
                    }
                }
            }
            Err(e) => {
                 warn!(error = ?e, url = url, "Failed to create provider for primary relay.");
                 last_error = Some(eyre!(e).wrap_err("Failed to create provider for primary relay"));
            }
        }
    }

    // 2. Try Secondary Private Relay
    if let Some(url) = &config.secondary_private_rpc_url {
        info!("Attempting submission via Secondary Private Relay: {}", url);
         match Provider::<Http>::try_from(url.clone()) {
             Ok(relay_provider) => {
                 let result = if url.contains("alchemy") {
                    send_alchemy_private_tx(&relay_provider, rlp_hex).await
                } else {
                    send_flashbots_private_tx(&relay_provider, rlp_hex).await
                };
                match result {
                    Ok(tx_hash) => {
                        info!(%tx_hash, relay = url, "Submitted via Secondary Private Relay.");
                        return Ok(PendingTransaction::new(tx_hash, client.provider()));
                    }
                    Err(e) => {
                        warn!(error = ?e, relay = url, "Secondary Private Relay submission failed.");
                        last_error = Some(e.wrap_err("Secondary Private Relay failed"));
                    }
                }
             }
             Err(e) => {
                  warn!(error = ?e, url = url, "Failed to create provider for secondary relay.");
                  last_error = Some(eyre!(e).wrap_err("Failed to create provider for secondary relay"));
             }
         }
    }

    // 3. Fallback to Public RPC
    info!("Attempting submission via Public RPC...");
    match client.send_raw_transaction(rlp_signed.clone()).await {
        Ok(pending_tx) => {
            info!(tx_hash = ?pending_tx.tx_hash(), "Submitted via Public RPC.");
            return Ok(pending_tx);
        }
        Err(e) => {
            error!(error = ?e, "Public RPC submission failed.");
             // FIX E0782: Simplify the match, use e.to_string()
             let middleware_error_context = format!(" ({})", e.to_string());
            last_error = Some(eyre!(e).wrap_err(format!("Public RPC submission failed{}", middleware_error_context)));
        }
    }

    Err(last_error.unwrap_or_else(|| eyre!("No submission methods configured or succeeded")))
}