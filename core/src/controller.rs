//! The single object Swift holds per connected device.

use std::sync::{Arc, Mutex, MutexGuard};

use crate::driver::{EarbudDriver, Event};
use crate::drivers::driver_for;
use crate::error::BarBudsError;
use crate::model::{
    ANC_CYCLE_MIN_MODES, AncAvailability, AncMode, AncStrength, BALANCE_MAX, BALANCE_MIN,
    ControlSetting, DeviceCapabilities, DeviceModel, DeviceState, EqSettings, FeatureToggle,
    SpatialAudioMode,
};

/// Drop buffered bytes past this size; a well-behaved device never gets close.
const MAX_RX_BUFFER: usize = 4096;

/// Sans-I/O protocol engine for one device.
///
/// Swift flow:
/// 1. After the Bluetooth channel opens, write `start()` to it.
/// 2. Pass every chunk read from the channel to `feed()` and publish the returned state.
///    Then write whatever `take_outbound()` returns: some protocols answer replies
///    with follow-up requests during the handshake.
/// 3. To change a setting, write the frame from `encode_anc()` / `encode_eq()`.
///    State updates when the device confirms the change through `feed()`, not when you send it.
/// 4. On disconnect, call `reset()`.
#[derive(uniffi::Object)]
pub struct BarBudsController {
    model: DeviceModel,
    inner: Mutex<Inner>,
}

struct Inner {
    driver: Box<dyn EarbudDriver>,
    /// The Bluetooth name passed to `set_device_name`, kept for new drivers.
    device_name: Option<String>,
    rx: Vec<u8>,
    outbound: Vec<u8>,
    state: DeviceState,
}

impl Inner {
    fn new(model: DeviceModel) -> Self {
        Self {
            driver: driver_for(model),
            device_name: None,
            rx: Vec::new(),
            outbound: Vec::new(),
            state: DeviceState::default(),
        }
    }
}

fn apply(state: &mut DeviceState, event: Event) {
    match event {
        Event::Battery(battery) => state.battery = battery,
        Event::Anc(mode) => state.anc = mode,
        Event::Eq(eq) => state.eq = Some(eq),
        Event::Toggle(toggle, on) => {
            state.toggles.insert(toggle, on);
        }
        Event::Firmware(firmware) => state.info.firmware = firmware,
        Event::Balance(balance) => state.balance = Some(balance),
        Event::SerialNumbers(serials) => state.info.serial_numbers = serials,
        Event::AncCycle(modes) => state.anc_cycle = Some(modes),
        Event::AncState(mode, availability) => {
            state.anc = mode;
            state.anc_availability = availability;
        }
        Event::AncAvailability(availability) => state.anc_availability = availability,
        Event::SpatialAudio(mode) => state.spatial_audio = Some(mode),
        Event::NoiseControl { mode, strength } => {
            state.anc = mode;
            if strength.is_some() {
                state.anc_strength = strength;
            }
        }
        Event::Controls(settings) => {
            for setting in settings {
                let slot =
                    |c: &ControlSetting| c.side == setting.side && c.gesture == setting.gesture;
                match state.controls.iter_mut().find(|c| slot(c)) {
                    Some(existing) => *existing = setting,
                    None => state.controls.push(setting),
                }
            }
        }
    }
}

impl BarBudsController {
    fn inner(&self) -> MutexGuard<'_, Inner> {
        // A panic while holding the lock can't leave `Inner` logically broken,
        // so recover instead of propagating the poison to every later call.
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[uniffi::export]
impl BarBudsController {
    #[uniffi::constructor]
    pub fn new(model: DeviceModel) -> Arc<Self> {
        Arc::new(Self {
            model,
            inner: Mutex::new(Inner::new(model)),
        })
    }

    pub fn model(&self) -> DeviceModel {
        self.model
    }

    /// What the model supports. Usually fixed, but a driver that learns
    /// them from the device (e.g. the generic Fast Pair driver's noise-control
    /// modes) may change them as frames arrive, so read them again after `feed()`.
    pub fn capabilities(&self) -> DeviceCapabilities {
        self.inner().driver.capabilities()
    }

    pub fn state(&self) -> DeviceState {
        self.inner().state.clone()
    }

    /// Feed bytes read from the device. Chunks may split or merge frames.
    ///
    /// Every valid frame is applied. If any frame was invalid, the first error
    /// is returned, but valid frames in the same chunk are still applied, so
    /// call `state()` to get the up-to-date state.
    pub fn feed(&self, bytes: Vec<u8>) -> Result<DeviceState, BarBudsError> {
        let mut guard = self.inner();
        let Inner {
            driver,
            rx,
            outbound,
            state,
            ..
        } = &mut *guard;
        rx.extend_from_slice(&bytes);

        let mut first_error = None;
        let mut offset = 0;
        while let Some(decoded) = driver.parse_frame(&rx[offset..], outbound) {
            debug_assert!(decoded.consumed > 0, "driver must always make progress");
            if decoded.consumed == 0 {
                break;
            }
            offset += decoded.consumed;
            match decoded.result {
                Ok(Some(event)) => apply(state, event),
                Ok(None) => {}
                Err(err) => {
                    first_error.get_or_insert(err);
                }
            }
        }
        rx.drain(..offset);

        if rx.len() > MAX_RX_BUFFER {
            let dropped = rx.len();
            rx.clear();
            first_error.get_or_insert(BarBudsError::malformed(format!(
                "dropped {dropped} buffered bytes without a complete frame"
            )));
        }

        match first_error {
            Some(err) => Err(err),
            None => Ok(state.clone()),
        }
    }

    /// Frames to write once the channel opens. They ask the device for its full state.
    pub fn start(&self) -> Vec<u8> {
        self.inner().driver.start()
    }

    /// Frames the driver queued while handling `feed()`. Write them to the
    /// device. Returns an empty buffer when there is nothing to send.
    pub fn take_outbound(&self) -> Vec<u8> {
        std::mem::take(&mut self.inner().outbound)
    }

    pub fn encode_anc(&self, mode: AncMode) -> Result<Vec<u8>, BarBudsError> {
        if !self.capabilities().anc_modes.contains(&mode) {
            return Err(BarBudsError::Unsupported {
                feature: format!("ANC mode {mode:?}"),
            });
        }
        let mut inner = self.inner();
        match inner.state.anc_availability {
            AncAvailability::Available => inner.driver.encode_anc(mode),
            AncAvailability::NotNow => Err(BarBudsError::device(
                "noise control can't be changed right now",
            )),
            AncAvailability::OnDeviceOnly => Err(BarBudsError::device(
                "these earbuds only change noise control on the earbuds themselves",
            )),
        }
    }

    pub fn encode_eq(&self, eq: EqSettings) -> Result<Vec<u8>, BarBudsError> {
        // A preset must be one the model lists; a custom curve needs bands.
        let supported = self
            .capabilities()
            .eq
            .as_ref()
            .is_some_and(|caps| match &eq.preset {
                Some(id) => caps.presets.iter().any(|preset| &preset.id == id),
                None => !caps.band_labels.is_empty(),
            });
        if !supported {
            return Err(BarBudsError::Unsupported {
                feature: match &eq.preset {
                    Some(id) => format!("EQ preset {id:?}"),
                    None => "a custom EQ curve".to_string(),
                },
            });
        }
        self.inner().driver.encode_eq(&eq)
    }

    /// Frame setting the left/right balance (see [`DeviceState::balance`]).
    /// The state changes once the device confirms it through `feed()`.
    pub fn encode_balance(&self, balance: i32) -> Result<Vec<u8>, BarBudsError> {
        if !self.capabilities().has_balance {
            return Err(BarBudsError::Unsupported {
                feature: "balance".to_string(),
            });
        }
        if !(BALANCE_MIN..=BALANCE_MAX).contains(&balance) {
            return Err(BarBudsError::invalid_argument(format!(
                "balance {balance} is outside {BALANCE_MIN}…{BALANCE_MAX}"
            )));
        }
        self.inner().driver.encode_balance(balance)
    }

    /// Frame turning an on/off feature on or off. The state changes once the
    /// device confirms it through `feed()`.
    pub fn encode_toggle(&self, toggle: FeatureToggle, on: bool) -> Result<Vec<u8>, BarBudsError> {
        if !self.capabilities().toggles.contains(&toggle) {
            return Err(BarBudsError::Unsupported {
                feature: format!("{toggle:?}"),
            });
        }
        self.inner().driver.encode_toggle(toggle, on)
    }

    /// Frame changing one gesture's action. The state changes once the
    /// device confirms it through `feed()`.
    pub fn encode_control(&self, setting: ControlSetting) -> Result<Vec<u8>, BarBudsError> {
        let allowed = self
            .capabilities()
            .controls
            .as_ref()
            .is_some_and(|controls| {
                controls.slots.iter().any(|slot| {
                    slot.side == setting.side
                        && slot.gesture == setting.gesture
                        && slot.actions.contains(&setting.action)
                })
            });
        if !allowed {
            return Err(BarBudsError::Unsupported {
                feature: format!(
                    "{:?} {:?} set to {:?}",
                    setting.side, setting.gesture, setting.action
                ),
            });
        }
        self.inner().driver.encode_control(setting)
    }

    /// Frame setting the Noise Cancelling strength (which also turns Noise
    /// Cancelling on). The state changes once the device confirms it.
    pub fn encode_anc_strength(&self, strength: AncStrength) -> Result<Vec<u8>, BarBudsError> {
        if !self.capabilities().anc_strengths.contains(&strength) {
            return Err(BarBudsError::Unsupported {
                feature: format!("noise-cancelling strength {strength:?}"),
            });
        }
        self.inner().driver.encode_anc_strength(strength)
    }

    /// Frame setting the spatial audio mode. The state changes once the
    /// device confirms it through `feed()`.
    pub fn encode_spatial_audio(&self, mode: SpatialAudioMode) -> Result<Vec<u8>, BarBudsError> {
        if !self.capabilities().spatial_audio_modes.contains(&mode) {
            return Err(BarBudsError::Unsupported {
                feature: format!("spatial audio {mode:?}"),
            });
        }
        self.inner().driver.encode_spatial_audio(mode)
    }

    /// Frame choosing which noise-control modes a noise-control gesture
    /// cycles through (at least [`ANC_CYCLE_MIN_MODES`]). The state changes
    /// once the device confirms it through `feed()`.
    pub fn encode_anc_cycle(&self, modes: Vec<AncMode>) -> Result<Vec<u8>, BarBudsError> {
        let offered = &self.capabilities().anc_cycle_modes;
        if offered.is_empty() {
            return Err(BarBudsError::Unsupported {
                feature: "the noise-control cycle".to_string(),
            });
        }
        if let Some(mode) = modes.iter().find(|m| !offered.contains(m)) {
            return Err(BarBudsError::Unsupported {
                feature: format!("{mode:?} in the noise-control cycle"),
            });
        }
        // Capabilities order, without duplicates.
        let modes: Vec<AncMode> = offered
            .iter()
            .copied()
            .filter(|m| modes.contains(m))
            .collect();
        if modes.len() < ANC_CYCLE_MIN_MODES as usize {
            return Err(BarBudsError::invalid_argument(format!(
                "the noise-control cycle needs at least {ANC_CYCLE_MIN_MODES} modes"
            )));
        }
        self.inner().driver.encode_anc_cycle(&modes)
    }

    /// Forget buffered bytes, protocol state and device state, e.g. after a disconnect.
    pub fn reset(&self) {
        let mut inner = self.inner();
        let name = inner.device_name.take();
        *inner = Inner::new(self.model);
        if let Some(name) = name {
            inner.driver.set_device_name(&name);
            inner.device_name = Some(name);
        }
    }

    /// The device's Bluetooth name, before `start()`. Lets a driver that
    /// covers a vendor's lineup (Nothing) recognise the model straight away.
    pub fn set_device_name(&self, name: String) {
        let mut inner = self.inner();
        inner.driver.set_device_name(&name);
        inner.device_name = Some(name);
    }
}

#[cfg(test)]
mod tests {
    //! Model-independent behaviour, against the generic Fast Pair simulator
    //! (simple framing). Each model's own features are tested in its driver
    //! and in `simulator.rs`.
    use super::*;
    use crate::DeviceSimulator;
    use crate::model::BatteryReading;

    fn connected() -> (Arc<BarBudsController>, Arc<DeviceSimulator>) {
        let controller = BarBudsController::new(DeviceModel::FastPair);
        let device = DeviceSimulator::new(DeviceModel::FastPair);
        controller.feed(device.receive(controller.start())).unwrap();
        (controller, device)
    }

    #[test]
    fn starts_empty() {
        let controller = BarBudsController::new(DeviceModel::FastPair);
        assert_eq!(controller.state(), DeviceState::default());
    }

    #[test]
    fn connecting_populates_state() {
        let (controller, _) = connected();
        let state = controller.state();
        assert_eq!(
            state.battery.left,
            Some(BatteryReading {
                percent: 90,
                charging: false
            })
        );
        assert_eq!(state.anc, Some(AncMode::NoiseCancelling));
    }

    #[test]
    fn a_change_applies_after_the_device_confirms() {
        let (controller, device) = connected();
        let frame = controller.encode_anc(AncMode::Transparency).unwrap();
        assert_eq!(controller.state().anc, Some(AncMode::NoiseCancelling));
        let state = controller.feed(device.receive(frame)).unwrap();
        assert_eq!(state.anc, Some(AncMode::Transparency));
    }

    #[test]
    fn unsupported_settings_are_refused() {
        let (controller, _) = connected();
        let eq = EqSettings {
            preset: None,
            gains: vec![1.0],
        };
        assert!(matches!(
            controller.encode_eq(eq),
            Err(BarBudsError::Unsupported { .. })
        ));
        assert!(controller.encode_balance(0).is_err());
    }

    #[test]
    fn tick_drains_battery() {
        let (controller, device) = connected();
        let state = controller.feed(device.tick()).unwrap();
        assert_eq!(state.battery.left.unwrap().percent, 89);
        assert_eq!(state.battery.charging_case.unwrap().percent, 60);
    }

    #[test]
    fn handles_byte_at_a_time_delivery() {
        let controller = BarBudsController::new(DeviceModel::FastPair);
        let reply = DeviceSimulator::new(DeviceModel::FastPair).receive(controller.start());
        for byte in reply {
            controller.feed(vec![byte]).unwrap();
        }
        assert_eq!(controller.state().anc, Some(AncMode::NoiseCancelling));
        assert_eq!(controller.state().battery.right.unwrap().percent, 85);
    }

    #[test]
    fn applies_valid_frames_around_garbage() {
        // Needs a protocol with a start-of-frame byte to resync on (Fast
        // Pair messages have none), so this uses the Nothing protocol.
        let controller = BarBudsController::new(DeviceModel::Nothing);
        let device = DeviceSimulator::new(DeviceModel::Nothing);
        let mut bytes = vec![0xDE, 0xAD];
        bytes.extend(device.receive(controller.start()));
        let err = controller.feed(bytes).unwrap_err();
        assert!(matches!(err, BarBudsError::MalformedFrame { .. }));
        let state = controller.state();
        assert_eq!(state.battery.left.unwrap().percent, 100);
        assert!(state.info.firmware.left.is_some());
    }

    #[test]
    fn reset_clears_state() {
        let (controller, _) = connected();
        controller.reset();
        assert_eq!(controller.state(), DeviceState::default());
    }
}
