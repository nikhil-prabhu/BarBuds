//! BarBuds core: a sans-I/O earbud protocol engine exposed to Swift via UniFFI.
//!
//! The Swift shell owns Bluetooth I/O. It feeds raw bytes it reads from the
//! device into [`BarBudsController::feed`] and writes the frames returned by the
//! `encode_*` methods back to the device. Nothing in this crate touches the OS.

uniffi::setup_scaffolding!();

mod controller;
mod driver;
mod drivers;
mod error;
mod fast_pair;
mod model;
mod simulator;

pub use controller::BarBudsController;
pub use drivers::mock::MockEarbuds;
pub use error::BarBudsError;
pub use fast_pair::{FindController, FindSimulator, RingState};
pub use model::*;
pub use simulator::DeviceSimulator;

/// Returns the version of the core library.
#[uniffi::export]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// The Bluetooth Classic RFCOMM service (SDP UUID) to open for `model`, or
/// `None` for models that aren't reached over RFCOMM (such as `Mock`).
#[uniffi::export]
pub fn rfcomm_service_uuid(model: DeviceModel) -> Option<String> {
    drivers::rfcomm_service_uuid(model).map(String::from)
}

/// Every device model the core has a driver for.
#[uniffi::export]
pub fn supported_models() -> Vec<DeviceModel> {
    vec![
        DeviceModel::Mock,
        DeviceModel::PixelBudsPro,
        DeviceModel::PixelBudsASeries,
        DeviceModel::NothingEarOpen,
        // Last: discovery picks the first match, and every specific model
        // above may also offer Fast Pair.
        DeviceModel::FastPair,
    ]
}
