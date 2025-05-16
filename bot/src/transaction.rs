// bot/src/transaction.rs

use crate::bindings::BalancerVault;
use crate::config::Config;
use crate::encoding::encode_user_data;
use crate::gas::estimate_flash_loan_gas;
use crate::state::{AppState, DexType};
use crate::path_optimizer::RouteCandidate;
use ethers::{
    prelude::*,
    types::{
        transaction::eip2718::TypedTransaction, Address, Bytes, Eip1559TransactionRequest, U256,
        U64, I256, TxHash, H256,
    },
    utils::format_units,
};
use eyre::{eyre, Result, WrapErr};
use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::str::FromStr;
use tokio::sync::Mutex;
use tokio::time::{sleep, timeout}; // Import timeout
use tracing::{debug, error, info, instrument, warn, trace};

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
#[derive(Serialize, Debug)] #[serde(rename_all = "camelCase")] struct AlchemyPrivateTxParams<'a> { tx: &'a str }

// --- NonceManager Impl ---
// (remains unchanged)
impl NonceManager {
    pub fn new(wallet_address: Address) -> Self { Self { current_nonce: Mutex::new(None), wallet_address } }

    #[instrument(skip(self, client), fields(wallet=%self.wallet_address))]
    pub async fn get_next_nonce<M: Middleware + 'static>(&self, client: Arc<M>) -> Result<U256> where M::Error: 'static+Send+Sync {
        let mut guard = self.current_nonce.lock().await;
        let next_nonce = match *guard {
            Some(current) => {
                debug!(current_nonce=%current, "Using cached nonce.");
                current + U256::one()
            },
            None => {
                warn!("Nonce cache empty, fetching from network (pending)...");
                let fetched_nonce = client.get_transaction_count(self.wallet_address, Some(BlockNumber::Pending.into()))
                    .await
                    .wrap_err("Failed to fetch transaction count")?;
                info!(fetched_nonce=%fetched_nonce, "Fetched initial/reset nonce.");
                fetched_nonce
            }
        };
        *guard = Some(next_nonce);
        debug!(next_nonce=%next_nonce, "Next nonce assigned.");
        Ok(next_nonce)
    }

    #[instrument(skip(self), fields(wallet=%self.wallet_address))]
    pub async fn handle_nonce_error(&self) {
        let mut guard = self.current_nonce.lock().await;
        warn!(last_known_nonce=?*guard, "Nonce error detected or suspected, resetting internal nonce cache.");
        *guard = None;
    }

     #[instrument(skip(self), fields(wallet=%self.wallet_address, used_nonce=%used_nonce))]
    pub async fn confirm_nonce_used(&self, used_nonce: U256) {
        let mut guard = self.current_nonce.lock().await;
        match *guard {
            Some(current) if current == used_nonce => {
                *guard = Some(used_nonce + U256::one());
                 debug!(next_nonce=%(used_nonce + U256::one()), "Confirmed nonce used, incremented cache.");
            }
            Some(current) if current > used_nonce => {
                warn!(current_cached_nonce=%current, "Confirmed nonce is lower than current cached nonce, cache might be ahead or reset. Not changing cache.");
            }
            Some(current) if current < used_nonce => {
                 warn!(current_cached_nonce=%current, "Confirmed nonce is higher than expected cached nonce, updating cache to confirmed + 1.");
                *guard = Some(used_nonce + U256::one());
            }
            None => {
                warn!("Confirmed nonce but manager cache was empty, setting next based on confirmed.");
                *guard = Some(used_nonce + U256::one());
            }
             Some(current) => {
                 // This case was previously hitting a panic due to `current` being moved, fixed comparison logic
                 if current != used_nonce { // Explicitly check inequality if not covered above
                    error!(confirmed_nonce=%used_nonce, current_cached_nonce=%current, "Unexpected state in nonce confirmation logic! Check comparison.");
                    // Decide recovery strategy, e.g., reset or use confirmed nonce
                    *guard = Some(used_nonce + U256::one());
                 }
                 // If current == used_nonce, it's handled by the first arm
             }
        }
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
) -> Result<U256> {
    debug!(simulated_net_profit_wei=%simulated_net_profit_wei, "Calculating min profit threshold...");
    if simulated_net_profit_wei <= I256::zero() {
        warn!("Simulated net profit is zero or negative. Setting min profit threshold to 1 wei.");
        return Ok(U256::one());
    }
    let abs_buffer_wei = U256::from_str(&config.min_profit_abs_buffer_wei_str)
        .wrap_err("Failed to parse MIN_PROFIT_ABS_BUFFER_WEI from config")?;
    let abs_buffer = I256::from_raw(abs_buffer_wei);
    let bps_buffer = (simulated_net_profit_wei * I256::from(config.min_profit_buffer_bps)) / I256::from(10000);
    let effective_buffer = std::cmp::max(bps_buffer, abs_buffer);
    debug!(config_bps=config.min_profit_buffer_bps, config_abs_wei=%abs_buffer_wei, calculated_bps_buffer=%bps_buffer, effective_buffer=%effective_buffer);
    let final_buffer = if effective_buffer >= simulated_net_profit_wei {
         simulated_net_profit_wei.saturating_sub(I256::one())
    } else {
         effective_buffer
    };
    debug!(final_buffer=%final_buffer);
    let min_profit_wei_i256 = simulated_net_profit_wei - final_buffer;
    let min_profit_wei_u256 = if min_profit_wei_i256 > I256::zero() {
        min_profit_wei_i256.into_raw()
    } else {
        warn!("Min profit calculation resulted in <= 0 after applying buffer. Setting to 1 wei.");
        U256::one()
    };
    info!(threshold_wei=%min_profit_wei_u256, "Calculated minimum profit threshold for Huff contract.");
    Ok(min_profit_wei_u256)
}


/// Constructs, submits, and monitors the arbitrage transaction using polling.
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

    // DRY-RUN MODE: skip on-chain submission
    if config.dry_run {
        info!("🌵 Dry-run mode enabled: skipping on-chain flash-loan. loan={}, profit={}", 
              loan_amount_wei, simulated_net_profit_wei);
        return Ok(H256::zero());
    }

    // --- Prepare Tx Data ---
    trace!("Step 1: Fetching gas price...");
    let gas_info = fetch_gas_price(client.clone(), config).await.wrap_err("ALERT: Failed gas price fetch pre-submission")?;
    trace!("Step 2: Calculating min profit threshold...");
    let min_profit_wei_u256 = calculate_min_profit_threshold(simulated_net_profit_wei, config)
        .wrap_err("ALERT: Failed to calculate minimum profit threshold")?;
    trace!("Step 3: Generating salt...");
    let salt = U256::from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos());
    trace!("Step 4: Determining effective router address...");
    let effective_router_addr = {
         if route.buy_dex_type.is_velo_style() || route.sell_dex_type.is_velo_style() {
            if route.buy_dex_type == DexType::Aerodrome || route.sell_dex_type == DexType::Aerodrome {
                config.aerodrome_router_addr.ok_or_else(|| eyre!("Aerodrome Router address missing from config but needed for route"))?
            } else {
                config.velo_router_addr
            }
        } else {
             // Should not happen in 2-swap arb, but use Velo as default
             config.velo_router_addr
        }
    };
    trace!("Step 5: Encoding user data...");
    let user_data = encode_user_data( route.buy_pool_addr, route.sell_pool_addr, app_state.usdc_address, route.zero_for_one_a, route.buy_dex_type.is_velo_style(), route.sell_dex_type.is_velo_style(), effective_router_addr, min_profit_wei_u256, salt )?;

    // --- Step 6: Estimate Gas with Timeout ---
    trace!("Step 6: Estimating gas limit (timeout: {}s)...", GAS_ESTIMATION_TIMEOUT_SECS);
    let gas_est_timeout = Duration::from_secs(GAS_ESTIMATION_TIMEOUT_SECS);
    let gas_estimate_result = timeout(
        gas_est_timeout,
        estimate_flash_loan_gas(
            client.clone(),
            config.balancer_vault_address,
            config.arb_executor_address.ok_or_else(|| eyre!("Executor address missing for gas estimate"))?,
            app_state.weth_address, // Use loan token (WETH) from app_state
            loan_amount_wei,
            user_data.clone(),
        )
    ).await;

    let estimated_gas_limit = match gas_estimate_result {
        Ok(Ok(est)) => {
            trace!("Gas estimation successful: {}", est);
            est
        }
        Ok(Err(e)) => {
             // Propagate error if gas estimation fails within timeout
            error!(error = ?e, "ALERT: Gas estimation failed pre-submission");
            return Err(e.wrap_err("ALERT: Gas estimation failed pre-submission"));
        }
        Err(_) => {
            // Handle timeout specifically
             error!(timeout_secs = gas_est_timeout.as_secs(), "ALERT: Gas estimation timed out pre-submission");
             return Err(eyre!("ALERT: Gas estimation timed out after {}s", gas_est_timeout.as_secs()));
        }
    };

    trace!("Step 7: Calculating final gas limit...");
    let final_gas_limit = std::cmp::max(estimated_gas_limit * (100 + config.gas_limit_buffer_percentage) / 100, U256::from(config.min_flashloan_gas_limit));
    trace!("Step 8: Getting next nonce...");
    let nonce = nonce_manager.get_next_nonce(client.clone()).await.wrap_err("ALERT: Nonce fetch failed pre-submission")?;
    trace!("Step 9: Preparing contract call...");
    let balancer_contract = BalancerVault::new(config.balancer_vault_address, client.clone());
    let executor_address = config.arb_executor_address.ok_or_else(|| eyre!("Executor address missing for flash loan target"))?;
    let calldata = balancer_contract.flash_loan( executor_address, vec![app_state.weth_address], vec![loan_amount_wei], user_data, ).calldata().ok_or_else(|| eyre!("ALERT: Calldata generation failed"))?;
    trace!("Step 10: Constructing transaction request...");
    let tx_request = Eip1559TransactionRequest::new().to(config.balancer_vault_address).value(U256::zero()).data(calldata).gas(final_gas_limit).max_fee_per_gas(gas_info.max_fee_per_gas).max_priority_fee_per_gas(gas_info.max_priority_fee_per_gas).nonce(nonce).chain_id(client.signer().chain_id());
    info!(nonce = %nonce, gas_limit = %final_gas_limit, max_fee = %gas_info.max_fee_per_gas, max_prio = %gas_info.max_priority_fee_per_gas, min_profit_req_wei = %min_profit_wei_u256, "Constructed Tx Request");
    let typed_tx: TypedTransaction = tx_request.clone().into();
    trace!("Step 11: Signing transaction...");
    let signature = client.signer().sign_transaction(&typed_tx).await.wrap_err("ALERT: Signing failed pre-submission")?;
    let rlp_signed = typed_tx.rlp_signed(&signature);
    let rlp_hex = format!("0x{}", hex::encode(rlp_signed.as_ref()));
    trace!("Transaction signed. RLP Hex: {}", rlp_hex); // Be careful logging this if sensitive

    // --- Step 12: Attempt Submissions Sequentially ---
    trace!("Step 12: Attempting sequential submission...");
    let submitted_tx_hash = match timeout(Duration::from_secs(TX_SUBMISSION_TIMEOUT_SECS), submit_sequentially( &config, client.provider(), client.clone(), &rlp_hex, &rlp_signed )).await {
        Ok(Ok(hash)) => {
            tracing::Span::current().record("tx_hash", tracing::field::debug(hash));
            info!(%hash, "Transaction submitted successfully.");
            hash
        },
        Ok(Err(submission_error)) => {
            error!(error = ?submission_error, route = ?route, "ALERT: All transaction submission attempts failed.");
            if submission_error.to_string().to_lowercase().contains("nonce") || submission_error.to_string().to_lowercase().contains("known transaction") {
                warn!("Submission error likely due to nonce, resetting manager state.");
                nonce_manager.handle_nonce_error().await;
            }
            return Err(submission_error);
        }
        Err(_) => {
             error!(timeout_secs = TX_SUBMISSION_TIMEOUT_SECS, route = ?route, "ALERT: Timeout during transaction submission attempt.");
             warn!("Submission timeout, resetting nonce manager state.");
             nonce_manager.handle_nonce_error().await;
             return Err(eyre!("Timeout submitting transaction"));
        }
    };

    // --- Step 13: Monitor Submitted Transaction via Polling ---
    // (Monitoring logic remains unchanged)
    info!(%submitted_tx_hash, "Monitoring transaction confirmation (Polling every {}ms, Timeout: {}s)...", TX_POLLING_INTERVAL_MS, TX_CONFIRMATION_TIMEOUT_SECS);
    let confirmation_start_time = SystemTime::now();
    let mut poll_count = 0;

    loop {
        if confirmation_start_time.elapsed()? > Duration::from_secs(TX_CONFIRMATION_TIMEOUT_SECS) {
            warn!(%submitted_tx_hash, timeout_secs = TX_CONFIRMATION_TIMEOUT_SECS, route = ?route, "ALERT: Timeout waiting for transaction confirmation via polling.");
            nonce_manager.handle_nonce_error().await;
            return Err(eyre!("Timeout confirming tx {}", submitted_tx_hash));
        }

        poll_count += 1;
        trace!(%submitted_tx_hash, poll_attempt = poll_count, "Polling for transaction receipt...");

        match client.get_transaction_receipt(submitted_tx_hash).await {
            Ok(Some(receipt)) => {
                let gas_used = receipt.gas_used.unwrap_or_default();
                let effective_gas_price = receipt.effective_gas_price.unwrap_or_default();
                let gas_cost_eth = format_units(gas_used * effective_gas_price, "ether").unwrap_or_default();

                if receipt.status == Some(TX_SUCCESS_STATUS) {
                     info!(tx_hash = %receipt.transaction_hash, block = %receipt.block_number.unwrap_or_default(), gas_used = %gas_used, gas_cost_eth = %gas_cost_eth, route = ?route, "ALERT: ✅✅✅ Tx Confirmed & Succeeded!");
                     nonce_manager.confirm_nonce_used(nonce).await;
                     return Ok(submitted_tx_hash);
                } else {
                     error!(tx_hash = %submitted_tx_hash, status = ?receipt.status, block = %receipt.block_number.unwrap_or_default(), gas_used = %gas_used, gas_cost_eth = %gas_cost_eth, route = ?route, "ALERT: ❌ Tx Confirmed but REVERTED on-chain!");
                     nonce_manager.confirm_nonce_used(nonce).await;
                     // Consider logging revert reason if available from trace
                     // let trace = client.trace_transaction(submitted_tx_hash).await; etc.
                     return Err(eyre!("Transaction reverted on-chain: {}", submitted_tx_hash));
                }
            }
            Ok(None) => {
                 trace!(%submitted_tx_hash, "Transaction still pending...");
                 if poll_count == TX_STALLED_POLL_COUNT {
                     warn!(%submitted_tx_hash, polls = poll_count, "Transaction has not confirmed after {} polls (~{}s). Might be stalled or mempool is busy.", TX_STALLED_POLL_COUNT, (TX_STALLED_POLL_COUNT as u64 * TX_POLLING_INTERVAL_MS) / 1000);
                 }
                 sleep(Duration::from_millis(TX_POLLING_INTERVAL_MS)).await;
                 continue;
            }
            Err(provider_err) => {
                 warn!(%submitted_tx_hash, error = ?provider_err, "Error fetching transaction receipt. Retrying polling...");
                 if provider_err.to_string().contains("transaction not found") {
                     error!(%submitted_tx_hash, "ALERT: Transaction likely dropped or replaced (not found by provider). Resetting nonce.");
                     nonce_manager.handle_nonce_error().await;
                     return Err(eyre!("Transaction likely dropped/replaced: {}", submitted_tx_hash));
                 }
                 sleep(Duration::from_millis(TX_POLLING_INTERVAL_MS)).await;
                 continue;
            }
        }
    }
}

// --- Helper functions (send_alchemy_private_tx, send_flashbots_private_tx, submit_sequentially) ---
// (remain unchanged)
async fn send_alchemy_private_tx( provider: &Provider<Http>, rlp_hex: &str ) -> Result<TxHash> {
    let method="alchemy_sendPrivateTransaction";
    let params=AlchemyPrivateTxParams{tx:rlp_hex};
    provider.request(method,[serde_json::to_value(params)?]).await
        .map_err(|e|eyre!("Alchemy RPC error: {}", e.to_string()))
}
async fn send_flashbots_private_tx( provider: &Provider<Http>, rlp_hex: &str ) -> Result<TxHash> {
    let method="eth_sendPrivateRawTransaction";
    let params=[rlp_hex];
    provider.request(method,params).await
        .map_err(|e|eyre!("Flashbots RPC error: {}", e.to_string()))
}
#[instrument(level="debug", skip(config, _provider, client, rlp_hex, rlp_signed))]
async fn submit_sequentially(
    config: &Config,
    _provider: &Provider<Http>,
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    rlp_hex: &str,
    rlp_signed: &Bytes
) -> Result<TxHash> {
    async fn try_relay(url: &str, rlp_hex: &str) -> Result<TxHash> {
        debug!("Attempting submission via relay: {}", url);
        let relay_provider = Provider::<Http>::try_from(url.to_string())
             .wrap_err_with(|| format!("Failed to create provider for relay: {}", url))?;
        let result = if url.contains("alchemy") {
            send_alchemy_private_tx(&relay_provider, rlp_hex).await
        } else {
            send_flashbots_private_tx(&relay_provider, rlp_hex).await
        };
        match result {
            Ok(tx_hash) => {
                info!(%tx_hash, relay = url, "Submitted via Private Relay.");
                Ok(tx_hash)
            }
            Err(e) => {
                warn!(error = ?e, relay = url, "Private Relay submission failed.");
                Err(e)
            }
        }
    }
    if let Some(url) = &config.private_rpc_url {
        if !url.is_empty() {
             match try_relay(url, rlp_hex).await {
                 Ok(hash) => return Ok(hash),
                 Err(_) => {} // Ignore error, try next
             }
        }
    }
    if let Some(url) = &config.secondary_private_rpc_url {
         if !url.is_empty() {
            match try_relay(url, rlp_hex).await {
                Ok(hash) => return Ok(hash),
                Err(_) => {} // Ignore error, try next
            }
         }
    }
    info!("Attempting submission via Public RPC...");
    match client.send_raw_transaction(rlp_signed.clone()).await {
        Ok(pending_tx) => {
            let tx_hash = pending_tx.tx_hash();
            info!(%tx_hash, "Submitted via Public RPC.");
            return Ok(tx_hash);
        }
        Err(provider_error) => {
            let error_string = provider_error.to_string();
            error!(error = error_string, "Public RPC submission failed.");
            return Err(eyre!(provider_error).wrap_err(format!("Public RPC submission failed: {}", error_string)));
        }
    }
}