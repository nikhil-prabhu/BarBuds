//! Vendor-specific drivers.

pub(crate) mod mock;

use crate::driver::EarbudDriver;
use crate::model::DeviceModel;

pub(crate) fn driver_for(model: DeviceModel) -> Box<dyn EarbudDriver> {
    match model {
        DeviceModel::Mock => Box::new(mock::MockDriver),
    }
}
