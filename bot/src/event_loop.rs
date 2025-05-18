use ethers::providers::{Provider, Ws};
use std::error::Error;

pub async fn run_event_loop_ws_test(
    ws_url: &str,
    _http_url: &str,
    _pool_address: &str,
) -> Result<bool, Box<dyn Error>> {
    // connect via WebSocket
    let _ws = Provider::<Ws>::connect(ws_url).await?;
    // TODO: initialize AppState, subscribe to Swap events, trigger a swap, check a flag
    Ok(false)
}
