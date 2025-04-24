// src/gas.rs
// Module for handling gas estimation.

use ethers::{
    prelude::{Middleware, SignerMiddleware, Provider, Http, LocalWallet}, // Core types
    // Need contract bindings to generate calldata
    // FIX: Correct import path for BalancerVault from our bindings module
    // bindings::BalancerVault, // <- REMOVE THIS WRONG LINE
    types::{Address, Bytes, Eip1559TransactionRequest, U256}, // Tx types & Bytes
};
use eyre::{Result, WrapErr}; // Error handling
use std::sync::Arc; // Arc for client

// Re-import from crate root
use crate::bindings::BalancerVault; // <<< ADD THIS CORRECT LINE

/// Estimates the gas required for the Balancer flash loan transaction.
// ... (Rest of the file is the same as previous correct version) ...
pub async fn estimate_flash_loan_gas(
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    balancer_vault_address: Address,
    receiver: Address,
    token_in: Address,
    amount_in_wei: U256,
    user_data: Bytes,
) -> Result<U256> {
    println!("      Estimating gas...");

    // Prepare parameters for flash loan calldata generation
    let tokens = vec![token_in];
    let amounts = vec![amount_in_wei];

    // Generate the calldata using the imported BalancerVault type
    let flash_loan_calldata = BalancerVault::new(balancer_vault_address, client.clone())
        .flash_loan(receiver, tokens, amounts, user_data)
        .calldata()
        .ok_or_else(|| eyre::eyre!("Failed to get flashLoan calldata"))?;

    // Create the transaction request for estimation
    let tx_request = Eip1559TransactionRequest::new()
        .to(balancer_vault_address)
        .data(flash_loan_calldata);

    // Estimate gas using the client
    let estimated_gas_units = client
        .estimate_gas(&tx_request.into(), None)
        .await
        .wrap_err("Gas estimation failed")?;

    println!("      Est. Gas Units: {}", estimated_gas_units);
    Ok(estimated_gas_units)
}