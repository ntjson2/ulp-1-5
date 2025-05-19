// src/gas.rs
// Module for handling gas estimation.

use ethers::{
    prelude::{Middleware, SignerMiddleware, Provider, Http, LocalWallet}, // Core types
    // Contract bindings are imported via crate root in this version
    types::{Address, Bytes, Eip1559TransactionRequest, U256}, // Tx types & Bytes
};
use eyre::{Result, WrapErr}; // Error handling
use std::sync::Arc; // Arc for client
use tracing::{debug, instrument}; // Import tracing macros

// Re-import BalancerVault binding from crate root
use crate::bindings::BalancerVault;
use crate::config::Config; // Assuming Config is in crate::config
use std::error::Error as StdError; // Added for the 'static bound

#[derive(Debug, Clone, Copy)]
pub struct GasInfo {
    pub max_fee_per_gas: U256,
    pub max_priority_fee_per_gas: U256,
}

/// Estimates the gas required for the Balancer flash loan transaction.
/// This involves sending an `eth_estimateGas` RPC call.
#[instrument(skip(client, user_data), level = "debug", fields(
    vault = %balancer_vault_address,
    receiver = %receiver,
    token = %token_in,
    amount = %amount_in_wei,
))]
pub async fn estimate_flash_loan_gas(
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    balancer_vault_address: Address,
    receiver: Address, // The address that will receive the flash loan (our ArbitrageExecutor)
    token_in: Address, // The token being loaned
    amount_in_wei: U256, // The amount of the token being loaned
    user_data: Bytes,   // Encoded data passed to the receiver's callback
) -> Result<U256> {
    debug!("Estimating gas for flash loan transaction...");

    // Prepare parameters for flash loan calldata generation
    let tokens = vec![token_in];
    let amounts = vec![amount_in_wei];

    // Create a BalancerVault instance to generate calldata easily
    let vault_contract = BalancerVault::new(balancer_vault_address, client.clone());

    // Generate the calldata for the flashLoan function call
    let flash_loan_calldata = vault_contract
        .flash_loan(receiver, tokens, amounts, user_data)
        .calldata() // Get the Bytes representation of the call
        .ok_or_else(|| eyre::eyre!("Failed to generate flashLoan calldata"))?; // Handle potential Option::None

    // Create the transaction request for estimation
    // We only need `to` and `data` for gas estimation. `from` will be filled by the middleware.
    let tx_request = Eip1559TransactionRequest::new()
        .to(balancer_vault_address)
        .data(flash_loan_calldata);

    // Estimate gas using the client middleware
    // The `estimate_gas` function takes a `&TypedTransaction` and optional block number.
    // We convert our Eip1559 request into a generic `TypedTransaction`.
    let estimated_gas_units = client
        .estimate_gas(&tx_request.clone().into(), None) // Use .into() for conversion, clone tx_request if needed later
        .await
        .wrap_err_with(|| format!( // Add context to the error
            "Gas estimation failed for flashLoan to vault {} for receiver {}",
            balancer_vault_address, receiver
        ))?;

    debug!(estimated_gas = %estimated_gas_units, "Gas estimation successful");
    Ok(estimated_gas_units)
}

pub async fn fetch_gas_price<M: Middleware>(
    client: Arc<M>,
    _config: &Config, // config might be used for overrides or strategy
) -> Result<GasInfo> 
where <M as Middleware>::Error: StdError + Send + Sync + 'static { // Added 'static bound
    // Placeholder: Fetch current EIP-1559 gas prices
    // In a real scenario, you might use client.get_gas_price() for legacy,
    // or client.estimate_eip1559_fees(None) for EIP-1559.
    // For simplicity, returning fixed values or simple calculation.
    let (max_fee, priority_fee) = client.estimate_eip1559_fees(None).await?;
    Ok(GasInfo {
        max_fee_per_gas: max_fee,
        max_priority_fee_per_gas: priority_fee,
    })
}
// END OF FILE: bot/src/gas.rs