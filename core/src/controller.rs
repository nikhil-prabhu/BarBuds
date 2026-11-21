//! The single object Swift holds per connected device.

use std::sync::{Arc, Mutex, MutexGuard};

use crate::driver::{Event, EarbudDriver};
use crate::drivers::driver_for;
use crate::error::BarBudsError;
use crate::model::{AncMode, DeviceCapabilities, DeviceModel, DeviceState, EqSettings};

/// Drop buffered bytes past this size; a well-behaved device never gets close.
const MAX_RX_BUFFER: usize = 4096;

/// Sans-I/O protocol engine for one device.
///
/// Swift flow:
/// 1. After the Bluetooth channel opens, write `status_request()` to it.
/// 2. Pass every chunk read from the channel to `feed()` and publish the returned state.
/// 3. To change a setting, write the frame from `encode_anc()` / `encode_eq()`.
///    State updates when the device confirms the change through `feed()`, not when you send it.
/// 4. On disconnect, call `reset()`.
#[derive(uniffi::Object)]
pub struct BarBudsController {
    model: DeviceModel,
    driver: Box<dyn EarbudDriver>,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    rx: Vec<u8>,
    state: DeviceState,
}

fn apply(state: &mut DeviceState, event: Event) {
    match event {
        Event::Battery(battery) => state.battery = battery,
        Event::Anc(mode) => state.anc = Some(mode),
        Event::Eq(eq) => state.eq = Some(eq),
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
        Arc::new(Self {
            model,
            driver: driver_for(model),
            inner: Mutex::new(Inner::default()),
        })
    }

    pub fn model(&self) -> DeviceModel {
        self.model
    }

    pub fn capabilities(&self) -> DeviceCapabilities {
        self.driver.capabilities()
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
        let Inner { rx, state } = &mut *guard;
        rx.extend_from_slice(&bytes);

        let mut first_error = None;
        let mut offset = 0;
        while let Some(decoded) = self.driver.parse_frame(&rx[offset..]) {
            debug_assert!(decoded.consumed > 0, "driver must always make progress");
            if decoded.consumed == 0 {
                break;
            }
            offset += decoded.consumed;
            match decoded.result {
                Ok(event) => apply(state, event),
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

    /// Frame asking the device to report its full state.
    pub fn status_request(&self) -> Vec<u8> {
        self.driver.encode_status_request()
    }

    pub fn encode_anc(&self, mode: AncMode) -> Result<Vec<u8>, BarBudsError> {
        if !self.driver.capabilities().anc_modes.contains(&mode) {
            return Err(BarBudsError::Unsupported {
                feature: format!("ANC mode {mode:?}"),
            });
        }
        self.driver.encode_anc(mode)
    }

    pub fn encode_eq(&self, eq: EqSettings) -> Result<Vec<u8>, BarBudsError> {
        let supported = self
            .driver
            .capabilities()
            .eq
            .is_some_and(|caps| caps.presets.contains(&eq.preset));
        if !supported {
            return Err(BarBudsError::Unsupported {
                feature: format!("EQ preset {:?}", eq.preset),
            });
        }
        self.driver.encode_eq(&eq)
    }

    /// Forget buffered bytes and device state, e.g. after a disconnect.
    pub fn reset(&self) {
        *self.inner() = Inner::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::mock::MockEarbuds;
    use crate::model::{BatteryReading, EqBand, EqPreset};

    fn connected() -> (Arc<BarBudsController>, Arc<MockEarbuds>) {
        let controller = BarBudsController::new(DeviceModel::Mock);
        let device = MockEarbuds::new();
        controller
            .feed(device.receive(controller.status_request()))
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
        assert_eq!(state.eq.unwrap().preset, EqPreset::Flat);
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
        let caps = controller.capabilities().eq.unwrap();
        let bands = caps
            .band_frequencies_hz
            .iter()
            .zip([1.0, 2.0, 3.0, -2.0, -4.0])
            .map(|(&frequency_hz, gain_db)| EqBand { frequency_hz, gain_db })
            .collect::<Vec<_>>();
        let eq = EqSettings { preset: EqPreset::Custom, bands };
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
        let reply = MockEarbuds::new().receive(controller.status_request());
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
        bytes.extend(device.receive(controller.status_request()));
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
