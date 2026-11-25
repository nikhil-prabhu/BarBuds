//! Simulated earbuds for every supported model, so the app can show a model's
//! popover without the hardware. Each simulator speaks its model's real
//! protocol, so the UI runs through the same driver code as real earbuds.

use std::sync::{Arc, Mutex, MutexGuard};

use crate::drivers::simulator_for;
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
    use crate::model::{ControlSetting, EqSettings};
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
