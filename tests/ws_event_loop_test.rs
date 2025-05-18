#![cfg(feature = "local_simulation")]
use std::sync::Arc;
use eyre::Result;
use ethers::types::U256;
use ethers::utils::parse_ether;
use ethers::providers::Middleware;
use tokio::time::{sleep, Duration, timeout};
use ulp1_5::{NonceManager, run_event_loop_ws_test, config, AppState};
use ulp1_5::local_simulator::{setup_simulation_environment, trigger_v3_swap_via_router, SIMULATION_CONFIG};

// adjust timeout as needed
const WS_TEST_TIMEOUT: Duration = Duration::from_secs(15);

#[tokio::test]
async fn test_ws_event_loop_triggers_arbitrage() -> Result<()> {
    let sim_env = setup_simulation_environment().await?;

    // prepare AppState for test
    let mut cfg = config::load_config()?;
    cfg.deploy_executor = false;
    cfg.arb_executor_address = sim_env.executor_address;
    let client = sim_env.http_client.clone();
    let http_provider = client.provider().clone();
    let nonce_manager = Arc::new(NonceManager::new(sim_env.wallet_address));

    let state = Arc::new(AppState::new(http_provider, client.clone(), nonce_manager.clone(), cfg));

    // spawn the WS loop (uses AppState internal flag)
    let handle = tokio::spawn(run_event_loop_ws_test(
        state.clone(),
        SIMULATION_CONFIG.anvil_ws_url,
    ));

    sleep(Duration::from_millis(100)).await;

    // trigger a UniV3 swap to generate a Swap event
    let usdc = state.usdc_address;
    let recipient = sim_env.wallet_address;
    trigger_v3_swap_via_router(
        &sim_env,
        parse_ether("0.01")?,
        usdc,
        500,
        recipient,
        U256::zero(),
    )
    .await?;

    // wait until the AppState flag is true or timeout
    timeout(WS_TEST_TIMEOUT, async {
        loop {
            let flag_mutex = state
                .test_arb_check_triggered
                .as_ref()
                .expect("test_arb_check_triggered not initialized");
            if *flag_mutex.lock().await {
                // explicitly specify eyre::Report error type
                break Ok::<(), eyre::Report>(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await??;

    // assert that arbitrage check was triggered
    let flag_mutex = state
        .test_arb_check_triggered
        .as_ref()
        .expect("test_arb_check_triggered not initialized");
    assert!(
        *flag_mutex.lock().await,
        "WS event loop did not set arbitrage check flag"
    );

    // stop the WS loop
    handle.abort();

    Ok(())
}

#[cfg(test)]
mod integration_tests {
    use ulp1_5::run_event_loop_ws_test;
    use std::error::Error;

    #[tokio::test]
    #[ignore]
    async fn test_ws_event_loop_triggers_arbitrage() -> Result<(), Box<dyn Error>> {
        let ws_url = "ws://127.0.0.1:8545/ws";
        let http_url = "http://127.0.0.1:8545";
        let minimal_emitter = "<MINIMAL_SWAP_EMITTER_ADDRESS>";

        let triggered = run_event_loop_ws_test(ws_url, http_url, minimal_emitter).await?;
        assert!(triggered, "WS loop did not detect arbitrage trigger");
        Ok(())
    }
}
