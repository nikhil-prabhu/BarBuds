//! A made-up protocol for building the UI before any real driver exists.
//!
//! Every frame, in both directions:
//! ```text
//! +------+------+-----+--------------+----------+
//! | 0xBB | type | len | payload[len] | checksum |
//! +------+------+-----+--------------+----------+
//! ```
//! `checksum` is the XOR of `type`, `len` and every payload byte. Frame types
//! and payload layouts are in [`codec`]. [`MockEarbuds`] simulates the device side.

mod codec;
mod device;

pub use device::MockEarbuds;

use crate::driver::{Decoded, Event, EarbudDriver};
use crate::error::BarBudsError;
use crate::model::{AncMode, DeviceCapabilities, EqBand, EqCapabilities, EqPreset, EqSettings};
use codec::frame_type;

pub(crate) const BAND_COUNT: usize = 5;
const BAND_FREQUENCIES_HZ: [u32; BAND_COUNT] = [60, 250, 1_000, 4_000, 12_000];
const MIN_GAIN_DB: i8 = -6;
const MAX_GAIN_DB: i8 = 6;

/// Band gains the device applies for each built-in preset.
fn preset_curve(preset: EqPreset) -> Option<[i8; BAND_COUNT]> {
    match preset {
        EqPreset::Flat => Some([0, 0, 0, 0, 0]),
        EqPreset::BassBoost => Some([5, 3, 0, 0, 0]),
        EqPreset::Vocal => Some([-2, 0, 3, 2, 0]),
        EqPreset::TrebleBoost => Some([0, 0, 0, 3, 5]),
        EqPreset::Custom => None,
    }
}

fn eq_settings(preset: EqPreset, gains: &[i8; BAND_COUNT]) -> EqSettings {
    EqSettings {
        preset,
        bands: BAND_FREQUENCIES_HZ
            .iter()
            .zip(gains)
            .map(|(&frequency_hz, &gain)| EqBand {
                frequency_hz,
                gain_db: f32::from(gain),
            })
            .collect(),
    }
}

/// Round slider values to the device's 1 dB steps and clamp to its range.
fn quantize_gains(bands: &[EqBand]) -> Result<[i8; BAND_COUNT], BarBudsError> {
    if bands.len() != BAND_COUNT {
        return Err(BarBudsError::invalid_argument(format!(
            "expected {BAND_COUNT} EQ bands, got {}",
            bands.len()
        )));
    }
    let mut gains = [0i8; BAND_COUNT];
    for (gain, band) in gains.iter_mut().zip(bands) {
        if !band.gain_db.is_finite() {
            return Err(BarBudsError::invalid_argument(format!(
                "EQ gain for {} Hz is not a finite number",
                band.frequency_hz
            )));
        }
        *gain = band
            .gain_db
            .round()
            .clamp(f32::from(MIN_GAIN_DB), f32::from(MAX_GAIN_DB)) as i8;
    }
    Ok(gains)
}

pub(crate) struct MockDriver;

impl EarbudDriver for MockDriver {
    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            display_name: "Mock Buds".to_string(),
            anc_modes: vec![AncMode::Off, AncMode::NoiseCancelling, AncMode::Transparency],
            eq: Some(EqCapabilities {
                band_frequencies_hz: BAND_FREQUENCIES_HZ.to_vec(),
                min_gain_db: f32::from(MIN_GAIN_DB),
                max_gain_db: f32::from(MAX_GAIN_DB),
                step_db: 1.0,
                presets: vec![
                    EqPreset::Flat,
                    EqPreset::BassBoost,
                    EqPreset::Vocal,
                    EqPreset::TrebleBoost,
                    EqPreset::Custom,
                ],
            }),
            has_charging_case: true,
        }
    }

    fn parse_frame(&self, buf: &[u8]) -> Option<Decoded> {
        let (consumed, frame) = codec::decode(buf)?;
        let result = frame.and_then(|frame| match frame.frame_type {
            frame_type::BATTERY => codec::decode_battery(frame.payload).map(Event::Battery),
            frame_type::ANC => codec::decode_anc(frame.payload).map(Event::Anc),
            frame_type::EQ => codec::decode_eq(frame.payload)
                .map(|(preset, gains)| Event::Eq(eq_settings(preset, &gains))),
            other => Err(BarBudsError::UnknownFrameType { frame_type: other }),
        });
        Some(Decoded { consumed, result })
    }

    fn encode_status_request(&self) -> Vec<u8> {
        codec::encode(frame_type::STATUS_REQUEST, &[])
    }

    fn encode_anc(&self, mode: AncMode) -> Result<Vec<u8>, BarBudsError> {
        Ok(codec::encode(frame_type::SET_ANC, &[codec::anc_to_byte(mode)]))
    }

    fn encode_eq(&self, eq: &EqSettings) -> Result<Vec<u8>, BarBudsError> {
        let gains = match preset_curve(eq.preset) {
            Some(curve) => curve,
            None => quantize_gains(&eq.bands)?,
        };
        Ok(codec::encode(frame_type::SET_EQ, &codec::eq_payload(eq.preset, &gains)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bands(gains: &[f32]) -> Vec<EqBand> {
        BAND_FREQUENCIES_HZ
            .iter()
            .zip(gains)
            .map(|(&frequency_hz, &gain_db)| EqBand { frequency_hz, gain_db })
            .collect()
    }

    #[test]
    fn custom_eq_is_rounded_and_clamped() {
        let eq = EqSettings {
            preset: EqPreset::Custom,
            bands: bands(&[-9.0, -0.4, 0.6, 2.5, 12.0]),
        };
        let frame = MockDriver.encode_eq(&eq).unwrap();
        let (_, raw) = codec::decode(&frame).unwrap();
        let (_, gains) = codec::decode_eq(raw.unwrap().payload).unwrap();
        assert_eq!(gains, [-6, 0, 1, 3, 6]);
    }

    #[test]
    fn preset_eq_ignores_supplied_bands() {
        let eq = EqSettings {
            preset: EqPreset::BassBoost,
            bands: vec![],
        };
        let frame = MockDriver.encode_eq(&eq).unwrap();
        let (_, raw) = codec::decode(&frame).unwrap();
        let (preset, gains) = codec::decode_eq(raw.unwrap().payload).unwrap();
        assert_eq!(preset, EqPreset::BassBoost);
        assert_eq!(Some(gains), preset_curve(EqPreset::BassBoost));
    }

    #[test]
    fn custom_eq_rejects_wrong_band_count_and_nan() {
        let short = EqSettings {
            preset: EqPreset::Custom,
            bands: bands(&[0.0; 3]),
        };
        assert!(matches!(
            MockDriver.encode_eq(&short),
            Err(BarBudsError::InvalidArgument { .. })
        ));
        let nan = EqSettings {
            preset: EqPreset::Custom,
            bands: bands(&[0.0, f32::NAN, 0.0, 0.0, 0.0]),
        };
        assert!(matches!(
            MockDriver.encode_eq(&nan),
            Err(BarBudsError::InvalidArgument { .. })
        ));
    }

    #[test]
    fn host_rejects_command_frames_from_device() {
        let frame = codec::encode(frame_type::SET_ANC, &[1]);
        let decoded = MockDriver.parse_frame(&frame).unwrap();
        assert_eq!(decoded.consumed, frame.len());
        assert_eq!(
            decoded.result,
            Err(BarBudsError::UnknownFrameType { frame_type: frame_type::SET_ANC })
        );
    }
}
