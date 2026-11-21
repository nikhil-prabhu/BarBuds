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
mod model;

pub use controller::BarBudsController;
pub use drivers::mock::MockEarbuds;
pub use error::BarBudsError;
pub use model::*;

/// Returns the version of the core library.
#[uniffi::export]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Every device model the core has a driver for.
#[uniffi::export]
pub fn supported_models() -> Vec<DeviceModel> {
    vec![DeviceModel::Mock]
}
