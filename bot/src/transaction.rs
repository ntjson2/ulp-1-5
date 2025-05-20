// bot/src/transaction.rs

use ethers::{
    core::types::{
        transaction::eip2718::TypedTransaction, Address, TransactionReceipt,
        TransactionRequest, U256, I256, H256, Bytes,
        U64 as EthersU64, 
    },
    middleware::{SignerMiddleware, MiddlewareBuilder, NonceManagerMiddleware, SignerMiddlewareError}, // Added SignerMiddlewareError
    providers::{Http, Middleware, Provider, PendingTransaction, ProviderError as EthersProviderError}, 
    signers::{LocalWallet, Signer}, 
    utils::{rlp, parse_units, format_units}, // Added parse_units, format_units
};
use eyre::{eyre, Result, Report};
use serde::Serialize; // Added Serialize
use std::error::Error as StdErrorTrait;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex; // Added Mutex
use tracing::{info, warn, error, instrument}; // Added error

use crate::{
    config::Config,
    state::{AppState, DexType}, // Using AppState, RouteDetails assumed to be RouteCandidate
    path_optimizer::RouteCandidate, // Using RouteCandidate
    bindings, 
    utils::{calculate_salt, get_provider_from_url},
};


const TX_SUCCESS_STATUS: EthersU64 = EthersU64::new(1u64);
const GAS_LIMIT_DEFAULT: u64 = 1_500_000; 

#[derive(Serialize, Debug)] // Added Serialize
pub struct TransactionDetails {
    // ... fields ...
}

// Using ethers' NonceManager
// If a custom one is truly needed, its definition must be complete and correct.
// For now, assuming ethers' NonceManager is intended or sufficient.
// pub type NonceManager = ethers::middleware::nonce_manager::NonceManager<Arc<SignerMiddleware<Provider<Http>, LocalWallet>>>;
// The AppState uses `crate::transaction::NonceManager`, so a local definition is expected.

#[derive(Debug)]
pub struct NonceManager {
    current_nonce: Mutex<Option<U256>>, // Use tokio::sync::Mutex
    wallet_address: Address,
}

impl NonceManager {
    pub fn new(wallet_address: Address) -> Self {
        Self {
            current_nonce: Mutex::new(None),
            wallet_address,
        }
    }

    pub async fn next<M: Middleware>(&self, client_or_provider: &M) -> Result<U256>
    where
        <M as Middleware>::Error: StdErrorTrait + Send + Sync + 'static, // Use StdErrorTrait
    {
        let mut nonce_lock = self.current_nonce.lock().await;
        let current_val = match *nonce_lock {
            Some(n) => n + U256::one(),
            None => {
                client_or_provider
                    .get_transaction_count(self.wallet_address, None)
                    .await
                    .map_err(|e| eyre!("Failed to get initial nonce: {:?}", e))?
            }
        };
        *nonce_lock = Some(current_val);
        Ok(current_val)
    }
}

#[derive(Debug)] // Added Debug for NonceGuard
pub struct NonceGuard {
    manager: Arc<NonceManager>,
    wallet: Address,
    nonce: U256,
    released: bool,
}

impl NonceGuard {
    pub fn new(manager: Arc<NonceManager>, wallet: Address, nonce: U256) -> Self {
        Self { manager, wallet, nonce, released: false }
    }

    #[allow(dead_code)]
    pub async fn release(&mut self) {
        if !self.released {
            let mut current_nonce_lock = self.manager.current_nonce.lock().await;
            if *current_nonce_lock == Some(self.nonce) {
                 if self.nonce > U256::zero() { // Prevent underflow
                    *current_nonce_lock = Some(self.nonce - U256::one());
                 } else {
                    *current_nonce_lock = None; // Or reset to None if it was the first nonce
                 }
            }
            self.released = true;
            debug!("NonceGuard released nonce {}", self.nonce);
        }
    }
}

impl Drop for NonceGuard {
    fn drop(&mut self) {
        if !self.released {
            // If the guard is dropped and release() was not called, it means the nonce was likely consumed
            // or the transaction sending part is responsible for handling its state.
            // In a more robust scenario, if the transaction failed to even send,
            // this drop might try to release/reset the nonce.
            // For now, relying on explicit release() call for failures before submission.
            // If using tokio::task::spawn, ensure NonceGuard is handled correctly across await points or not sent.
        }
    }
}


// TODO: Decimal type was removed from Cargo.toml. This function needs to be refactored
// or rust_decimal re-added. For now, returning zero/default values.
pub fn calculate_profit_thresholds(config: &Config, weth_price_usd: f64) -> (U256, U256) {
    warn!("Decimal type removed, calculate_profit_thresholds returning default values. Refactor needed.");
    let min_flat_profit_weth_val = config.min_flat_profit_weth_threshold.unwrap_or(0.0001);
    let weth_decimals_val = config.weth_decimals.unwrap_or(18);

    // Perform calculations using f64 and convert to U256 at the end
    let min_flat_profit_wei_f64 = min_flat_profit_weth_val * 10f64.powi(weth_decimals_val as i32);
    let min_flat_profit_wei = U256::from_dec_str(&format!("{:.0}", min_flat_profit_wei_f64)).unwrap_or_default();
    
    // Placeholder for min_profit_usd, as its calculation also involved Decimal
    let min_profit_usd_wei = U256::zero(); // Placeholder

    // Original logic for min_profit_wei (based on percentage) might still be valid if it doesn't use Decimal
    let min_profit_threshold_wei_val = config.min_profit_threshold_wei.unwrap_or_default();

    (min_profit_threshold_wei_val.max(min_flat_profit_wei), min_profit_usd_wei)
}

/// Constructs, submits, and monitors the arbitrage transaction using polling.
#[instrument(
    skip_all,
    level = "info",
    fields(
        route_id = %route.id(),
        loan_amount_eth = %format_units(loan_amount_wei, app_state.config.weth_decimals.unwrap_or(18) as i32).unwrap_or_else(|_| String::from("parse_err")),
        profit_eth = %format_units(min_profit_wei, app_state.config.weth_decimals.unwrap_or(18) as i32).unwrap_or_else(|_| String::from("parse_err"))
    )
)]
pub async fn submit_arbitrage_transaction(
    config: Arc<Config>,
    route: &RouteCandidate, // Using RouteCandidate
    loan_amount_wei: U256,
    min_profit_wei: I256,
    wallet: LocalWallet,
    http_provider: Provider<Http>,
    app_state: Arc<AppState>, // Added AppState
) -> Result<Option<TransactionReceipt>> {
    let config = &app_state.config;
    let http_provider = Provider::<Http>::try_from(config.http_rpc_url.clone())
        .wrap_err("Failed to create HTTP provider for submission")?;
    
    let wallet = app_state.client.signer().clone(); 
    let wallet_address = wallet.address();
    let chain_id_u64 = wallet.chain_id(); // u64

    // Nonce manager setup for the submission client
    let client_for_submission_with_nonce_manager: NonceManagerMiddleware<SignerMiddleware<Provider<Http>, LocalWallet>> = SignerMiddleware::new(http_provider.clone(), wallet.clone())
        .nonce_manager(wallet_address); 
    
    let client_for_submission_arc = Arc::new(client_for_submission_with_nonce_manager);


    let executor_address = config.arb_executor_address.ok_or_else(|| eyre!("Arb executor address not configured"))?;
    let executor_contract = crate::bindings::arbitrage_executor::ArbitrageExecutor::new(executor_address, client_for_submission_arc.clone());
    
    let salt_val = U256::zero(); 

    // Ensure execute_flash_arbitrage_balancer and execute_flash_arbitrage methods are in ArbitrageExecutor.json
    // and that abigen generates them correctly.
    let call_data = if route.buy_dex_type == DexType::Balancer || route.sell_dex_type == DexType::Balancer {
        let balancer_vault_addr = config.balancer_vault_address.ok_or_else(|| eyre!("Balancer vault address not configured"))?;
        executor_contract.execute_flash_arbitrage_balancer( 
            route.buy_pool_addr,
            route.sell_pool_addr,
            route.token0, // loan token (WETH)
            route.token1, // intermediate token
            loan_amount_wei, // Use the passed loan_amount_wei
            balancer_vault_addr,
            route.buy_dex_type as u8, 
            route.sell_dex_type as u8, 
            salt_val
        ).calldata().ok_or_else(|| eyre!("Failed to get calldata for execute_flash_arbitrage_balancer"))?
    } else {
        executor_contract.execute_flash_arbitrage(
            route.buy_pool_addr,
            route.sell_pool_addr,
            route.token0, // loan token (WETH)
            route.token1, // intermediate token
            loan_amount_wei, // Use the passed loan_amount_wei
            route.buy_dex_type as u8, 
            route.sell_dex_type as u8, 
            salt_val
        ).calldata().ok_or_else(|| eyre!("Failed to get calldata for execute_flash_arbitrage"))?
    };

    let base_tx_request = if route.buy_dex_type == DexType::Balancer || route.sell_dex_type == DexType::Balancer {
        TransactionRequest::new().to(config.balancer_vault_address.ok_or_else(|| eyre!("Balancer vault address not configured"))?).data(call_data.clone())
    } else {
        TransactionRequest::new().to(executor_address).data(call_data.clone())
    };
    
    let mut tx_request_typed: TypedTransaction = base_tx_request.into();
    
    // Set gas limit
    let gas_limit = config.submission_gas_limit_default.unwrap_or(GAS_LIMIT_DEFAULT);
    match &mut tx_request_typed {
        TypedTransaction::Legacy(tx) => tx.gas = Some(gas_limit.into()),
        TypedTransaction::Eip1559(tx) => tx.gas = Some(gas_limit.into()),
        TypedTransaction::Eip2930(tx) => tx.tx.gas = Some(gas_limit.into()),
        _ => {} // Other types like EIP4844 might not use .gas directly
    }

    // Set gas price or EIP-1559 fees
    if let Some(fixed_gas_price_gwei) = config.submission_gas_price_gwei_fixed {
        let gas_price_wei = parse_units(fixed_gas_price_gwei, "gwei")?.into(); // Added ethers::utils::parse_units
        match &mut tx_request_typed {
            TypedTransaction::Legacy(tx) => tx.gas_price = Some(gas_price_wei),
            TypedTransaction::Eip2930(tx) => tx.tx.gas_price = Some(gas_price_wei),
            TypedTransaction::Eip1559(tx) => { // If fixed gas price is set, use it for EIP-1559 as well for simplicity
                tx.max_fee_per_gas = Some(gas_price_wei);
                tx.max_priority_fee_per_gas = Some(gas_price_wei); // Or a portion of it
            }
            _ => {}
        }
    } else { // EIP-1559 dynamic fees
        let (max_fee, max_priority_fee) = client_for_submission_arc.estimate_eip1559_fees(None).await
            .wrap_err("Failed to estimate EIP-1559 fees")?;
        match &mut tx_request_typed {
            TypedTransaction::Eip1559(tx) => {
                tx.max_fee_per_gas = Some(max_fee);
                tx.max_priority_fee_per_gas = Some(max_priority_fee);
            }
            TypedTransaction::Legacy(_) | TypedTransaction::Eip2930(_) => {
                // If not EIP1559, but no fixed_gas_price, we might need to fetch legacy gas price
                let gas_price = client_for_submission_arc.get_gas_price().await?;
                 match &mut tx_request_typed {
                    TypedTransaction::Legacy(tx) => tx.gas_price = Some(gas_price),
                    TypedTransaction::Eip2930(tx) => tx.tx.gas_price = Some(gas_price),
                    _ => {} // Should not happen here
                 }
            }
            _ => {}
        }
    }
    
    // Nonce will be handled by the NonceManager in client_for_submission_arc

    match timeout(
        Duration::from_secs(config.submission_timeout_duration_seconds.unwrap_or(60)), 
        submit_sequentially(config, client_for_submission_arc.clone(), tx_request_typed, wallet_address, chain_id_u64) // Pass client_for_submission_arc directly
    ).await {
        Ok(Ok(receipt_option)) => Ok(receipt_option),
        Ok(Err(e)) => Err(e.wrap_err("Transaction submission attempt failed")),
        Err(_) => Err(eyre!("Transaction submission timed out overall")),
    }
}


// submit_sequentially now takes the client with NonceManager
pub async fn submit_sequentially<P, S>( 
    config: &Config,
    client_with_nonce_mgr: Arc<NonceManagerMiddleware<SignerMiddleware<P, S>>>, 
    tx_request: TypedTransaction,
    _wallet_address: Address, 
    _chain_id: u64 
) -> Result<Option<TransactionReceipt>> 
    where 
        P: Middleware + 'static,
        S: Signer + 'static,
        <P as Middleware>::Error: StdErrorTrait + Send + Sync + 'static, 
        <S as Signer>::Error: StdErrorTrait + Send + Sync + 'static, // Added bound for Signer error
        P::Provider: Send + Sync,
        NonceManagerMiddleware<SignerMiddleware<P, S>>: Middleware<Provider = P::Provider, Error = NonceManagerError<SignerMiddleware<P,S>>>, // Correct Error type
        NonceManagerError<SignerMiddleware<P,S>>: StdErrorTrait + Send + Sync + 'static // Ensure NonceManagerError is StdError
{
    let mut final_tx_request = tx_request.clone();
    // Ensure chain_id is set if not already (NonceManager client should handle this, but double check)
    if final_tx_request.chain_id().is_none() {
        final_tx_request.set_chain_id(_chain_id);
    }


    // Attempt direct submission first
    debug!("Attempting direct transaction submission...");
    let pending_tx_fut = client_with_nonce_mgr.send_transaction(final_tx_request.clone(), None);
    
    match timeout(Duration::from_secs(config.submission_timeout_duration_seconds.unwrap_or(15)), pending_tx_fut).await {
        Ok(Ok(pending_tx)) => { // Successfully sent
            let tx_hash = pending_tx.tx_hash();
            debug!("Transaction submitted directly, hash: {:?}", tx_hash);

            match timeout(
                Duration::from_secs(config.submission_timeout_duration_seconds.unwrap_or(30)),
                client_with_nonce_mgr.get_transaction_receipt(tx_hash)
            ).await {
                Ok(Ok(Some(receipt))) => {
                    if receipt.status == Some(TX_SUCCESS_STATUS) {
                        info!(tx_hash = ?receipt.transaction_hash, block = ?receipt.block_number, "✅ Transaction confirmed successfully!");
                        return Ok(Some(receipt));
                    } else {
                        error!(tx_hash = ?receipt.transaction_hash, block = ?receipt.block_number, "❌ Transaction failed (status 0)!");
                        return Ok(Some(receipt)); // Return receipt for inspection
                    }
                }
                Ok(Ok(None)) => warn!(%tx_hash, "Direct submission receipt not found after timeout (still pending or dropped)."),
                Ok(Err(e)) => error!(%tx_hash, error = ?e, "Error waiting for direct submission receipt."),
                Err(_) => warn!(%tx_hash, "Timeout waiting for direct submission receipt."),
            }
        }
        Ok(Err(e)) => { // Error during send_transaction itself
            error!("Direct transaction submission failed: {:?}", e);
            // NonceManager should handle nonce increment failures, but if send_transaction itself fails before mempool, nonce might not be consumed.
            // This depends on NonceManager's internal logic.
        }
        Err(_) => { // Timeout during send_transaction
            warn!("Direct transaction submission timed out.");
        }
    }
    
    // Fallback to private relay if configured and direct submission didn't confirm quickly
    if let Some(relay_urls) = &config.transaction_relay_urls {
        if relay_urls.is_empty() {
            return Ok(None); // No relays to try
        }
        let rlp_encoded_tx = final_tx_request.rlp();
        for url_str in relay_urls {
            match Provider::<Http>::try_from(url_str.as_str()) {
                Ok(relay_provider) => {
                    info!("Attempting to send private transaction via relay: {}", url_str);
                    let send_fut = relay_provider.send_raw_transaction(rlp_encoded_tx.clone()); 
                    match timeout(Duration::from_secs(10), send_fut).await { 
                        Ok(Ok(pending_tx_from_relay)) => { // pending_tx_from_relay is PendingTransaction
                            let tx_hash: H256 = pending_tx_from_relay.tx_hash(); // Get H256 tx_hash
                            info!("Transaction sent to relay {}, tx_hash: {:?}", url_str, tx_hash);
                            // After sending to relay, wait for receipt using the main client
                            match timeout(
                                Duration::from_secs(config.submission_timeout_duration_seconds.unwrap_or(30)),
                                client_with_nonce_mgr.get_transaction_receipt(tx_hash)
                            ).await {
                                Ok(Ok(Some(receipt))) => {
                                    if receipt.status == Some(TX_SUCCESS_STATUS) {
                                        info!(tx_hash = ?receipt.transaction_hash, block = ?receipt.block_number, "✅ Relay transaction confirmed!");
                                        return Ok(Some(receipt));
                                    } else {
                                        error!(tx_hash = ?receipt.transaction_hash, block = ?receipt.block_number, "❌ Relay transaction failed!");
                                        // Don't return here, try next relay or fail
                                    }
                                }
                                Ok(Ok(None)) => warn!(%tx_hash, relay_url = %url_str, "Relay submission receipt not found."),
                                Ok(Err(e)) => error!(%tx_hash, relay_url = %url_str, error = ?e, "Error waiting for relay receipt."),
                                Err(_) => warn!(%tx_hash, relay_url = %url_str, "Timeout waiting for relay receipt."),
                            }
                        }
                        Ok(Err(e)) => error!("Failed to send transaction to relay {}: {:?}", url_str, e),
                        Err(_) => warn!("Timeout sending transaction to relay {}", url_str),
                    }
                }
                Err(e) => error!("Failed to create provider for relay URL {}: {:?}", url_str, e),
            }
        }
    }
    warn!("Transaction submission failed after all attempts or no relays configured.");
    Ok(None) 
}

// Define a helper error wrapper
// This ProviderErrorWrapper is for wrapping errors from the *inner* SignerMiddleware
// when we are not using NonceManagerMiddleware directly, or for custom error handling.
// For submit_sequentially, NonceManagerError is the direct error type.
#[derive(Error, Debug)] // Use thiserror::Error
pub enum ProviderErrorWrapper<MError, SError> 
where
    MError: StdErrorTrait + Send + Sync + 'static,
    SError: StdErrorTrait + Send + Sync + 'static,
{
    #[error("Middleware error: {0}")] // Use #[error] from thiserror
    Middleware(MError),
    #[error("Signer error: {0}")]
    Signer(SError),
    #[error("Ethers Provider error: {0}")]
    Provider(#[from] EthersProviderError), // Use #[from] for direct conversion
    #[error("Custom error: {0}")]
    Custom(String),
}

// This From impl is for SignerMiddlewareError -> ProviderErrorWrapper
impl<M, S> From<SignerMiddlewareError<M, S>> for ProviderErrorWrapper<<M as Middleware>::Error, <S as Signer>::Error>
where
    M: Middleware + 'static,
    S: Signer + 'static,
    <M as Middleware>::Error: StdErrorTrait + Send + Sync + 'static,
    <S as Signer>::Error: StdErrorTrait + Send + Sync + 'static,
{
    fn from(err: SignerMiddlewareError<M, S>) -> Self {
        match err {
            SignerMiddlewareError::MiddlewareError(e) => ProviderErrorWrapper::Middleware(e),
            SignerMiddlewareError::SignerError(e) => ProviderErrorWrapper::Signer(e),
            // SignerMiddlewareError itself doesn't have a direct ProviderError variant like that.
            // It has NonceMissing, GasPriceMissing etc. which are more specific.
            SignerMiddlewareError::NonceMissing => ProviderErrorWrapper::Custom("NonceMissing from SignerMiddlewareError".to_string()),
            SignerMiddlewareError::GasPriceMissing => ProviderErrorWrapper::Custom("GasPriceMissing from SignerMiddlewareError".to_string()),
            SignerMiddlewareError::GasMissing => ProviderErrorWrapper::Custom("GasMissing from SignerMiddlewareError".to_string()),
            // Other variants of SignerMiddlewareError might need specific handling or mapping to Custom/Provider
            // For example, if there was an internal EthersProviderError, it would be wrapped in MiddlewareError typically.
            e => ProviderErrorWrapper::Custom(format!("Unhandled SignerMiddlewareError: {}", e)),
        }
    }
}

// This From impl is for SignerMiddlewareError<Provider<Http>, LocalWallet> -> ProviderErrorWrapper
impl<MError, SError> From<SignerMiddlewareError<Provider<Http>, LocalWallet>> for ProviderErrorWrapper<MError, SError>
where
    MError: StdErrorTrait + Send + Sync + 'static,
    SError: StdErrorTrait + Send + Sync + 'static,
    <Provider<Http> as Middleware>::Error: Into<MError>,
    <LocalWallet as Signer>::Error: Into<SError>,
{
    fn from(err: SignerMiddlewareError<Provider<Http>, LocalWallet>) -> Self {
        match err {
            SignerMiddlewareError::MiddlewareError(e) => ProviderErrorWrapper::Middleware(e.into()),
            SignerMiddlewareError::SignerError(e) => ProviderErrorWrapper::Signer(e.into()),
            SignerMiddlewareError::ProviderError(e) => ProviderErrorWrapper::Provider(e.into()), // Use .into()
            SignerMiddlewareError::NonceMissing => ProviderErrorWrapper::Provider(EthersProviderError::CustomError("NonceMissing from SignerMiddlewareError".to_string())),
            SignerMiddlewareError::GasPriceMissing => ProviderErrorWrapper::Provider(EthersProviderError::CustomError("GasPriceMissing from SignerMiddlewareError".to_string())),
            SignerMiddlewareError::GasMissing => ProviderErrorWrapper::Provider(EthersProviderError::CustomError("GasMissing from SignerMiddlewareError".to_string())),
            SignerMiddlewareError::UnsupportedTxType(s) => ProviderErrorWrapper::Provider(EthersProviderError::CustomError(format!("UnsupportedTxType: {}",s))),
            SignerMiddlewareError::SignatureError(e) => ProviderErrorWrapper::Signer(e.into()),
        }
    }
}

