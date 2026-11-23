//! Vendor-specific drivers.

pub(crate) mod mock;
mod pixel_buds_pro;

use crate::driver::EarbudDriver;
use crate::model::DeviceModel;

/// The RFCOMM service UUID a model's control channel is advertised under.
pub(crate) fn rfcomm_service_uuid(model: DeviceModel) -> Option<&'static str> {
    match model {
        DeviceModel::Mock => None,
        DeviceModel::PixelBudsPro => Some(pixel_buds_pro::SERVICE_UUID),
    }
}

pub(crate) fn driver_for(model: DeviceModel) -> Box<dyn EarbudDriver> {
    match model {
        DeviceModel::Mock => Box::new(mock::MockDriver),
        DeviceModel::PixelBudsPro => Box::<pixel_buds_pro::PixelBudsProDriver>::default(),
    }
}
