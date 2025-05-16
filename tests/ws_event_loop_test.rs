#![cfg(feature = "local_simulation")]
use std::sync::Arc;
use eyre::Result;
use ethers::types::U256;
use ethers::utils::parse_ether;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};

use ulp1_5::{
    config,
    run_event_loop_ws_test,
    local_simulator::{setup_simulation_environment, trigger_v3_swap_via_router, SIMULATION_CONFIG},
    state::AppState,
};

#[tokio::test]
#[ignore]
async fn test_ws_event_loop_triggers_arbitrage() -> Result<()> {
    let sim_env = setup_simulation_environment().await?;

    // prepare AppState and inject test flag
    let mut cfg = config::load_config()?;
    cfg.deploy_executor = false;
    cfg.arb_executor_address = sim_env.executor_address;

    // supply HTTP provider, client, and nonce manager
    let client = sim_env.http_client.clone();
    let http_provider = client.provider().clone();
    let nonce_manager = Arc::new(NonceManager::new(sim_env.wallet_address));
    let state = Arc::new(AppState::new(http_provider, client.clone(), nonce_manager.clone(), cfg));

    let test_flag = Arc::new(Mutex::new(false));
    state.set_test_arb_check_flag(test_flag.clone());

    // spawn WS event-loop test
    let ws_url = SIMULATION_CONFIG.anvil_ws_url;
    let handle = tokio::spawn(run_event_loop_ws_test(state.clone(), ws_url));

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

    handle.await??;

    // assert that the test flag was set
    assert!(*test_flag.lock().await);

    Ok(())
}
