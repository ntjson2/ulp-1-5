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
        U64, I256, TxHash, TransactionReceipt,
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

// Constants for transaction monitoring
const TX_CONFIRMATION_TIMEOUT_SECS: u64 = 60;
const TX_SUCCESS_STATUS: U64 = U64([1]);
// Constants for minProfit calculation buffer
// Use f64 for percentage calculation before converting to I256
const MIN_PROFIT_BUFFER_BPS: u64 = 10; // Example: 10 BPS = 0.1% buffer relative to profit
const MIN_PROFIT_ABS_BUFFER_WEI: u128 = 5_000_000_000_000; // Example: Require ~0.000005 ETH absolute min profit buffer

// --- Structs (GasInfo, NonceManager - Unchanged) ---
#[derive(Debug, Clone, Copy)] pub struct GasInfo { pub max_fee_per_gas: U256, pub max_priority_fee_per_gas: U256 }
#[derive(Debug)] pub struct NonceManager { current_nonce: Mutex<Option<U256>>, wallet_address: Address }

// --- NonceManager Impl (Unchanged) ---
impl NonceManager { /* ... methods unchanged ... */
    pub fn new(wallet_address: Address) -> Self { Self { current_nonce: Mutex::new(None), wallet_address } }
    pub async fn get_next_nonce<M: Middleware>(&self, client: Arc<M>) -> Result<U256> where M::Error: 'static+Send+Sync { /* ... */ let mut g=self.current_nonce.lock().await; let n=match*g{Some(c)=>c+1.into(),None=>client.get_transaction_count(self.wallet_address,Some(BlockNumber::Pending.into())).await?};*g=Some(n);Ok(n) }
    pub async fn handle_nonce_error(&self) { /* ... */ let mut n=self.current_nonce.lock().await; *n=None; }
    pub async fn confirm_nonce_used(&self, used_nonce: U256) { /* ... */ let mut g=self.current_nonce.lock().await; if*g==Some(used_nonce){*g=Some(used_nonce+1.into());} }
}

// --- fetch_gas_price function (Unchanged) ---
#[instrument(skip(client, config), level = "debug")]
pub async fn fetch_gas_price(client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, config: &Config) -> Result<GasInfo> { /* ... implementation unchanged ... */ }

// --- Alchemy Struct (Unchanged) ---
#[derive(Serialize, Debug)] #[serde(rename_all = "camelCase")] struct AlchemyPrivateTxParams<'a> { tx: &'a str }

/// Constructs, submits, and monitors the arbitrage transaction. Includes ALERT logs.
#[instrument(skip(client, app_state, route, nonce_manager), level = "info", fields(tx_hash = tracing::field::Empty))]
pub async fn submit_arbitrage_transaction(
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    app_state: Arc<AppState>,
    route: RouteCandidate,
    loan_amount_wei: U256,
    simulated_net_profit_wei: I256, // Use this as basis for minProfitWei
    nonce_manager: Arc<NonceManager>,
) -> Result<TxHash> {
    info!(buy_pool=%route.buy_pool_addr, sell_pool=%route.sell_pool_addr, loan_eth = %format_units(loan_amount_wei, "ether")?, sim_profit_wei = %simulated_net_profit_wei, "Attempting submission & monitoring");
    let config = &app_state.config; let tx_hash: Option<TxHash> = None;

    // 1. Fetch Gas Price
    let gas_info = fetch_gas_price(client.clone(), config).await?;

    // 2. Calculate minProfitWei threshold ** WITH REFINED BUFFER **
    let min_profit_wei_u256 = if simulated_net_profit_wei > I256::zero() {
        // Calculate percentage buffer (e.g., 10 BPS = 0.1%)
        // Use BPS for integer math precision: profit * BPS / 10000
        let bps_buffer = simulated_net_profit_wei.unsigned_abs() * U256::from(MIN_PROFIT_BUFFER_BPS) / U256::from(10000);
        let bps_buffer_i256 = I256::try_from(bps_buffer).unwrap_or_else(|_| I256::max_value()); // Convert back safely

        // Absolute buffer
        let abs_buffer_i256 = I256::from(MIN_PROFIT_ABS_BUFFER_WEI);

        // Take the LARGER of the two buffers to ensure minimum safety margin
        let effective_buffer = std::cmp::max(bps_buffer_i256, abs_buffer_i256);

        // Ensure buffer doesn't exceed the profit itself (leave at least 1 wei)
        let final_buffer = std::cmp::min(effective_buffer, simulated_net_profit_wei - I256::one());

        let min_profit_wei = simulated_net_profit_wei - final_buffer;
        debug!(sim_profit = %simulated_net_profit_wei, buffer = %final_buffer, min_profit = %min_profit_wei, "Calculated minProfitWei with buffer");

        // Ensure result is non-negative before converting to U256 for encoding
        if min_profit_wei > I256::zero() {
            min_profit_wei.into_raw()
        } else {
            U256::one() // Default to requiring at least 1 wei profit if buffer makes it zero/negative
        }
    } else {
        // If simulated profit is not positive, we shouldn't be here, but defensively require 1 wei.
        warn!(%simulated_net_profit_wei, "Simulated profit was not positive, setting minProfitWei to 1");
        U256::one()
    };
    debug!(min_profit_encoded = %min_profit_wei_u256);


    // --- Steps 3-8 (Salt, UserData, GasEst, Nonce, TxConstruct, Sign - use calculated min_profit_wei_u256) ---
    let salt = U256::from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos());
    let effective_router_addr = if route.buy_dex_type.is_velo_style() || route.sell_dex_type.is_velo_style() { if route.buy_dex_type == DexType::Aerodrome { config.aerodrome_router_addr.ok_or_else(|| eyre!("Aero router needed"))? } else { config.velo_router_addr } } else { config.velo_router_addr };
    let user_data = encode_user_data( route.buy_pool_addr, route.sell_pool_addr, app_state.usdc_address, route.zero_for_one_a, route.buy_dex_type.is_velo_style(), route.sell_dex_type.is_velo_style(), effective_router_addr, min_profit_wei_u256, salt )?; // Pass calculated min profit
    let estimated_gas_limit = estimate_flash_loan_gas( client.clone(), config.balancer_vault_address, config.arb_executor_address.unwrap(), app_state.weth_address, loan_amount_wei, user_data.clone(), ).await?;
    let final_gas_limit = std::cmp::max(estimated_gas_limit * (100 + config.gas_limit_buffer_percentage) / 100, U256::from(config.min_flashloan_gas_limit));
    let nonce = nonce_manager.get_next_nonce(client.clone()).await?;
    let balancer_contract = BalancerVault::new(config.balancer_vault_address, client.clone());
    let calldata = balancer_contract.flash_loan( config.arb_executor_address.unwrap(), vec![app_state.weth_address], vec![loan_amount_wei], user_data, ).calldata().ok_or_else(|| eyre!("Failed get calldata"))?;
    let tx_request = Eip1559TransactionRequest::new().to(config.balancer_vault_address).value(U256::zero()).data(calldata).gas(final_gas_limit).max_fee_per_gas(gas_info.max_fee_per_gas).max_priority_fee_per_gas(gas_info.max_priority_fee_per_gas).nonce(nonce).chain_id(client.signer().chain_id());
    info!(%nonce, %final_gas_limit, "Constructed Tx Request");
    let typed_tx: TypedTransaction = tx_request.clone().into();
    let signature = client.signer().sign_transaction(&typed_tx).await?;
    let rlp_signed = typed_tx.rlp_signed(&signature);
    let rlp_hex = format!("0x{}", hex::encode(rlp_signed.as_ref()));

    // --- Step 9 (Attempt Submissions - unchanged) ---
    let pending_tx_result = submit_sequentially( &config, client.provider(), client.clone(), &rlp_hex, &rlp_signed ).await;
    let pending_tx = match pending_tx_result {
        Ok(ptx) => { tracing::Span::current().record("tx_hash", tracing::field::debug(ptx.tx_hash())); ptx },
        Err(e) => { error!(error=?e, "ALERT: All submissions failed."); /* TODO: ALERT */ if e.to_string().contains("nonce"){nonce_manager.handle_nonce_error().await;} return Err(e); }
    };

    // --- Step 10 (Monitor Submitted Tx - unchanged alert logic) ---
    let submitted_tx_hash = pending_tx.tx_hash();
    info!(%submitted_tx_hash, "Submitted, awaiting confirmation...");
    match timeout(Duration::from_secs(TX_CONFIRMATION_TIMEOUT_SECS), pending_tx).await {
        Err(_) => { warn!(%submitted_tx_hash, "ALERT: Timeout confirm."); /* TODO: ALERT */ Err(eyre!("Timeout")) }
        Ok(Ok(Some(receipt))) => {
            if receipt.status == Some(TX_SUCCESS_STATUS) { info!(tx_hash=%receipt.transaction_hash, block=%receipt.block_number.unwrap_or_default(), "✅✅✅ Tx SUCCESS!"); /* TODO: ALERT */ nonce_manager.confirm_nonce_used(nonce).await; Ok(submitted_tx_hash) }
            else { error!(%submitted_tx_hash, status=?receipt.status, "ALERT: ❌ Tx REVERTED!"); /* TODO: ALERT */ nonce_manager.confirm_nonce_used(nonce).await; Err(eyre!("Tx Reverted")) }
        }
        Ok(Ok(None)) => { warn!(%submitted_tx_hash, "ALERT: Tx Dropped?"); /* TODO: ALERT */ nonce_manager.handle_nonce_error().await; Err(eyre!("Tx Dropped")) }
        Ok(Err(e)) => { error!(%submitted_tx_hash, error=?e, "ALERT: Error monitoring tx"); /* TODO: ALERT */ if e.to_string().contains("nonce") { nonce_manager.handle_nonce_error().await; } Err(eyre!(e).wrap_err("Tx monitor error")) }
    }
}


/// Helper: Sends tx via Alchemy. (Unchanged)
async fn send_alchemy_private_tx<C: EthersJsonRpcClient>( provider: &C, rlp_hex: &str ) -> Result<TxHash> { /* ... */ let m="alchemy_sendPrivateTransaction"; let p=AlchemyPrivateTxParams{tx:rlp_hex}; provider.request(m,[serde_json::to_value(p)?]).await.map_err(|e|eyre!(e)) }
/// Helper: Sends tx via Flashbots. (Unchanged)
async fn send_flashbots_private_tx<C: EthersJsonRpcClient>( provider: &C, rlp_hex: &str ) -> Result<TxHash> { /* ... */ let m="eth_sendPrivateRawTransaction"; let p=[rlp_hex]; provider.request(m,p).await.map_err(|e|eyre!(e)) }
/// Helper: Tries submitting via private relays then public. (Unchanged)
async fn submit_sequentially( config: &Config, provider: &Provider<Http>, client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>, rlp_hex: &str, rlp_signed: &Bytes ) -> Result<PendingTransaction<'static, Http>> { /* ... */
    let mut last_error: Option<eyre::Report> = None;
    if let Some(url)=&config.private_rpc_url{let res=if url.contains("flash"){send_flashbots_private_tx(provider,rlp_hex).await}else if url.contains("alchemy"){send_alchemy_private_tx(provider,rlp_hex).await}else{Err(eyre!("Unrec primary"))}; if let Ok(h)=res{return Ok(client.get_transaction(h).await?.into());}else{last_error=res.err(); warn!(relay="primary",error=?last_error,"FAIL");}}
    if let Some(url)=&config.secondary_private_rpc_url{let res=if url.contains("flash"){send_flashbots_private_tx(provider,rlp_hex).await}else if url.contains("alchemy"){send_alchemy_private_tx(provider,rlp_hex).await}else{Err(eyre!("Unrec secondary"))}; if let Ok(h)=res{return Ok(client.get_transaction(h).await?.into());}else{if last_error.is_none(){last_error=res.err();} warn!(relay="secondary",error=?last_error,"FAIL");}}
    info!("Attempting PUBLIC..."); match client.send_raw_transaction(rlp_signed.clone()).await { Ok(ptx)=>{info!(tx_hash=%ptx.tx_hash(),"Public OK");Ok(ptx)}, Err(e)=>{error!(error=?e,"Public FAIL"); Err(last_error.unwrap_or_else(|| eyre!(e).wrap_err("Public submit failed")))} }
}

// Helper trait method impl (Unchanged)
impl DexType { fn is_velo_style(&self) -> bool { matches!(self, DexType::VelodromeV2 | DexType::Aerodrome) } }

// END OF FILE: bot/src/transaction.rs