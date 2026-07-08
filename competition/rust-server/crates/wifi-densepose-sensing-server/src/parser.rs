//! Binary protocol parsers for ESP32 CSI, vitals, and WASM output packets.
//!
//! Extracted from `main.rs` to keep the entry point slim.

use crate::types::{Esp32Frame, Esp32VitalsPacket, WasmEvent, WasmOutputPacket};
use tracing::warn;

/// Parse a 32-byte edge vitals packet (magic 0xC511_0002).
pub(crate) fn parse_esp32_vitals(buf: &[u8]) -> Option<Esp32VitalsPacket> {
    if buf.len() < 32 {
        return None;
    }
    let magic = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if magic != 0xC511_0002 {
        return None;
    }

    let node_id = buf[4];
    let flags = buf[5];
    let breathing_raw = u16::from_le_bytes([buf[6], buf[7]]);
    let heartrate_raw = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
    let rssi = buf[12] as i8;
    let n_persons = buf[13];
    let motion_energy = f32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]);
    let presence_score = f32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]);
    let timestamp_ms = u32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]]);

    Some(Esp32VitalsPacket {
        node_id,
        presence: (flags & 0x01) != 0,
        fall_detected: (flags & 0x02) != 0,
        motion: (flags & 0x04) != 0,
        breathing_rate_bpm: (breathing_raw as f64 / 100.0).clamp(4.0, 60.0),
        heartrate_bpm: (heartrate_raw as f64 / 10000.0).clamp(20.0, 300.0),
        rssi,
        n_persons,
        motion_energy,
        presence_score,
        timestamp_ms,
    })
}

/// Parse a WASM output packet (magic 0xC511_0005).
pub(crate) fn parse_wasm_output(buf: &[u8]) -> Option<WasmOutputPacket> {
    if buf.len() < 8 {
        return None;
    }
    let magic = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if magic != 0xC511_0005 {
        return None;
    }

    let node_id = buf[4];
    let module_id = buf[5];
    let event_count = u16::from_le_bytes([buf[6], buf[7]]) as usize;

    let mut events = Vec::with_capacity(event_count);
    let mut offset = 8;
    for _ in 0..event_count {
        if offset + 5 > buf.len() {
            break;
        }
        let event_type = buf[offset];
        let value = f32::from_le_bytes([
            buf[offset + 1], buf[offset + 2], buf[offset + 3], buf[offset + 4],
        ]);
        events.push(WasmEvent { event_type, value });
        offset += 5;
    }

    Some(WasmOutputPacket {
        node_id,
        module_id,
        events,
    })
}

/// Parse an ESP32 CSI binary frame (magic 0xC511_0001).
pub(crate) fn parse_esp32_frame(buf: &[u8]) -> Option<Esp32Frame> {
    if buf.len() < 20 {
        return None;
    }

    let magic = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if magic != 0xC511_0001 {
        return None;
    }

    let node_id = buf[4];
    let n_antennas = buf[5];
    let n_subcarriers = u16::from_le_bytes([buf[6], buf[7]]);
    let freq_mhz = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
    let sequence = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
    let rssi = buf[16] as i8;
    let noise_floor = buf[17] as i8;

    let iq_start = 20;
    let n_pairs = n_antennas as usize * n_subcarriers as usize;

    // BUG 10 fix: bound n_pairs to prevent DoS via malicious packets.
    // C5 max: 1 antenna × 484 subcarriers = 484. With margin: 2048.
    const MAX_PAIRS: usize = 2048;
    if n_pairs > MAX_PAIRS || n_antennas == 0 || n_subcarriers == 0 {
        warn!("Rejecting frame: n_pairs={n_pairs} exceeds MAX_PAIRS={MAX_PAIRS} or zero antennas/subcarriers");
        return None;
    }

    let expected_len = iq_start + n_pairs * 2;

    if buf.len() < expected_len {
        return None;
    }

    let mut amplitudes = Vec::with_capacity(n_pairs);
    let mut phases = Vec::with_capacity(n_pairs);

    for k in 0..n_pairs {
        let i_val = buf[iq_start + k * 2] as i8 as f64;
        let q_val = buf[iq_start + k * 2 + 1] as i8 as f64;
        amplitudes.push((i_val * i_val + q_val * q_val).sqrt());
        phases.push(q_val.atan2(i_val));
    }

    Some(Esp32Frame {
        magic,
        node_id,
        n_antennas,
        n_subcarriers,
        freq_mhz,
        sequence,
        rssi,
        noise_floor,
        amplitudes,
        phases,
    })
}
