//! pigweed `pw_rpc` packets, and how Maestro maps RPC channels to HDLC addresses.
//!
//! Each RPC packet is a protobuf (`pw.rpc.packet.RpcPacket`) carried in the
//! data of one HDLC UI frame. Services and methods are identified by a 65599
//! hash of their names, not by the names themselves.

use super::hdlc;
use crate::error::BarBudsError;
use prost::Message;

/// `pw.rpc.packet.RpcPacket`.
#[derive(Clone, PartialEq, Message)]
pub(crate) struct RpcPacket {
    #[prost(int32, tag = "1")]
    pub packet_type: i32,
    #[prost(uint32, tag = "2")]
    pub channel_id: u32,
    #[prost(fixed32, tag = "3")]
    pub service_id: u32,
    #[prost(fixed32, tag = "4")]
    pub method_id: u32,
    #[prost(bytes = "vec", tag = "5")]
    pub payload: Vec<u8>,
    #[prost(uint32, tag = "6")]
    pub status: u32,
    #[prost(uint32, tag = "7")]
    pub call_id: u32,
}

/// `pw.rpc.packet.PacketType`. Client-to-server types are even, server-to-client odd.
pub(crate) mod packet_type {
    pub const REQUEST: i32 = 0;
    pub const RESPONSE: i32 = 1;
    pub const SERVER_ERROR: i32 = 5;
    pub const SERVER_STREAM: i32 = 7;
}

/// `pw::Status` codes the driver reports by name.
pub(crate) fn status_name(code: u32) -> &'static str {
    match code {
        0 => "OK",
        1 => "CANCELLED",
        2 => "UNKNOWN",
        3 => "INVALID_ARGUMENT",
        5 => "NOT_FOUND",
        7 => "PERMISSION_DENIED",
        9 => "FAILED_PRECONDITION",
        12 => "UNIMPLEMENTED",
        13 => "INTERNAL",
        14 => "UNAVAILABLE",
        _ => "error",
    }
}

/// pw_rpc's name hash: starts from the length and adds each character times
/// increasing powers of 65599, wrapping at 32 bits.
pub(crate) const fn hash(name: &str) -> u32 {
    const K: u32 = 65599;
    let bytes = name.as_bytes();
    let mut hash = bytes.len() as u32;
    let mut coefficient = K;
    let mut i = 0;
    while i < bytes.len() {
        hash = hash.wrapping_add(coefficient.wrapping_mul(bytes[i] as u32));
        coefficient = coefficient.wrapping_mul(K);
        i += 1;
    }
    hash
}

/// A method of the `maestro_pw.Maestro` service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Method {
    GetSoftwareInfo,
    SubscribeRuntimeInfo,
    WriteSetting,
    ReadSetting,
    SubscribeToSettingsChanges,
}

pub(crate) const MAESTRO_SERVICE: u32 = hash("maestro_pw.Maestro");

impl Method {
    const ALL: [Method; 5] = [
        Method::GetSoftwareInfo,
        Method::SubscribeRuntimeInfo,
        Method::WriteSetting,
        Method::ReadSetting,
        Method::SubscribeToSettingsChanges,
    ];

    const fn name(self) -> &'static str {
        match self {
            Method::GetSoftwareInfo => "GetSoftwareInfo",
            Method::SubscribeRuntimeInfo => "SubscribeRuntimeInfo",
            Method::WriteSetting => "WriteSetting",
            Method::ReadSetting => "ReadSetting",
            Method::SubscribeToSettingsChanges => "SubscribeToSettingsChanges",
        }
    }

    pub(crate) const fn id(self) -> u32 {
        hash(self.name())
    }

    pub(crate) fn from_packet(packet: &RpcPacket) -> Option<Method> {
        if packet.service_id != MAESTRO_SERVICE {
            return None;
        }
        Method::ALL.into_iter().find(|m| m.id() == packet.method_id)
    }
}

/// Endpoints inside the earbuds and on the host. Maestro packs a source and a
/// target peer into the HDLC address, and each pairing has a fixed RPC channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Peer {
    Case = 2,
    LeftBtCore = 3,
    RightBtCore = 4,
    /// Host-side endpoints. The earbuds decide which one a connection gets.
    MaestroA = 10,
    MaestroB = 13,
}

/// The channels a host may be assigned. The device answers on exactly one of
/// them, so the driver probes them all after connecting.
pub(crate) const CANDIDATE_CHANNELS: [u32; 6] = [18, 19, 21, 23, 24, 26];

fn peers_for_channel(channel: u32) -> Option<(Peer, Peer)> {
    use Peer::*;
    Some(match channel {
        18 => (MaestroA, Case),
        19 => (MaestroA, LeftBtCore),
        21 => (MaestroA, RightBtCore),
        23 => (MaestroB, Case),
        24 => (MaestroB, LeftBtCore),
        26 => (MaestroB, RightBtCore),
        _ => return None,
    })
}

fn hdlc_address(channel: u32) -> Option<u32> {
    let (source, target) = peers_for_channel(channel)?;
    Some(((source as u32 & 0xF) << 6) | ((target as u32 & 0xF) << 10))
}

/// Wrap a client request in an HDLC frame.
pub(crate) fn encode_request(
    channel: u32,
    method: Method,
    call_id: u32,
    payload: Vec<u8>,
) -> Vec<u8> {
    encode_packet(&RpcPacket {
        packet_type: packet_type::REQUEST,
        channel_id: channel,
        service_id: MAESTRO_SERVICE,
        method_id: method.id(),
        payload,
        status: 0,
        call_id,
    })
}

pub(crate) fn encode_packet(packet: &RpcPacket) -> Vec<u8> {
    let address = hdlc_address(packet.channel_id).expect("only candidate channels are used");
    hdlc::encode(&hdlc::Frame {
        address,
        control: hdlc::CONTROL_UI,
        data: packet.encode_to_vec(),
    })
}

pub(crate) fn decode_packet(frame: &hdlc::Frame) -> Result<RpcPacket, BarBudsError> {
    if frame.control != hdlc::CONTROL_UI {
        return Err(BarBudsError::UnknownFrameType {
            frame_type: frame.control,
        });
    }
    RpcPacket::decode(frame.data.as_slice())
        .map_err(|err| BarBudsError::malformed(format!("bad RPC packet: {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_hashes() {
        // Values from pbpctrl, which match what the earbuds expect.
        assert_eq!(MAESTRO_SERVICE, 0x7EDE_71EA);
        assert_eq!(Method::GetSoftwareInfo.id(), 0x7199_FA44);
        assert_eq!(Method::SubscribeToSettingsChanges.id(), 0x2821_ADF5);
    }

    #[test]
    fn method_hashes_are_distinct() {
        for (i, a) in Method::ALL.iter().enumerate() {
            for b in &Method::ALL[i + 1..] {
                assert_ne!(a.id(), b.id(), "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn channel_addresses() {
        // MaestroA (10) -> Case (2): 10 << 6 | 2 << 10.
        assert_eq!(hdlc_address(18), Some(0x0A80));
        for channel in CANDIDATE_CHANNELS {
            assert!(hdlc_address(channel).is_some(), "channel {channel}");
        }
        assert_eq!(hdlc_address(20), None);
    }

    #[test]
    fn request_round_trips_through_hdlc() {
        let bytes = encode_request(21, Method::ReadSetting, 7, vec![0x20, 0x0D]);
        let (_, frame) = hdlc::decode(&bytes[1..]).unwrap();
        let packet = decode_packet(&frame.unwrap().unwrap()).unwrap();
        assert_eq!(packet.channel_id, 21);
        assert_eq!(Method::from_packet(&packet), Some(Method::ReadSetting));
        assert_eq!(packet.call_id, 7);
        assert_eq!(packet.payload, [0x20, 0x0D]);
    }
}
