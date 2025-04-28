// bot/src/transaction.rs

use crate::config::Config;
use crate::encoding::encode_user_data;
use crate::state::{AppState, DexType};
use crate::path_optimizer::RouteCandidate;
use ethers::{
    prelude::*,
    providers::{JsonRpcClient as EthersJsonRpcClient, MiddlewareError, ProviderError, StreamExt},
    types::{
        transaction::eip2718::TypedTransaction, Address, Bytes, Eip1559TransactionRequest, U256,
        U64, I256, TxHash, TransactionReceipt, // Keep TransactionReceipt
    },
    utils::{format_units, parse_units},
};
use eyre::{eyre, Result, WrapErr};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use tokio::time::timeout;
use tracing::{debug, error, info, instrument, warn, trace};

// --- Constants ---
const TX_CONFIRMATION_TIMEOUT_SECS: u64 = 60;
const TX_SUCCESS_STATUS: U64 = U64([1]);
const MIN_PROFIT_BUFFER_BPS: u64 = 10;
const MIN_PROFIT_ABS_BUFFER_WEI: u128 = 5_000_000_000_000;

// --- Structs (GasInfo, NonceManager, AlchemyPrivateTxParams - Unchanged) ---
#[derive(Debug, Clone, Copy)] pub struct GasInfo { pub max_fee_per_gas: U256, pub max_priority_fee_per_gas: U256 }
#[derive(Debug)] pub struct NonceManager { current_nonce: Mutex<Option<U256>>, wallet_address: Address }
#[derive(Serialize, Debug)] #[serde(rename_all = "camelCase")] struct AlchemyPrivateTxParams<'a> { tx: &'a str }

// --- NonceManager Impl (Unchanged) ---
impl NonceManager { /* ... methods unchanged ... */
    pub fn new(wallet_address: Address) -> Self { Self { current_nonce: Mutex::new(None), wallet_address } }
    pub async fn get_next_nonce<M: Middleware>(&self, client: Arc<M>) -> Result<U256> where M::Error: 'static+Send+Sync { let mut g=self.current_nonce.lock().await; let n=match*g{Some(c)=>c+1.into(),None=>client.get_transaction_count(self.wallet_address,Some(BlockNumber::Pending.into())).await?};*g=Some(n);Ok(n) }
    pub async fn handle_nonce_error(&self) { let mut n=self.current_nonce.lock().await;*n=None; }
    pub async fn confirm_nonce_used(&self, used_nonce: U256) { let mut g=self.current_nonce.lock().await;if*g==Some(used_nonce){*g=Some(used_nonce+1.into());}}
}

// --- fetch_gas_price function (Unchanged) ---
#[instrument(skip(client, config), level = "debug")]
pub async fn fetch_gas_price(client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, config: &Config) -> Result<GasInfo> { /* ... implementation unchanged ... */ }


/// Constructs, submits, and monitors the arbitrage transaction. Includes enhanced ALERT logs.
#[instrument(skip_all, level = "info", fields(
    buy_pool = %route.buy_pool_addr,
    sell_pool = %route.sell_pool_addr,
    loan_eth = %format_units(loan_amount_wei, "ether").unwrap_or_default(),
    sim_profit_wei = %simulated_net_profit_wei,
    tx_hash = tracing::field::Empty // Initialize empty tx_hash field in span
))]
pub async fn submit_arbitrage_transaction(
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    app_state: Arc<AppState>,
    route: RouteCandidate, // Take ownership for logging purposes if needed, or clone route details
    loan_amount_wei: U256,
    simulated_net_profit_wei: I256,
    nonce_manager: Arc<NonceManager>,
) -> Result<TxHash> {
    // Log entry with route details already covered by instrument span
    info!("Attempting submission & monitoring");
    let config = &app_state.config;
    let mut tx_hash: Option<TxHash> = None;

    // --- Steps 1-8 (Prepare Tx Data & Sign) ---
    // Add context to errors where external alerts might be useful
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
    info!(%nonce, %final_gas_limit, "Constructed Tx Request");
    let typed_tx: TypedTransaction = tx_request.clone().into();
    let signature = client.signer().sign_transaction(&typed_tx).await.wrap_err("ALERT: Signing failed pre-submission")?;
    let rlp_signed = typed_tx.rlp_signed(&signature);
    let rlp_hex = format!("0x{}", hex::encode(rlp_signed.as_ref()));

    // --- Step 9 (Attempt Submissions Sequentially) ---
    let pending_tx_result = submit_sequentially( &config, client.provider(), client.clone(), &rlp_hex, &rlp_signed ).await;
    let pending_tx = match pending_tx_result {
        Ok(ptx) => {
            let current_tx_hash = ptx.tx_hash();
            tx_hash = Some(current_tx_hash); // Store hash
            // Record hash in span *after* successful submission
            tracing::Span::current().record("tx_hash", tracing::field::debug(current_tx_hash));
            ptx
        },
        Err(submission_error) => {
            // ALERT: Critical failure - cannot submit transaction
            error!(error = ?submission_error, route = ?route, "ALERT: All transaction submission attempts failed.");
            // TODO: ALERT: Trigger P1 external alert for consistent submission failures.
            if submission_error.to_string().to_lowercase().contains("nonce") { nonce_manager.handle_nonce_error().await; }
            return Err(submission_error);
        }
    };

    // --- Step 10 (Monitor Submitted Transaction) ---
    let submitted_tx_hash = tx_hash.unwrap(); // Safe to unwrap as we error'd earlier if None
    info!(%submitted_tx_hash, "Submitted, awaiting confirmation (timeout: {}s)...", TX_CONFIRMATION_TIMEOUT_SECS);
    match timeout(Duration::from_secs(TX_CONFIRMATION_TIMEOUT_SECS), pending_tx).await {
        Err(_) => {
            warn!(%submitted_tx_hash, timeout_secs = TX_CONFIRMATION_TIMEOUT_SECS, route = ?route, "ALERT: Timeout waiting for transaction confirmation.");
            // TODO: ALERT: Trigger external alert for transaction timeout. Include route details.
            Err(eyre!("Timeout confirmation for {}", submitted_tx_hash))
        }
        Ok(Ok(Some(receipt))) => { // Confirmed
            let gas_used = receipt.gas_used.unwrap_or_default();
            let effective_gas_price = receipt.effective_gas_price.unwrap_or_default();
            let gas_cost_eth = format_units(gas_used * effective_gas_price, "ether").unwrap_or_default();

            if receipt.status == Some(TX_SUCCESS_STATUS) {
                 // SUCCESS NOTIFICATION
                 info!(tx_hash = %receipt.transaction_hash, block = %receipt.block_number.unwrap_or_default(), gas_used = %gas_used, gas_cost_eth = %gas_cost_eth, route = ?route, "ALERT: ✅✅✅ Tx Confirmed & Succeeded!");
                 // TODO: ALERT: Trigger external notification (e.g., Slack/Telegram) for SUCCESSFUL trade. Include route, profit, gas cost.
                 nonce_manager.confirm_nonce_used(nonce).await;
                 Ok(submitted_tx_hash)
            } else {
                 // ALERT: Transaction reverted
                 error!(tx_hash = %submitted_tx_hash, status = ?receipt.status, block = %receipt.block_number.unwrap_or_default(), gas_used = %gas_used, gas_cost_eth = %gas_cost_eth, route = ?route, "ALERT: ❌ Tx Confirmed but REVERTED on-chain!");
                 // TODO: ALERT: Trigger external alert (e.g., PagerDuty/OpsGenie) for reverted trade. Include route details, gas cost.
                 nonce_manager.confirm_nonce_used(nonce).await; // Nonce is consumed
                 Err(eyre!("Transaction reverted: {}", submitted_tx_hash))
            }
        }
        Ok(Ok(None)) => { // Dropped / Replaced
             warn!(%submitted_tx_hash, route = ?route, "ALERT: Transaction dropped/replaced (receipt not found).");
             // TODO: ALERT: Trigger external alert for dropped transaction. Include route details.
             nonce_manager.handle_nonce_error().await; // Nonce likely reusable
             Err(eyre!("Transaction dropped/replaced: {}", submitted_tx_hash))
        }
        Ok(Err(e)) => { // Monitoring Error
             error!(%submitted_tx_hash, error = ?e, route = ?route, "ALERT: Error monitoring transaction");
             // TODO: ALERT: Trigger external alert for monitoring errors. Include route details.
             if e.to_string().contains("nonce") { nonce_manager.handle_nonce_error().await; }
             Err(eyre!(e).wrap_err(format!("Error monitoring tx {}", submitted_tx_hash)))
        }
    }
}


// --- Helper: send_alchemy_private_tx (Unchanged) ---
async fn send_alchemy_private_tx<C: EthersJsonRpcClient>( provider: &C, rlp_hex: &str ) -> Result<TxHash> { /* ... */ let m="alchemy_sendPrivateTransaction"; let p=AlchemyPrivateTxParams{tx:rlp_hex}; provider.request(m,[serde_json::to_value(p)?]).await.map_err(|e|eyre!(e)) }
// --- Helper: send_flashbots_private_tx (Unchanged) ---
async fn send_flashbots_private_tx<C: EthersJsonRpcClient>( provider: &C, rlp_hex: &str ) -> Result<TxHash> { /* ... */ let m="eth_sendPrivateRawTransaction"; let p=[rlp_hex]; provider.request(m,p).await.map_err(|e|eyre!(e)) }
// --- Helper: submit_sequentially (Unchanged) ---
async fn submit_sequentially( config: &Config, provider: &Provider<Http>, client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, rlp_hex: &str, rlp_signed: &Bytes ) -> Result<PendingTransaction<'static, Http>> { /* ... implementation unchanged ... */ }
// --- Helper trait method impl (Unchanged) ---
impl DexType { fn is_velo_style(&self) -> bool { matches!(self, DexType::VelodromeV2 | DexType::Aerodrome) } }


// END OF FILE: bot/src/transaction.rs