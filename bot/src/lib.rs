// bot/src/lib.rs
// New file to define the library interface

// Import EthEvent trait
use ethers::contract::EthEvent; // <-- Added this line
use ethers::types::H256;
use lazy_static::lazy_static;

// Re-export modules needed by integration tests and potentially the binary
pub mod bindings;
pub mod config;

// single alias for simulation tests
pub use state::AppState;
pub type SimEnv = AppState;

// re-export simulation API for tests
pub use simulation::{
    SimulationConfig,
    SIMULATION_CONFIG,
    setup_simulation_environment,
    trigger_v3_swap_via_router,
    VELO_ROUTER_IMPL_ADDR_FOR_SIM,
    PAIR_DOES_NOT_EXIST_SELECTOR_STR,
};

// Public types/constants re-exported for convenience
pub use state::{AppState, DexType, PoolSnapshot, PoolState}; // Re-export key types
pub use path_optimizer::RouteCandidate;
pub use transaction::NonceManager; // Re-export NonceManager

// Re-export event topics if needed directly by tests/binary

// expose WS test runner from event_handler
#[cfg(feature = "local_simulation")]
pub use event_loop::run_event_loop_ws_test;

// Declare all modules (stubs & real)
mod state;
mod simulation;
mod event_loop;
mod transaction;
mod path_optimizer;
mod gas;
mod utils;
mod encoding;

// Define lazy_static within the lib so they are accessible via the library.
lazy_static! {
    pub static ref UNI_V3_SWAP_TOPIC: H256 = bindings::uniswap_v3_pool::SwapFilter::signature();
    pub static ref UNI_V3_POOL_CREATED_TOPIC: H256 = bindings::i_uniswap_v3_factory::PoolCreatedFilter::signature();
    pub static ref VELO_AERO_SWAP_TOPIC: H256 = bindings::velodrome_v2_pool::SwapFilter::signature();
    pub static ref VELO_AERO_POOL_CREATED_TOPIC: H256 = bindings::i_velodrome_factory::PoolCreatedFilter::signature();
}

// constants referenced by integration_test.rs
pub const VELO_ROUTER_IMPL_ADDR_FOR_SIM: &str = "0x0000000000000000000000000000000000000000";
pub const PAIR_DOES_NOT_EXIST_SELECTOR_STR: &str = "";