//! Vendor-specific drivers.

mod fast_pair;
mod google;
mod nothing;
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
        DeviceModel::Nothing => nothing::SERVICE_UUID,
        DeviceModel::FastPair => crate::fast_pair::FAST_PAIR_SERVICE_UUID,
    }
}

/// Every Nothing and CMF model the `Nothing` driver knows.
pub(crate) fn nothing_models() -> Vec<crate::model::NothingModelInfo> {
    nothing::models::MODELS
        .iter()
        .map(|m| crate::model::NothingModelInfo {
            id: m.id,
            name: m.name.to_string(),
            tested: m.tested,
        })
        .collect()
}

/// A simulated Nothing or CMF device of the model with this ID.
pub(crate) fn nothing_simulator(id: u16) -> Option<Box<dyn Simulator>> {
    let model = nothing::models::model(id)?;
    Some(Box::new(nothing::sim::NothingSim::new(model)))
}

/// A simulated device speaking `model`'s protocol, for the app's Debug menu.
pub(crate) fn simulator_for(model: DeviceModel) -> Box<dyn Simulator> {
    match model {
        DeviceModel::PixelBudsPro => Box::new(pixel_buds_pro::sim::PixelBudsSim::new(
            pixel_buds_pro::sim::DEFAULT_CHANNEL,
        )),
        DeviceModel::PixelBudsASeries => Box::<pixel_buds_a_series::sim::ASeriesSim>::default(),
        DeviceModel::Nothing => Box::<nothing::sim::NothingSim>::default(),
        DeviceModel::FastPair => Box::<fast_pair::sim::FastPairSim>::default(),
    }
}

pub(crate) fn driver_for(model: DeviceModel) -> Box<dyn EarbudDriver> {
    match model {
        DeviceModel::PixelBudsPro => Box::<pixel_buds_pro::PixelBudsProDriver>::default(),
        DeviceModel::PixelBudsASeries => {
            Box::<pixel_buds_a_series::PixelBudsASeriesDriver>::default()
        }
        DeviceModel::Nothing => Box::<nothing::NothingDriver>::default(),
        DeviceModel::FastPair => Box::<fast_pair::FastPairDriver>::default(),
    }
}
