//! Vendor-specific drivers.

mod fast_pair;
mod google;
mod nothing_ear_open;
mod pixel_buds_a_series;
mod pixel_buds_pro;

use crate::driver::EarbudDriver;
use crate::model::DeviceModel;
use crate::simulator::Simulator;

/// The RFCOMM service UUID a model's control channel is advertised under.
pub(crate) fn rfcomm_service_uuid(model: DeviceModel) -> &'static str {
    match model {
        DeviceModel::PixelBudsPro => pixel_buds_pro::SERVICE_UUID,
        DeviceModel::PixelBudsASeries => pixel_buds_a_series::SERVICE_UUID,
        DeviceModel::NothingEarOpen => nothing_ear_open::SERVICE_UUID,
        DeviceModel::FastPair => crate::fast_pair::FAST_PAIR_SERVICE_UUID,
    }
}

/// A simulated device speaking `model`'s protocol, for the app's Debug menu.
pub(crate) fn simulator_for(model: DeviceModel) -> Box<dyn Simulator> {
    match model {
        DeviceModel::PixelBudsPro => Box::new(pixel_buds_pro::sim::PixelBudsSim::new(
            pixel_buds_pro::sim::DEFAULT_CHANNEL,
        )),
        DeviceModel::PixelBudsASeries => Box::<pixel_buds_a_series::sim::ASeriesSim>::default(),
        DeviceModel::NothingEarOpen => Box::<nothing_ear_open::sim::NothingEarOpenSim>::default(),
        DeviceModel::FastPair => Box::<fast_pair::sim::FastPairSim>::default(),
    }
}

pub(crate) fn driver_for(model: DeviceModel) -> Box<dyn EarbudDriver> {
    match model {
        DeviceModel::PixelBudsPro => Box::<pixel_buds_pro::PixelBudsProDriver>::default(),
        DeviceModel::PixelBudsASeries => {
            Box::<pixel_buds_a_series::PixelBudsASeriesDriver>::default()
        }
        DeviceModel::NothingEarOpen => Box::<nothing_ear_open::NothingEarOpenDriver>::default(),
        DeviceModel::FastPair => Box::<fast_pair::FastPairDriver>::default(),
    }
}
