//! The single object Swift holds per connected device.

use std::sync::{Arc, Mutex, MutexGuard};

use crate::driver::{Event, EarbudDriver};
use crate::drivers::driver_for;
use crate::error::BarBudsError;
use crate::model::{AncMode, DeviceCapabilities, DeviceModel, DeviceState, EqSettings, FeatureToggle};

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
    capabilities: DeviceCapabilities,
    inner: Mutex<Inner>,
}

struct Inner {
    driver: Box<dyn EarbudDriver>,
    rx: Vec<u8>,
    outbound: Vec<u8>,
    state: DeviceState,
}

impl Inner {
    fn new(model: DeviceModel) -> Self {
        Self {
            driver: driver_for(model),
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
        Event::SerialNumbers(serials) => state.info.serial_numbers = serials,
    }
}

impl BarBudsController {
    fn inner(&self) -> MutexGuard<'_, Inner> {
        // A panic while holding the lock can't leave `Inner` logically broken,
        // so recover instead of propagating the poison to every later call.
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[uniffi::export]
impl BarBudsController {
    #[uniffi::constructor]
    pub fn new(model: DeviceModel) -> Arc<Self> {
        let inner = Inner::new(model);
        Arc::new(Self {
            model,
            capabilities: inner.driver.capabilities(),
            inner: Mutex::new(inner),
        })
    }

    pub fn model(&self) -> DeviceModel {
        self.model
    }

    pub fn capabilities(&self) -> DeviceCapabilities {
        self.capabilities.clone()
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
        let Inner { driver, rx, outbound, state } = &mut *guard;
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
        if !self.capabilities.anc_modes.contains(&mode) {
            return Err(BarBudsError::Unsupported {
                feature: format!("ANC mode {mode:?}"),
            });
        }
        self.inner().driver.encode_anc(mode)
    }

    pub fn encode_eq(&self, eq: EqSettings) -> Result<Vec<u8>, BarBudsError> {
        // A preset must be one the model lists; a custom curve needs bands.
        let supported = self.capabilities.eq.as_ref().is_some_and(|caps| match &eq.preset {
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

    /// Frame turning an on/off feature on or off. The state changes once the
    /// device confirms it through `feed()`.
    pub fn encode_toggle(&self, toggle: FeatureToggle, on: bool) -> Result<Vec<u8>, BarBudsError> {
        if !self.capabilities.toggles.contains(&toggle) {
            return Err(BarBudsError::Unsupported {
                feature: format!("{toggle:?}"),
            });
        }
        self.inner().driver.encode_toggle(toggle, on)
    }

    /// Forget buffered bytes, protocol state and device state, e.g. after a disconnect.
    pub fn reset(&self) {
        *self.inner() = Inner::new(self.model);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::mock::MockEarbuds;
    use crate::model::BatteryReading;

    fn connected() -> (Arc<BarBudsController>, Arc<MockEarbuds>) {
        let controller = BarBudsController::new(DeviceModel::Mock);
        let device = MockEarbuds::new();
        controller
            .feed(device.receive(controller.start()))
            .unwrap();
        (controller, device)
    }

    #[test]
    fn starts_empty() {
        let controller = BarBudsController::new(DeviceModel::Mock);
        assert_eq!(controller.state(), DeviceState::default());
    }

    #[test]
    fn status_request_populates_state() {
        let (controller, _) = connected();
        let state = controller.state();
        assert_eq!(state.battery.left, Some(BatteryReading { percent: 82, charging: false }));
        assert_eq!(state.anc, Some(AncMode::NoiseCancelling));
        assert_eq!(state.eq.unwrap().preset.as_deref(), Some("flat"));
    }

    #[test]
    fn anc_change_applies_after_device_confirms() {
        let (controller, device) = connected();
        let frame = controller.encode_anc(AncMode::Transparency).unwrap();
        assert_eq!(controller.state().anc, Some(AncMode::NoiseCancelling));
        let state = controller.feed(device.receive(frame)).unwrap();
        assert_eq!(state.anc, Some(AncMode::Transparency));
    }

    #[test]
    fn custom_eq_round_trip() {
        let (controller, device) = connected();
        let eq = EqSettings {
            preset: None,
            gains: vec![1.0, 2.0, 3.0, -2.0, -4.0],
        };
        let state = controller
            .feed(device.receive(controller.encode_eq(eq.clone()).unwrap()))
            .unwrap();
        assert_eq!(state.eq, Some(eq));
    }

    #[test]
    fn tick_drains_battery() {
        let (controller, device) = connected();
        let state = controller.feed(device.tick()).unwrap();
        assert_eq!(state.battery.left.unwrap().percent, 81);
        assert_eq!(state.battery.charging_case.unwrap().percent, 64);
    }

    #[test]
    fn handles_byte_at_a_time_delivery() {
        let controller = BarBudsController::new(DeviceModel::Mock);
        let reply = MockEarbuds::new().receive(controller.start());
        for byte in reply {
            controller.feed(vec![byte]).unwrap();
        }
        assert_eq!(controller.state().anc, Some(AncMode::NoiseCancelling));
    }

    #[test]
    fn applies_valid_frames_around_garbage() {
        let controller = BarBudsController::new(DeviceModel::Mock);
        let device = MockEarbuds::new();
        let mut bytes = vec![0xDE, 0xAD];
        bytes.extend(device.receive(controller.start()));
        let err = controller.feed(bytes).unwrap_err();
        assert!(matches!(err, BarBudsError::MalformedFrame { .. }));
        assert_eq!(controller.state().anc, Some(AncMode::NoiseCancelling));
    }

    #[test]
    fn reset_clears_state() {
        let (controller, _) = connected();
        controller.reset();
        assert_eq!(controller.state(), DeviceState::default());
    }
}
