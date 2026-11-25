//! Ringing earbuds to find them, via the Google Fast Pair message stream: a
//! second RFCOMM channel ([`FAST_PAIR_SERVICE_UUID`]) that Fast Pair earbuds
//! offer next to their control channel. Swift opens it only while the user
//! is finding their earbuds.
//!
//! Every message, in both directions:
//! ```text
//! group u8 | code u8 | length u16 (big-endian) | data[length]
//! ```
//! - Ring (host → device): group `0x04` (device action), code `0x01`, one
//!   data byte: bit 0 = ring right, bit 1 = ring left; `0x00` stops.
//! - The device answers with an acknowledgement, group `0xFF`: code `0x01`
//!   (ACK) or `0x02` (NAK), naming the acknowledged group and code.
//! - While ringing, the device sends ring status updates (group `0x04`, code
//!   `0x01`, the same bits) whenever they change, including when it stops on
//!   its own; the host acknowledges each one.
//!
//! Sources: Google's Fast Pair Message Stream specification, and pbpctrl's
//! `ring` example (MIT/Apache-2.0), which rings real Pixel Buds Pro.

use std::sync::{Arc, Mutex, MutexGuard};

use crate::error::BarBudsError;

/// RFCOMM service UUID of the Fast Pair message stream.
pub const FAST_PAIR_SERVICE_UUID: &str = "df21fe2c-2515-4fdb-8886-f12c4d67927c";

pub(crate) const HEADER_LEN: usize = 4;
/// Message stream payloads are small; anything larger means lost sync.
pub(crate) const MAX_DATA: usize = 1024;

mod group {
    pub const DEVICE_ACTION: u8 = 0x04;
    pub const ACKNOWLEDGEMENT: u8 = 0xFF;
}

mod code {
    pub const RING: u8 = 0x01;
    pub const ACK: u8 = 0x01;
    pub const NAK: u8 = 0x02;
}

const RING_RIGHT: u8 = 0b01;
const RING_LEFT: u8 = 0b10;

/// The Fast Pair message stream's RFCOMM service UUID, for Swift to open.
#[uniffi::export]
pub fn fast_pair_service_uuid() -> String {
    FAST_PAIR_SERVICE_UUID.to_string()
}

/// Which buds are ringing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, uniffi::Record)]
pub struct RingState {
    pub left: bool,
    pub right: bool,
}

impl RingState {
    fn from_bits(bits: u8) -> Self {
        Self {
            left: bits & RING_LEFT != 0,
            right: bits & RING_RIGHT != 0,
        }
    }

    fn bits(self) -> u8 {
        (if self.left { RING_LEFT } else { 0 }) | (if self.right { RING_RIGHT } else { 0 })
    }
}

pub(crate) fn encode(group: u8, code: u8, data: &[u8]) -> Vec<u8> {
    let len = u16::try_from(data.len()).expect("message stream data is small");
    let mut message = Vec::with_capacity(HEADER_LEN + data.len());
    message.extend_from_slice(&[group, code]);
    message.extend_from_slice(&len.to_be_bytes());
    message.extend_from_slice(data);
    message
}

/// Split one message off the front of `buf`: `(consumed, group, code, data)`.
/// `None` means more bytes are needed.
pub(crate) fn decode(buf: &[u8]) -> Option<(usize, u8, u8, &[u8])> {
    if buf.len() < HEADER_LEN {
        return None;
    }
    let len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    if len > MAX_DATA {
        return Some((1, 0, 0, &[])); // Out of sync: skip a byte.
    }
    let total = HEADER_LEN + len;
    if buf.len() < total {
        return None;
    }
    Some((total, buf[0], buf[1], &buf[HEADER_LEN..total]))
}

/// Whether an acknowledgement's data names the ring command. ACKs carry
/// `[group, code]`; NAKs may put a reason byte first.
fn acknowledges_ring(data: &[u8]) -> bool {
    data.windows(2)
        .any(|pair| pair == [group::DEVICE_ACTION, code::RING])
}

/// Sans-I/O controller for the Fast Pair "find" (ring) feature of one device.
///
/// Swift flow: open the Fast Pair channel, write `ring(…)`, then pass every
/// chunk read to `feed()` and write whatever `take_outbound()` returns (the
/// acknowledgements the device expects). `state()` is what the device
/// confirmed is ringing.
#[derive(uniffi::Object)]
pub struct FindController {
    inner: Mutex<FindInner>,
}

#[derive(Default)]
struct FindInner {
    rx: Vec<u8>,
    outbound: Vec<u8>,
    state: RingState,
    /// The ring command sent but not yet acknowledged.
    pending: Option<RingState>,
}

impl FindController {
    fn inner(&self) -> MutexGuard<'_, FindInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[uniffi::export]
impl FindController {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(FindInner::default()),
        })
    }

    /// The command to make exactly these buds ring; ringing neither stops.
    pub fn ring(&self, left: bool, right: bool) -> Vec<u8> {
        let requested = RingState { left, right };
        self.inner().pending = Some(requested);
        encode(group::DEVICE_ACTION, code::RING, &[requested.bits()])
    }

    pub fn stop(&self) -> Vec<u8> {
        self.ring(false, false)
    }

    /// Feed bytes read from the Fast Pair channel. Returns the confirmed
    /// ring state, or an error if the device refused a ring command (state is
    /// still updated for any other messages in the chunk).
    pub fn feed(&self, bytes: Vec<u8>) -> Result<RingState, BarBudsError> {
        let mut guard = self.inner();
        let FindInner {
            rx,
            outbound,
            state,
            pending,
        } = &mut *guard;
        rx.extend_from_slice(&bytes);

        let mut first_error = None;
        let mut offset = 0;
        while let Some((consumed, group, code, data)) = decode(&rx[offset..]) {
            offset += consumed;
            match (group, code) {
                (group::DEVICE_ACTION, code::RING) => {
                    // A status update: confirm it, as the device expects.
                    if let Some(&bits) = data.first() {
                        *state = RingState::from_bits(bits);
                    }
                    outbound.extend(encode(
                        group::ACKNOWLEDGEMENT,
                        code::ACK,
                        &[group::DEVICE_ACTION, code::RING],
                    ));
                }
                (group::ACKNOWLEDGEMENT, code::ACK) if acknowledges_ring(data) => {
                    if let Some(requested) = pending.take() {
                        *state = requested;
                    }
                }
                (group::ACKNOWLEDGEMENT, code::NAK) if acknowledges_ring(data) => {
                    pending.take();
                    first_error.get_or_insert(BarBudsError::device(
                        "the earbuds can't ring right now (e.g. while in the case or in your ears)",
                    ));
                }
                _ => {} // Battery and other Fast Pair events: not ours.
            }
        }
        rx.drain(..offset);

        match first_error {
            Some(err) => Err(err),
            None => Ok(*state),
        }
    }

    pub fn take_outbound(&self) -> Vec<u8> {
        std::mem::take(&mut self.inner().outbound)
    }

    pub fn state(&self) -> RingState {
        self.inner().state
    }
}

/// The device side of the Fast Pair ring feature, for the app's Debug menu
/// and tests. It acknowledges ring commands, reports the new state, and stops
/// ringing on its own after a few ticks, like real earbuds do.
#[derive(uniffi::Object)]
pub struct FindSimulator {
    inner: Mutex<FindSimInner>,
}

#[derive(Default)]
struct FindSimInner {
    rx: Vec<u8>,
    ringing: RingState,
    ticks_ringing: u32,
}

/// Ticks a simulated ring lasts before stopping by itself.
const SIM_RING_TICKS: u32 = 15;

impl FindSimulator {
    fn inner(&self) -> MutexGuard<'_, FindSimInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[uniffi::export]
impl FindSimulator {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(FindSimInner::default()),
        })
    }

    pub fn receive(&self, bytes: Vec<u8>) -> Vec<u8> {
        let mut sim = self.inner();
        sim.rx.extend_from_slice(&bytes);
        let mut out = Vec::new();
        let mut offset = 0;
        let mut commands = Vec::new();
        while let Some((consumed, group, code, data)) = decode(&sim.rx[offset..]) {
            offset += consumed;
            if (group, code) == (group::DEVICE_ACTION, code::RING) {
                commands.push(data.first().copied().unwrap_or(0));
            }
        }
        sim.rx.drain(..offset);
        for bits in commands {
            sim.ringing = RingState::from_bits(bits);
            sim.ticks_ringing = 0;
            out.extend(encode(
                group::ACKNOWLEDGEMENT,
                code::ACK,
                &[group::DEVICE_ACTION, code::RING],
            ));
            out.extend(encode(group::DEVICE_ACTION, code::RING, &[bits]));
        }
        out
    }

    /// Time passing: after a while, a ringing simulator stops by itself.
    pub fn tick(&self) -> Vec<u8> {
        let mut sim = self.inner();
        if sim.ringing == RingState::default() {
            return Vec::new();
        }
        sim.ticks_ringing += 1;
        if sim.ticks_ringing < SIM_RING_TICKS {
            return Vec::new();
        }
        sim.ringing = RingState::default();
        encode(group::DEVICE_ACTION, code::RING, &[0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace()
            .map(|b| u8::from_str_radix(b, 16).unwrap())
            .collect()
    }

    #[test]
    fn ring_commands_match_pbpctrl() {
        let finder = FindController::new();
        // pbpctrl rings both buds with group 0x04, code 0x01, data [0x03].
        assert_eq!(finder.ring(true, true), hex("04 01 00 01 03"));
        assert_eq!(finder.ring(false, true), hex("04 01 00 01 01"));
        assert_eq!(finder.ring(true, false), hex("04 01 00 01 02"));
        assert_eq!(finder.stop(), hex("04 01 00 01 00"));
    }

    #[test]
    fn ack_confirms_the_requested_state() {
        let finder = FindController::new();
        finder.ring(true, false);
        assert_eq!(finder.state(), RingState::default());
        let state = finder.feed(hex("ff 01 00 02 04 01")).unwrap();
        assert_eq!(
            state,
            RingState {
                left: true,
                right: false
            }
        );
    }

    #[test]
    fn status_updates_are_applied_and_acknowledged() {
        let finder = FindController::new();
        finder.feed(hex("04 01 00 01 03")).unwrap();
        assert_eq!(
            finder.state(),
            RingState {
                left: true,
                right: true
            }
        );
        assert_eq!(finder.take_outbound(), hex("ff 01 00 02 04 01"));
        // The buds stop on their own.
        finder.feed(hex("04 01 00 01 00")).unwrap();
        assert_eq!(finder.state(), RingState::default());
    }

    #[test]
    fn nak_is_an_error_and_keeps_state() {
        let finder = FindController::new();
        finder.ring(true, true);
        // NAK with a reason byte first, then the acknowledged group and code.
        let err = finder.feed(hex("ff 02 00 03 00 04 01")).unwrap_err();
        assert!(matches!(err, BarBudsError::Device { .. }));
        assert_eq!(finder.state(), RingState::default());
    }

    #[test]
    fn ignores_other_fast_pair_events_and_split_delivery() {
        let finder = FindController::new();
        finder.ring(false, true);
        // A battery update (captured from real Pixel Buds Pro) and the ACK,
        // delivered a byte at a time.
        let mut bytes = hex("03 03 00 03 62 64 7f");
        bytes.extend(hex("ff 01 00 02 04 01"));
        for byte in bytes {
            finder.feed(vec![byte]).unwrap();
        }
        assert_eq!(
            finder.state(),
            RingState {
                left: false,
                right: true
            }
        );
        assert!(finder.take_outbound().is_empty());
    }

    #[test]
    fn simulator_round_trip_and_auto_stop() {
        let finder = FindController::new();
        let sim = FindSimulator::new();
        finder.feed(sim.receive(finder.ring(true, true))).unwrap();
        assert_eq!(
            finder.state(),
            RingState {
                left: true,
                right: true
            }
        );
        // The host's acknowledgement of the status update is harmless to the device.
        assert!(sim.receive(finder.take_outbound()).is_empty());
        for _ in 0..SIM_RING_TICKS {
            finder.feed(sim.tick()).unwrap();
        }
        assert_eq!(finder.state(), RingState::default());
    }
}
