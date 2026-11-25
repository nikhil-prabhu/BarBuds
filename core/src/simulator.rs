//! Simulated earbuds for every supported model, so the app can show a model's
//! popover without the hardware. Each simulator speaks its model's real
//! protocol, so the UI runs through the same driver code as real earbuds.

use std::sync::{Arc, Mutex, MutexGuard};

use crate::drivers::{nothing_simulator, simulator_for};
use crate::error::BarBudsError;
use crate::model::DeviceModel;

/// The device side of a control channel.
pub(crate) trait Simulator: Send {
    /// Bytes the host writes. Returns the bytes the device sends back, which
    /// may hold zero or more frames.
    fn receive(&mut self, bytes: &[u8]) -> Vec<u8>;

    /// Simulate time passing (e.g. battery drain). Returns any update the
    /// device would send unprompted.
    fn tick(&mut self) -> Vec<u8>;
}

/// A simulated pair of earbuds of any supported model. Swift uses it in place
/// of a Bluetooth channel: write the controller's frames to `receive`, and
/// feed what it returns back into a `BarBudsController` of the same model.
#[derive(uniffi::Object)]
pub struct DeviceSimulator {
    model: DeviceModel,
    device: Mutex<Box<dyn Simulator>>,
}

impl DeviceSimulator {
    fn device(&self) -> MutexGuard<'_, Box<dyn Simulator>> {
        self.device
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[uniffi::export]
impl DeviceSimulator {
    #[uniffi::constructor]
    pub fn new(model: DeviceModel) -> Arc<Self> {
        Arc::new(Self {
            model,
            device: Mutex::new(simulator_for(model)),
        })
    }

    /// A simulated Nothing or CMF device of one model (see `nothing_models()`);
    /// `new(Nothing)` plays an Ear (open).
    #[uniffi::constructor]
    pub fn nothing(model_id: u16) -> Result<Arc<Self>, BarBudsError> {
        let device = nothing_simulator(model_id).ok_or_else(|| {
            BarBudsError::invalid_argument(format!("no Nothing model {model_id:#06x}"))
        })?;
        Ok(Arc::new(Self {
            model: DeviceModel::Nothing,
            device: Mutex::new(device),
        }))
    }

    pub fn model(&self) -> DeviceModel {
        self.model
    }

    pub fn receive(&self, bytes: Vec<u8>) -> Vec<u8> {
        self.device().receive(&bytes)
    }

    pub fn tick(&self) -> Vec<u8> {
        self.device().tick()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::BarBudsController;
    use crate::model::{ANC_CYCLE_MIN_MODES, AncMode, ControlSetting, EqSettings};
    use crate::supported_models;

    /// Relay bytes between controller and simulator until both are quiet,
    /// like the Swift session does. Panics on any decode error.
    fn exchange(controller: &BarBudsController, sim: &DeviceSimulator, bytes: Vec<u8>) {
        let mut to_device = bytes;
        while !to_device.is_empty() {
            controller.feed(sim.receive(to_device)).unwrap();
            to_device = controller.take_outbound();
        }
    }

    fn connected(model: DeviceModel) -> (Arc<BarBudsController>, Arc<DeviceSimulator>) {
        let controller = BarBudsController::new(model);
        let sim = DeviceSimulator::new(model);
        exchange(&controller, &sim, controller.start());
        (controller, sim)
    }

    #[test]
    fn every_model_connects_and_reports_state() {
        for model in supported_models() {
            let (controller, _) = connected(model);
            let state = controller.state();
            let caps = controller.capabilities();
            assert!(state.battery.left.is_some(), "{model:?}: battery");
            if caps.eq.is_some() {
                assert!(state.eq.is_some(), "{model:?}: EQ");
            }
            for toggle in &caps.toggles {
                assert!(state.toggles.contains_key(toggle), "{model:?}: {toggle:?}");
            }
        }
    }

    #[test]
    fn every_nothing_model_can_be_simulated() {
        let models = crate::nothing_models();
        assert_eq!(models.len(), 19);
        assert!(
            models
                .iter()
                .any(|m| m.name == "Nothing Ear (open)" && m.tested)
        );
        for info in models {
            let controller = BarBudsController::new(DeviceModel::Nothing);
            let device = DeviceSimulator::nothing(info.id).unwrap();
            let mut outbound = controller.start();
            while !outbound.is_empty() {
                controller.feed(device.receive(outbound)).unwrap();
                outbound = controller.take_outbound();
            }
            // Identified by asking: the simulator answers the model ID.
            assert_eq!(controller.capabilities().display_name, info.name);
            assert!(controller.state().battery.left.is_some(), "{}", info.name);
        }
        assert!(DeviceSimulator::nothing(0x1234).is_err());
    }

    #[test]
    fn every_noise_control_cycle_round_trips() {
        for model in supported_models() {
            let (controller, sim) = connected(model);
            let offered = controller.capabilities().anc_cycle_modes;
            if offered.is_empty() {
                assert!(controller.encode_anc_cycle(vec![]).is_err());
                continue;
            }
            assert!(
                controller.state().anc_cycle.is_some(),
                "{model:?}: not reported"
            );
            // Every subset of at least two modes.
            for mask in 0u32..(1 << offered.len()) {
                let modes: Vec<AncMode> = offered
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| mask & (1 << i) != 0)
                    .map(|(_, &m)| m)
                    .collect();
                if modes.len() < ANC_CYCLE_MIN_MODES as usize {
                    assert!(controller.encode_anc_cycle(modes).is_err());
                    continue;
                }
                exchange(
                    &controller,
                    &sim,
                    controller.encode_anc_cycle(modes.clone()).unwrap(),
                );
                assert_eq!(controller.state().anc_cycle, Some(modes), "{model:?}");
            }
        }
    }

    #[test]
    fn every_gesture_takes_every_offered_action() {
        for model in supported_models() {
            let (controller, sim) = connected(model);
            let Some(controls) = controller.capabilities().controls else {
                continue;
            };
            for slot in controls.slots {
                assert!(
                    controller
                        .state()
                        .controls
                        .iter()
                        .any(|c| c.side == slot.side && c.gesture == slot.gesture),
                    "{model:?}: {slot:?} not reported"
                );
                for action in slot.actions {
                    let setting = ControlSetting {
                        side: slot.side,
                        gesture: slot.gesture,
                        action,
                    };
                    exchange(
                        &controller,
                        &sim,
                        controller.encode_control(setting).unwrap(),
                    );
                    assert!(
                        controller.state().controls.contains(&setting),
                        "{model:?}: {setting:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn every_toggle_round_trips() {
        for model in supported_models() {
            let (controller, sim) = connected(model);
            for toggle in controller.capabilities().toggles {
                let on = controller.state().toggles[&toggle];
                exchange(
                    &controller,
                    &sim,
                    controller.encode_toggle(toggle, !on).unwrap(),
                );
                assert_eq!(
                    controller.state().toggles[&toggle],
                    !on,
                    "{model:?}: {toggle:?}"
                );
            }
        }
    }

    #[test]
    fn every_anc_mode_eq_preset_and_custom_curve_round_trips() {
        for model in supported_models() {
            let (controller, sim) = connected(model);
            let caps = controller.capabilities();
            for mode in caps.anc_modes {
                exchange(&controller, &sim, controller.encode_anc(mode).unwrap());
                assert_eq!(controller.state().anc, Some(mode), "{model:?}: {mode:?}");
            }
            let Some(eq) = caps.eq else { continue };
            for preset in eq.presets {
                let gains = controller.state().eq.map(|eq| eq.gains).unwrap_or_default();
                let request = EqSettings {
                    preset: Some(preset.id.clone()),
                    gains,
                };
                exchange(&controller, &sim, controller.encode_eq(request).unwrap());
                assert_eq!(
                    controller.state().eq.unwrap().preset.as_deref(),
                    Some(preset.id.as_str()),
                    "{model:?}: {}",
                    preset.name
                );
            }
            // Models with bands also take a custom curve: every band at its maximum.
            if !eq.band_labels.is_empty() {
                let gains = vec![eq.max_gain; eq.band_labels.len()];
                let request = EqSettings {
                    preset: None,
                    gains: gains.clone(),
                };
                exchange(&controller, &sim, controller.encode_eq(request).unwrap());
                let state = controller.state().eq.unwrap();
                assert_eq!(state.preset, None, "{model:?}: custom");
                assert_eq!(state.gains, gains, "{model:?}: custom");
            }
        }
    }

    #[test]
    fn balance_round_trips_where_supported() {
        for model in supported_models() {
            let (controller, sim) = connected(model);
            if !controller.capabilities().has_balance {
                assert!(controller.encode_balance(10).is_err(), "{model:?}");
                continue;
            }
            for balance in [-100, -37, 0, 1, 64, 100] {
                exchange(
                    &controller,
                    &sim,
                    controller.encode_balance(balance).unwrap(),
                );
                assert_eq!(controller.state().balance, Some(balance), "{model:?}");
            }
        }
    }

    #[test]
    fn tick_drains_the_battery() {
        for model in supported_models() {
            let (controller, sim) = connected(model);
            let before = controller.state().battery.left.unwrap().percent;
            controller.feed(sim.tick()).unwrap();
            let after = controller.state().battery.left.unwrap().percent;
            assert_eq!(after, before - 1, "{model:?}");
        }
    }

    #[test]
    fn garbage_never_panics() {
        for model in supported_models() {
            let sim = DeviceSimulator::new(model);
            for bytes in [
                vec![0xFF; 64],
                vec![0x7E, 0x00, 0x7E],
                vec![0x87, 0x08, 0x00, 0x01, 0xFF],
            ] {
                let _ = sim.receive(bytes);
            }
        }
    }
}
