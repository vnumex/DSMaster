use super::constants::{
    DS_BT_AUDIO_COMBINED_LANE_OFFSET, DS_BT_AUDIO_OPUS_BYTES, DS_BT_AUDIO_SUBPKT_HAPTICS,
    DS_BT_AUDIO_SUBPKT_SESSION, DS_BT_HAPTICS_PCM_BYTES, DS_BT_MIC_OPUS_BYTES, DS_INPUT_REPORT_BT,
    DS_INPUT_REPORT_BT_SIZE, DS_OUTPUT_REPORT_BT_AUDIO, DS_OUTPUT_REPORT_BT_AUDIO_SIZE,
    DS_OUTPUT_REPORT_BT_HAPTICS, DS_OUTPUT_REPORT_BT_HAPTICS_SIZE, PS_INPUT_CRC32_SEED,
    PS_OUTPUT_CRC32_SEED, ProtocolError, crc32_with_seed,
};
use super::output::BtAudioRoute;

/// Folds interleaved stereo `f32` samples `(L, R)` to dual-mono `(M, M)` in place.
#[inline]
pub fn fold_stereo_to_mono_in_place(pcm_interleaved: &mut [f32]) {
    for pair in pcm_interleaved.chunks_exact_mut(2) {
        let mono = (0.5 * (pair[0] + pair[1])).clamp(-1.0, 1.0);
        pair[0] = mono;
        pair[1] = mono;
    }
}

/// Linearly time-compresses 512 stereo frames (10.667 ms at 48 kHz) into 480 stereo frames
/// (10.0 ms Opus frame) so pitch remains exact over DualSense's Bluetooth tick cadence.
#[inline]
pub fn time_compress_512_to_480(src_512: &[f32; 512 * 2], dst_480: &mut [f32; 480 * 2]) {
    let step = 512.0_f64 / 480.0_f64;
    for o in 0..480 {
        let pos = (o as f64) * step;
        let i0 = pos as usize;
        let t = (pos - (i0 as f64)) as f32;
        let i1 = (i0 + 1).min(511);
        let s0_l = src_512[i0 * 2];
        let s0_r = src_512[i0 * 2 + 1];
        let s1_l = src_512[i1 * 2];
        let s1_r = src_512[i1 * 2 + 1];
        dst_480[o * 2] = s0_l + (s1_l - s0_l) * t;
        dst_480[o * 2 + 1] = s0_r + (s1_r - s0_r) * t;
    }
}

/// Decimates 512 stereo 48 kHz frames 16:1 into 32 stereo 3 kHz signed 8-bit PCM frames
/// (64 bytes total) using 16-sample block mean. Returns `true` if any output byte is non-zero.
#[inline]
pub fn decimate_haptics_48k_to_3k(
    pcm_48k: &[f32; 512 * 2],
    gain: f32,
    out_64: &mut [u8; DS_BT_HAPTICS_PCM_BYTES],
) -> bool {
    let scale = gain.clamp(0.0, 1.5) * 127.0;
    let mut has_signal = false;
    for o in 0..32 {
        let mut acc_l = 0.0_f32;
        let mut acc_r = 0.0_f32;
        let base = o * 16 * 2;
        for k in 0..16 {
            acc_l += pcm_48k[base + k * 2];
            acc_r += pcm_48k[base + k * 2 + 1];
        }
        let l = ((acc_l / 16.0) * scale).round().clamp(-128.0, 127.0) as i8 as u8;
        let r = ((acc_r / 16.0) * scale).round().clamp(-128.0, 127.0) as i8 as u8;
        out_64[o * 2] = l;
        out_64[o * 2 + 1] = r;
        if l != 0 || r != 0 {
            has_signal = true;
        }
    }
    has_signal
}

#[inline]
fn write_bt_session_header(
    report_id: u8,
    seq: u8,
    pkt_counter: u8,
    buffer_len: u8,
    mic_open: bool,
    out: &mut [u8],
) {
    out[0] = report_id;
    out[1] = (seq & 0x0F) << 4;
    out[2] = DS_BT_AUDIO_SUBPKT_SESSION | 0x80;
    out[3] = 7;
    out[4] = if mic_open { 0xFF } else { 0xFE };
    out[9] = buffer_len;
    out[10] = pkt_counter;
}

#[inline]
fn write_haptics_subpacket(out: &mut [u8], offset: usize, haptics_64: &[u8; DS_BT_HAPTICS_PCM_BYTES]) {
    out[offset] = DS_BT_AUDIO_SUBPKT_HAPTICS | 0x80;
    out[offset + 1] = DS_BT_HAPTICS_PCM_BYTES as u8;
    out[offset + 2..offset + 2 + DS_BT_HAPTICS_PCM_BYTES].copy_from_slice(haptics_64);
}

#[inline]
fn write_audio_subpacket(out: &mut [u8], offset: usize, route: BtAudioRoute, opus: &[u8]) {
    out[offset] = route.bt_lane_pid() | 0x80;
    out[offset + 1] = DS_BT_AUDIO_OPUS_BYTES as u8;
    let n = opus.len().min(DS_BT_AUDIO_OPUS_BYTES);
    out[offset + 2..offset + 2 + n].copy_from_slice(&opus[..n]);
}

/// Encodes Bluetooth Report `0x32` (142-byte logical payload, zero-padded to `out.len()`).
#[inline]
pub fn encode_bt_haptics_report_32(
    seq: u8,
    pkt_counter: u8,
    buffer_len: u8,
    mic_open: bool,
    haptics_64: &[u8; DS_BT_HAPTICS_PCM_BYTES],
    out: &mut [u8],
) -> Result<usize, ProtocolError> {
    if out.len() < DS_OUTPUT_REPORT_BT_HAPTICS_SIZE {
        return Err(ProtocolError::BufferTooSmall {
            expected: DS_OUTPUT_REPORT_BT_HAPTICS_SIZE,
            actual: out.len(),
        });
    }
    out.fill(0);
    write_bt_session_header(
        DS_OUTPUT_REPORT_BT_HAPTICS,
        seq,
        pkt_counter,
        buffer_len,
        mic_open,
        out,
    );
    write_haptics_subpacket(out, 11, haptics_64);
    let crc = crc32_with_seed(PS_OUTPUT_CRC32_SEED, &out[..138]);
    out[138..142].copy_from_slice(&crc.to_le_bytes());
    Ok(out.len())
}

/// Encodes a standalone Bluetooth Report `0x32` mic open/close toggle (`0xFF` open / `0xFE` close).
#[inline]
pub fn encode_bt_mic_toggle_report_32(
    pkt_counter: u8,
    buffer_len: u8,
    mic_open: bool,
    out: &mut [u8],
) -> Result<usize, ProtocolError> {
    if out.len() < DS_OUTPUT_REPORT_BT_HAPTICS_SIZE {
        return Err(ProtocolError::BufferTooSmall {
            expected: DS_OUTPUT_REPORT_BT_HAPTICS_SIZE,
            actual: out.len(),
        });
    }
    out.fill(0);
    write_bt_session_header(
        DS_OUTPUT_REPORT_BT_HAPTICS,
        0,
        pkt_counter,
        buffer_len,
        mic_open,
        out,
    );
    out[11] = DS_BT_AUDIO_SUBPKT_HAPTICS | 0x80;
    out[12] = DS_BT_HAPTICS_PCM_BYTES as u8;
    let crc = crc32_with_seed(PS_OUTPUT_CRC32_SEED, &out[..138]);
    out[138..142].copy_from_slice(&crc.to_le_bytes());
    Ok(out.len())
}

/// Extracts a 71-byte mono 48 kHz 10 ms Opus microphone packet from a Bluetooth `0x31` input report.
#[inline]
pub fn extract_bt_mic_opus(report: &[u8]) -> Option<[u8; DS_BT_MIC_OPUS_BYTES]> {
    if report.len() < DS_INPUT_REPORT_BT_SIZE
        || report[0] != DS_INPUT_REPORT_BT
        || (report[1] & 0x02) == 0
    {
        return None;
    }
    let expected = crc32_with_seed(PS_INPUT_CRC32_SEED, &report[..74]);
    let actual = u32::from_le_bytes([report[74], report[75], report[76], report[77]]);
    if expected != actual {
        return None;
    }
    let mut pkt = [0u8; DS_BT_MIC_OPUS_BYTES];
    pkt.copy_from_slice(&report[3..3 + DS_BT_MIC_OPUS_BYTES]);
    Some(pkt)
}

/// Encodes Bluetooth Report `0x35` for speaker/headset Opus audio only (334-byte logical payload).
#[inline]
pub fn encode_bt_speaker_report_35(
    seq: u8,
    pkt_counter: u8,
    buffer_len: u8,
    mic_open: bool,
    route: BtAudioRoute,
    opus: &[u8],
    out: &mut [u8],
) -> Result<usize, ProtocolError> {
    if out.len() < DS_OUTPUT_REPORT_BT_AUDIO_SIZE {
        return Err(ProtocolError::BufferTooSmall {
            expected: DS_OUTPUT_REPORT_BT_AUDIO_SIZE,
            actual: out.len(),
        });
    }
    out.fill(0);
    write_bt_session_header(
        DS_OUTPUT_REPORT_BT_AUDIO,
        seq,
        pkt_counter,
        buffer_len,
        mic_open,
        out,
    );
    write_audio_subpacket(out, 11, route, opus);
    let crc = crc32_with_seed(PS_OUTPUT_CRC32_SEED, &out[..330]);
    out[330..334].copy_from_slice(&crc.to_le_bytes());
    Ok(out.len())
}

/// Encodes combined Bluetooth Report `0x35` carrying both 3 kHz haptics (`0x12`) and Opus audio.
#[inline]
pub fn encode_bt_combined_report_35(
    seq: u8,
    pkt_counter: u8,
    buffer_len: u8,
    mic_open: bool,
    route: BtAudioRoute,
    haptics_64: &[u8; DS_BT_HAPTICS_PCM_BYTES],
    opus: &[u8],
    out: &mut [u8],
) -> Result<usize, ProtocolError> {
    if out.len() < DS_OUTPUT_REPORT_BT_AUDIO_SIZE {
        return Err(ProtocolError::BufferTooSmall {
            expected: DS_OUTPUT_REPORT_BT_AUDIO_SIZE,
            actual: out.len(),
        });
    }
    out.fill(0);
    write_bt_session_header(
        DS_OUTPUT_REPORT_BT_AUDIO,
        seq,
        pkt_counter,
        buffer_len,
        mic_open,
        out,
    );
    write_haptics_subpacket(out, 11, haptics_64);
    write_audio_subpacket(out, DS_BT_AUDIO_COMBINED_LANE_OFFSET, route, opus);
    let crc = crc32_with_seed(PS_OUTPUT_CRC32_SEED, &out[..330]);
    out[330..334].copy_from_slice(&crc.to_le_bytes());
    Ok(out.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_bt_haptics_32_layout_and_padding() {
        let mut haptics = [0u8; 64];
        for (i, b) in haptics.iter_mut().enumerate() {
            *b = (i as u8).wrapping_add(1);
        }
        let mut buf = [0xFFu8; 547];
        let written =
            encode_bt_haptics_report_32(5, 0x2A, 48, false, &haptics, &mut buf).unwrap();
        assert_eq!(written, 547);
        assert_eq!(buf[0], 0x32);
        assert_eq!(buf[1], 0x50);
        assert_eq!(buf[2], 0x91);
        assert_eq!(buf[3], 7);
        assert_eq!(buf[4], 0xFE);
        assert_eq!(buf[9], 48);
        assert_eq!(buf[10], 0x2A);
        assert_eq!(buf[11], 0x92);
        assert_eq!(buf[12], 64);
        assert_eq!(&buf[13..77], &haptics);
        let expected_crc = crc32_with_seed(PS_OUTPUT_CRC32_SEED, &buf[..138]);
        assert_eq!(&buf[138..142], &expected_crc.to_le_bytes());
        assert!(buf[142..].iter().all(|&b| b == 0));
    }

    #[test]
    fn encode_bt_speaker_35_routes_and_padding() {
        let opus = [0x5Au8; 200];
        let mut buf = [0xFFu8; 547];
        encode_bt_speaker_report_35(
            0x0B,
            0x7F,
            48,
            true,
            BtAudioRoute::Speaker,
            &opus,
            &mut buf,
        )
        .unwrap();
        assert_eq!(buf[0], 0x35);
        assert_eq!(buf[1], 0xB0);
        assert_eq!(buf[2], 0x91);
        assert_eq!(buf[3], 7);
        assert_eq!(buf[4], 0xFF);
        assert_eq!(buf[9], 48);
        assert_eq!(buf[10], 0x7F);
        assert_eq!(buf[11], 0x93);
        assert_eq!(buf[12], 200);
        assert_eq!(&buf[13..213], &opus);
        let expected_crc = crc32_with_seed(PS_OUTPUT_CRC32_SEED, &buf[..330]);
        assert_eq!(&buf[330..334], &expected_crc.to_le_bytes());
        assert!(buf[334..].iter().all(|&b| b == 0));

        encode_bt_speaker_report_35(
            0x01,
            0x02,
            48,
            false,
            BtAudioRoute::Headphones,
            &opus,
            &mut buf,
        )
        .unwrap();
        assert_eq!(buf[4], 0xFE);
        assert_eq!(buf[11], 0x96);
    }

    #[test]
    fn encode_bt_combined_35_layout() {
        let haptics = [0x11u8; 64];
        let opus = [0x22u8; 200];
        let mut buf = [0u8; 547];
        encode_bt_combined_report_35(
            2,
            9,
            48,
            true,
            BtAudioRoute::Both,
            &haptics,
            &opus,
            &mut buf,
        )
        .unwrap();
        assert_eq!(buf[0], 0x35);
        assert_eq!(buf[1], 0x20);
        assert_eq!(buf[4], 0xFF);
        assert_eq!(buf[11], 0x92);
        assert_eq!(buf[12], 64);
        assert_eq!(&buf[13..77], &haptics);
        assert_eq!(buf[77], 0x96);
        assert_eq!(buf[78], 200);
        assert_eq!(&buf[79..279], &opus);
        let expected_crc = crc32_with_seed(PS_OUTPUT_CRC32_SEED, &buf[..330]);
        assert_eq!(&buf[330..334], &expected_crc.to_le_bytes());
    }

    #[test]
    fn mic_toggle_and_extract_bt_mic_opus() {
        let mut toggle = [0xAAu8; 547];
        encode_bt_mic_toggle_report_32(7, 48, true, &mut toggle).unwrap();
        assert_eq!(toggle[0], 0x32);
        assert_eq!(toggle[1], 0x00);
        assert_eq!(toggle[4], 0xFF);
        assert_eq!(toggle[11], 0x92);
        assert_eq!(toggle[12], 64);
        assert!(toggle[13..77].iter().all(|&b| b == 0));
        let crc = crc32_with_seed(PS_OUTPUT_CRC32_SEED, &toggle[..138]);
        assert_eq!(&toggle[138..142], &crc.to_le_bytes());

        let mut report = [0u8; 78];
        report[0] = DS_INPUT_REPORT_BT;
        report[1] = 0x02;
        for (i, b) in report[3..74].iter_mut().enumerate() {
            *b = (i as u8).wrapping_add(1);
        }
        let in_crc = crc32_with_seed(PS_INPUT_CRC32_SEED, &report[..74]);
        report[74..78].copy_from_slice(&in_crc.to_le_bytes());

        let extracted = extract_bt_mic_opus(&report).unwrap();
        assert_eq!(&extracted[..], &report[3..74]);

        // Normal state report (bit 1 clear) must be ignored.
        let mut state_only = report;
        state_only[1] = 0x01;
        let state_crc = crc32_with_seed(PS_INPUT_CRC32_SEED, &state_only[..74]);
        state_only[74..78].copy_from_slice(&state_crc.to_le_bytes());
        assert!(extract_bt_mic_opus(&state_only).is_none());

        // Bad CRC must be rejected.
        let mut bad_crc = report;
        bad_crc[74] ^= 0xFF;
        assert!(extract_bt_mic_opus(&bad_crc).is_none());
    }

    #[test]
    fn dsp_helpers_fold_compress_and_decimate() {
        let mut stereo = [0.5_f32, -0.25, 1.0, 1.5];
        fold_stereo_to_mono_in_place(&mut stereo);
        assert!((stereo[0] - 0.125).abs() < 1e-6);
        assert!((stereo[1] - 0.125).abs() < 1e-6);
        assert!((stereo[2] - 1.0).abs() < 1e-6);
        assert!((stereo[3] - 1.0).abs() < 1e-6);

        let mut src = [0.0_f32; 512 * 2];
        for i in 0..512 {
            src[i * 2] = (i as f32) / 511.0;
            src[i * 2 + 1] = -(i as f32) / 511.0;
        }
        let mut dst = [0.0_f32; 480 * 2];
        time_compress_512_to_480(&src, &mut dst);
        assert!(dst[0].abs() < 1e-5);
        assert!(dst[479 * 2] > 0.99);

        let mut haptics_in = [0.0_f32; 512 * 2];
        for i in 0..16 {
            haptics_in[i * 2] = 0.5;
            haptics_in[i * 2 + 1] = -0.5;
        }
        let mut out_64 = [0u8; 64];
        assert!(decimate_haptics_48k_to_3k(&haptics_in, 1.0, &mut out_64));
        assert_eq!(out_64[0] as i8, 64);
        assert_eq!(out_64[1] as i8, -64);
        assert_eq!(out_64[2], 0);
    }
}
