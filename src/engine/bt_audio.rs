use std::collections::VecDeque;
use std::f32::consts::{FRAC_1_SQRT_2, PI};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use rusty_opus::{Application, OpusDecoder, OpusEncoder};
use tracing::{debug, info, warn};
use windows::Win32::Foundation::{CloseHandle, HANDLE, S_FALSE, S_OK};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
    IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX,
    eConsole, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize,
};
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, SetWaitableTimerEx,
    TIMER_ALL_ACCESS, WaitForSingleObject,
};
use windows::core::GUID;

use super::profile::ControllerProfile;
use crate::emu::audio_device::SharedAudioBridge;
use crate::hid::DualSenseWriteHandle;
use crate::protocol::constants::{DS_BT_AUDIO_BUFFER_LEN_DEFAULT, DS_BT_AUDIO_OPUS_BYTES};
use crate::protocol::{
    decimate_haptics_48k_to_3k, encode_bt_combined_report_35, encode_bt_haptics_report_32,
    encode_bt_mic_toggle_report_32, encode_bt_speaker_report_35, fold_stereo_to_mono_in_place,
    time_compress_512_to_480,
};

const TARGET_RATE: u32 = 48_000;
const TICK_FRAMES: usize = 512; // 10.667 ms at 48 kHz
const OPUS_FRAME_SAMPLES: usize = 480; // 10.0 ms at 48 kHz
const TICK_INTERVAL_US: u64 = 10_667;
const SILENCE_HANGOVER: Duration = Duration::from_secs(2);
const RING_CAP_FRAMES: usize = 9600; // 200 ms at 48 kHz
const RING_LAG_LIMIT_FRAMES: usize = 2400; // 50 ms max cushion
const RING_LAG_TRIM_TARGET: usize = 1440; // 30 ms target after trim
const WAVE_FORMAT_PCM: u16 = 0x0001;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

const KSDATAFORMAT_SUBTYPE_PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);
const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: GUID =
    GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

#[derive(Default)]
pub struct BtAudioTestSignal {
    speaker_seq: AtomicU32,
    haptics_seq: AtomicU32,
}

impl BtAudioTestSignal {
    pub fn trigger_speaker(&self) {
        self.speaker_seq.fetch_add(1, Ordering::Release);
    }

    pub fn trigger_haptics(&self) {
        self.haptics_seq.fetch_add(1, Ordering::Release);
    }

    #[inline]
    fn speaker_seq(&self) -> u32 {
        self.speaker_seq.load(Ordering::Acquire)
    }

    #[inline]
    fn haptics_seq(&self) -> u32 {
        self.haptics_seq.load(Ordering::Acquire)
    }
}

#[derive(Default)]
pub struct BtAudioTelemetry {
    pub speaker_streaming: AtomicBool,
    pub haptics_streaming: AtomicBool,
    pub mic_streaming: AtomicBool,
    pub speaker_test_active: AtomicBool,
    pub haptics_test_active: AtomicBool,
    pub speaker_level: AtomicU8,
    pub haptics_level: AtomicU8,
    pub mic_level: AtomicU8,
}

/// 2nd-order Butterworth low-pass biquad filter for stereo 48 kHz audio (Direct Form II transposed).
pub struct StereoBiquadLpf {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: [f32; 2],
    z2: [f32; 2],
}

impl StereoBiquadLpf {
    pub fn new(sample_rate: f32, cutoff_hz: f32) -> Self {
        let w0 = 2.0 * PI * (cutoff_hz / sample_rate);
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        Self {
            b0: ((1.0 - cos_w0) * 0.5) / a0,
            b1: (1.0 - cos_w0) / a0,
            b2: ((1.0 - cos_w0) * 0.5) / a0,
            a1: (-2.0 * cos_w0) / a0,
            a2: (1.0 - alpha) / a0,
            z1: [0.0; 2],
            z2: [0.0; 2],
        }
    }

    #[inline]
    pub fn process_in_place(&mut self, interleaved_stereo: &mut [f32]) {
        for pair in interleaved_stereo.chunks_exact_mut(2) {
            for ch in 0..2 {
                let x = pair[ch];
                let y = self.b0 * x + self.z1[ch];
                self.z1[ch] = self.b1 * x - self.a1 * y + self.z2[ch];
                self.z2[ch] = self.b2 * x - self.a2 * y;
                pair[ch] = y;
            }
        }
    }
}

struct SpeakerTestChime {
    pos: usize,
    total_frames: usize,
}

impl SpeakerTestChime {
    fn new() -> Self {
        // 450 ms dual-note chime at 48 kHz = 21,600 frames.
        Self {
            pos: 0,
            total_frames: (TARGET_RATE as usize * 45) / 100,
        }
    }

    fn is_active(&self) -> bool {
        self.pos < self.total_frames
    }

    fn mix_into(&mut self, out_512: &mut [f32; TICK_FRAMES * 2]) -> bool {
        if !self.is_active() {
            return false;
        }
        let note_split = (TARGET_RATE as usize * 20) / 100; // 200 ms C5, 250 ms E5
        let fade_len = (TARGET_RATE as usize * 15) / 1000; // 15 ms fade
        for i in 0..TICK_FRAMES {
            if self.pos >= self.total_frames {
                break;
            }
            let freq = if self.pos < note_split { 523.25 } else { 659.25 };
            let local_pos = if self.pos < note_split {
                self.pos
            } else {
                self.pos - note_split
            };
            let note_len = if self.pos < note_split {
                note_split
            } else {
                self.total_frames - note_split
            };
            let env_in = (local_pos as f32 / fade_len as f32).min(1.0);
            let env_out = ((note_len.saturating_sub(local_pos)) as f32 / fade_len as f32).min(1.0);
            let t = (self.pos as f32) / (TARGET_RATE as f32);
            let sample = (2.0 * PI * freq * t).sin() * 0.35 * env_in * env_out;
            out_512[i * 2] = (out_512[i * 2] + sample).clamp(-1.0, 1.0);
            out_512[i * 2 + 1] = (out_512[i * 2 + 1] + sample).clamp(-1.0, 1.0);
            self.pos += 1;
        }
        true
    }
}

struct HapticsTestPulse {
    pos: usize,
    total_frames: usize,
}

impl HapticsTestPulse {
    fn new() -> Self {
        // 500 ms stereo pulse: 220 ms left (75 Hz), 60 ms gap, 220 ms right (130 Hz).
        Self {
            pos: 0,
            total_frames: TARGET_RATE as usize / 2,
        }
    }

    fn is_active(&self) -> bool {
        self.pos < self.total_frames
    }

    fn mix_into(&mut self, out_512: &mut [f32; TICK_FRAMES * 2]) -> bool {
        if !self.is_active() {
            return false;
        }
        let pulse_frames = (TARGET_RATE as usize * 22) / 100;
        let gap_end = (TARGET_RATE as usize * 28) / 100;
        for i in 0..TICK_FRAMES {
            if self.pos >= self.total_frames {
                break;
            }
            let t = (self.pos as f32) / (TARGET_RATE as f32);
            if self.pos < pulse_frames {
                let phase = (self.pos as f32) / (pulse_frames as f32);
                let env = (PI * phase).sin();
                let s = (2.0 * PI * 75.0 * t).sin() * 0.85 * env;
                out_512[i * 2] = (out_512[i * 2] + s).clamp(-1.0, 1.0);
            } else if self.pos >= gap_end {
                let local = self.pos - gap_end;
                let phase = (local as f32) / (pulse_frames as f32).max(1.0);
                let env = (PI * phase.min(1.0)).sin();
                let s = (2.0 * PI * 130.0 * t).sin() * 0.85 * env;
                out_512[i * 2 + 1] = (out_512[i * 2 + 1] + s).clamp(-1.0, 1.0);
            }
            self.pos += 1;
        }
        true
    }
}

struct StereoRing {
    buf: VecDeque<(f32, f32)>,
}

impl StereoRing {
    fn new(cap: usize) -> Self {
        Self {
            buf: VecDeque::with_capacity(cap),
        }
    }

    fn push_frame(&mut self, l: f32, r: f32) {
        if self.buf.len() >= RING_CAP_FRAMES {
            self.buf.pop_front();
        }
        self.buf.push_back((l, r));
    }

    fn trim_if_lagging(&mut self) {
        if self.buf.len() > RING_LAG_LIMIT_FRAMES {
            let drop_count = self.buf.len() - RING_LAG_TRIM_TARGET;
            self.buf.drain(..drop_count);
        }
    }

    fn read_512(&mut self, out: &mut [f32; TICK_FRAMES * 2]) -> bool {
        if self.buf.len() < TICK_FRAMES {
            out.fill(0.0);
            return false;
        }
        for i in 0..TICK_FRAMES {
            let (l, r) = self.buf.pop_front().unwrap_or((0.0, 0.0));
            out[i * 2] = l;
            out[i * 2 + 1] = r;
        }
        true
    }
}

#[derive(Clone, Copy)]
enum SampleFormat {
    Float32,
    Int16,
}

struct WasapiLoopback {
    audio_client: IAudioClient,
    capture_client: IAudioCaptureClient,
    src_rate: u32,
    channels: usize,
    format: SampleFormat,
    src_pos: f64,
    prev_sample: (f32, f32),
    scratch_stereo: Vec<(f32, f32)>,
}

impl WasapiLoopback {
    fn open() -> Result<Self, String> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|e| format!("CoCreateInstance(MMDeviceEnumerator): {e}"))?;
            let device = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .map_err(|e| format!("GetDefaultAudioEndpoint: {e}"))?;
            let audio_client: IAudioClient = device
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| format!("Activate(IAudioClient): {e}"))?;
            let pwfx = audio_client
                .GetMixFormat()
                .map_err(|e| format!("GetMixFormat: {e}"))?;

            let parse_res = parse_wave_format(pwfx);
            let init_res = match parse_res {
                Ok((src_rate, channels, format)) => {
                    let hns_buffer = 400_000_i64; // 40 ms
                    audio_client
                        .Initialize(
                            AUDCLNT_SHAREMODE_SHARED,
                            AUDCLNT_STREAMFLAGS_LOOPBACK,
                            hns_buffer,
                            0,
                            pwfx,
                            None,
                        )
                        .map(|()| (src_rate, channels, format))
                        .map_err(|e| format!("IAudioClient::Initialize: {e}"))
                }
                Err(e) => Err(e),
            };
            CoTaskMemFree(Some(pwfx as *const _));
            let (src_rate, channels, format) = init_res?;

            let capture_client: IAudioCaptureClient = audio_client
                .GetService()
                .map_err(|e| format!("GetService(IAudioCaptureClient): {e}"))?;
            audio_client
                .Start()
                .map_err(|e| format!("IAudioClient::Start: {e}"))?;

            Ok(Self {
                audio_client,
                capture_client,
                src_rate,
                channels,
                format,
                src_pos: 0.0,
                prev_sample: (0.0, 0.0),
                scratch_stereo: Vec::with_capacity(2048),
            })
        }
    }

    fn drain_to_48k_stereo(&mut self, ring: &mut StereoRing) -> Result<(), String> {
        loop {
            let packet_frames = unsafe {
                self.capture_client
                    .GetNextPacketSize()
                    .map_err(|e| format!("GetNextPacketSize: {e}"))?
            };
            if packet_frames == 0 {
                break;
            }

            let mut data_ptr = std::ptr::null_mut();
            let mut frames_avail = 0u32;
            let mut flags = 0u32;
            unsafe {
                self.capture_client
                    .GetBuffer(&mut data_ptr, &mut frames_avail, &mut flags, None, None)
                    .map_err(|e| format!("GetBuffer: {e}"))?;
            }

            let frame_count = frames_avail as usize;
            self.scratch_stereo.clear();
            let is_silent = (flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)) != 0;
            if is_silent || data_ptr.is_null() {
                self.scratch_stereo.resize(frame_count, (0.0, 0.0));
            } else {
                match self.format {
                    SampleFormat::Float32 => {
                        let total = frame_count * self.channels;
                        let slice =
                            unsafe { std::slice::from_raw_parts(data_ptr as *const f32, total) };
                        for frame in slice.chunks_exact(self.channels) {
                            let (l, r) = match self.channels {
                                1 => (frame[0], frame[0]),
                                _ => (frame[0], frame[1]),
                            };
                            self.scratch_stereo.push((l, r));
                        }
                    }
                    SampleFormat::Int16 => {
                        let total = frame_count * self.channels;
                        let slice =
                            unsafe { std::slice::from_raw_parts(data_ptr as *const i16, total) };
                        for frame in slice.chunks_exact(self.channels) {
                            let l = (frame[0] as f32) / 32768.0;
                            let r = if self.channels == 1 {
                                l
                            } else {
                                (frame[1] as f32) / 32768.0
                            };
                            self.scratch_stereo.push((l, r));
                        }
                    }
                }
            }

            unsafe {
                let _ = self.capture_client.ReleaseBuffer(frames_avail);
            }

            if self.src_rate == TARGET_RATE {
                for &(l, r) in &self.scratch_stereo {
                    ring.push_frame(l, r);
                }
            } else if !self.scratch_stereo.is_empty() {
                let step = (self.src_rate as f64) / (TARGET_RATE as f64);
                let len = self.scratch_stereo.len();
                while self.src_pos < len as f64 {
                    let idx = self.src_pos as usize;
                    let frac = (self.src_pos - idx as f64) as f32;
                    let s0 = if idx == 0 {
                        self.prev_sample
                    } else {
                        self.scratch_stereo[idx - 1]
                    };
                    let s1 = self.scratch_stereo[idx];
                    let l = s0.0 + (s1.0 - s0.0) * frac;
                    let r = s0.1 + (s1.1 - s0.1) * frac;
                    ring.push_frame(l, r);
                    self.src_pos += step;
                }
                self.src_pos -= len as f64;
                self.prev_sample = *self.scratch_stereo.last().unwrap_or(&(0.0, 0.0));
            }
        }
        ring.trim_if_lagging();
        Ok(())
    }
}

impl Drop for WasapiLoopback {
    fn drop(&mut self) {
        unsafe {
            let _ = self.audio_client.Stop();
        }
    }
}

unsafe fn parse_wave_format(pwfx: *const WAVEFORMATEX) -> Result<(u32, usize, SampleFormat), String> {
    if pwfx.is_null() {
        return Err("GetMixFormat returned null".to_owned());
    }
    let wfx = unsafe { *pwfx };
    let channels = wfx.nChannels as usize;
    let rate = wfx.nSamplesPerSec;
    let tag = wfx.wFormatTag;
    let bits = wfx.wBitsPerSample;

    if channels == 0 || rate == 0 {
        return Err("Invalid WASAPI mix format (0 channels or rate)".to_owned());
    }

    let fmt = if tag == WAVE_FORMAT_IEEE_FLOAT && bits == 32 {
        SampleFormat::Float32
    } else if tag == WAVE_FORMAT_PCM && bits == 16 {
        SampleFormat::Int16
    } else if tag == WAVE_FORMAT_EXTENSIBLE && wfx.cbSize >= 22 {
        let sub_ptr = unsafe { (pwfx as *const u8).add(24) as *const GUID };
        let sub = unsafe { std::ptr::read_unaligned(sub_ptr) };
        if sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT && bits == 32 {
            SampleFormat::Float32
        } else if sub == KSDATAFORMAT_SUBTYPE_PCM && bits == 16 {
            SampleFormat::Int16
        } else {
            return Err(format!("Unsupported WASAPI extensible format ({bits}-bit)"));
        }
    } else {
        return Err(format!("Unsupported WASAPI format tag 0x{tag:04X} ({bits}-bit)"));
    };

    Ok((rate, channels, fmt))
}

struct HighResTimer(HANDLE);

impl HighResTimer {
    fn new() -> Option<Self> {
        let h = unsafe {
            CreateWaitableTimerExW(
                None,
                None,
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
        }
        .ok()?;
        Some(Self(h))
    }

    fn wait_us(&self, us: u64) {
        let due_100ns = -((us as i64) * 10);
        unsafe {
            if SetWaitableTimerEx(self.0, &due_100ns, 0, None, None, None, 0).is_ok() {
                let _ = WaitForSingleObject(self.0, 100);
            } else {
                thread::sleep(Duration::from_micros(us));
            }
        }
    }
}

impl Drop for HighResTimer {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub struct BtAudioWorkerHandle {
    pub telemetry: Arc<BtAudioTelemetry>,
    stop_flag: Arc<AtomicBool>,
    join_handle: Option<JoinHandle<()>>,
}

impl BtAudioWorkerHandle {
    pub fn spawn(
        device_path: String,
        output_report_len: usize,
        profile: Arc<ArcSwap<ControllerProfile>>,
        test_signal: Arc<BtAudioTestSignal>,
        bridge: Arc<SharedAudioBridge>,
        cloak_enabled: Arc<AtomicBool>,
    ) -> Self {
        let telemetry = Arc::new(BtAudioTelemetry::default());
        let stop_flag = Arc::new(AtomicBool::new(false));
        let tel_c = Arc::clone(&telemetry);
        let stop_c = Arc::clone(&stop_flag);

        let join_handle = thread::Builder::new()
            .name("ds-bt-audio".to_owned())
            .spawn(move || {
                bt_audio_worker_loop(
                    device_path,
                    output_report_len,
                    profile,
                    test_signal,
                    bridge,
                    cloak_enabled,
                    tel_c,
                    stop_c,
                );
            })
            .ok();

        Self {
            telemetry,
            stop_flag,
            join_handle,
        }
    }

    pub fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::Release);
        if let Some(h) = self.join_handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for BtAudioWorkerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

fn create_ds5_opus_encoder() -> Result<OpusEncoder, String> {
    let mut enc = OpusEncoder::new(TARGET_RATE as i32, 2, Application::Audio)
        .map_err(|e| format!("OpusEncoder::new: {e:?}"))?;
    enc.bitrate_bps = 156_000;
    enc.use_cbr = true;
    enc.complexity = 5;
    Ok(enc)
}

#[inline]
fn peak_u8(buf: &[f32]) -> u8 {
    let mut peak = 0.0_f32;
    for &s in buf {
        let a = s.abs();
        if a > peak {
            peak = a;
        }
    }
    (peak.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn bt_audio_worker_loop(
    device_path: String,
    output_report_len: usize,
    profile_swap: Arc<ArcSwap<ControllerProfile>>,
    test_signal: Arc<BtAudioTestSignal>,
    bridge: Arc<SharedAudioBridge>,
    cloak_enabled: Arc<AtomicBool>,
    telemetry: Arc<BtAudioTelemetry>,
    stop_flag: Arc<AtomicBool>,
) {
    let com_hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let com_owned = com_hr == S_OK || com_hr == S_FALSE;

    let mut write_handle = match DualSenseWriteHandle::open(&device_path, output_report_len) {
        Ok(h) => h,
        Err(e) => {
            warn!(error = %e, "Failed to open DualSense write handle for BT audio");
            if com_owned {
                unsafe { CoUninitialize() };
            }
            return;
        }
    };

    let mut opus_enc = match create_ds5_opus_encoder() {
        Ok(e) => e,
        Err(e) => {
            warn!(error = %e, "Failed to initialize Opus encoder");
            if com_owned {
                unsafe { CoUninitialize() };
            }
            return;
        }
    };

    // DualSense BT mic is mono 48 kHz 10 ms Opus (480 samples/frame).
    let mut mic_dec = match OpusDecoder::new(TARGET_RATE as i32, 1) {
        Ok(d) => d,
        Err(e) => {
            warn!(error = ?e, "Failed to initialize mono Opus decoder for BT mic");
            if com_owned {
                unsafe { CoUninitialize() };
            }
            return;
        }
    };

    let timer = HighResTimer::new();
    let mut wasapi: Option<WasapiLoopback> = None;
    let mut last_wasapi_retry = Instant::now() - Duration::from_secs(5);
    let mut ring = StereoRing::new(RING_CAP_FRAMES);
    let mut haptics_lpf = StereoBiquadLpf::new(TARGET_RATE as f32, 220.0);

    let mut last_spk_test_seq = test_signal.speaker_seq();
    let mut last_hap_test_seq = test_signal.haptics_seq();
    let mut spk_chime: Option<SpeakerTestChime> = None;
    let mut hap_pulse: Option<HapticsTestPulse> = None;

    let mut bt_seq: u8 = 0;
    let mut pkt_counter: u8 = 0;
    let mut last_spk_audible: Option<Instant> = None;
    let mut last_hap_audible: Option<Instant> = None;
    let mut last_mic_rx: Option<Instant> = None;
    let mut last_mic_toggle_sent: Option<Instant> = None;
    let mut mic_session_open = false;
    let mut write_failed_logged = false;

    let mut pull_512 = [0.0_f32; TICK_FRAMES * 2];
    let mut virt_spk_512 = [0.0_f32; TICK_FRAMES * 2];
    let mut spk_512 = [0.0_f32; TICK_FRAMES * 2];
    let mut hap_512 = [0.0_f32; TICK_FRAMES * 2];
    let mut spk_480 = [0.0_f32; OPUS_FRAME_SAMPLES * 2];
    let mut opus_out = [0u8; DS_BT_AUDIO_OPUS_BYTES + 16];
    let mut haptics_64 = [0u8; 64];
    let mut mic_pcm_480 = [0.0_f32; OPUS_FRAME_SAMPLES];

    let tick_dur = Duration::from_micros(TICK_INTERVAL_US);
    let mut next_tick = Instant::now() + tick_dur;

    info!("Bluetooth audio/haptics worker started");

    while !stop_flag.load(Ordering::Acquire) {
        let prof = profile_swap.load();

        let cur_spk_seq = test_signal.speaker_seq();
        if cur_spk_seq != last_spk_test_seq {
            last_spk_test_seq = cur_spk_seq;
            spk_chime = Some(SpeakerTestChime::new());
        }
        let cur_hap_seq = test_signal.haptics_seq();
        if cur_hap_seq != last_hap_test_seq {
            last_hap_test_seq = cur_hap_seq;
            hap_pulse = Some(HapticsTestPulse::new());
        }

        let spk_test_on = spk_chime.as_ref().is_some_and(SpeakerTestChime::is_active);
        let hap_test_on = hap_pulse.as_ref().is_some_and(HapticsTestPulse::is_active);
        telemetry
            .speaker_test_active
            .store(spk_test_on, Ordering::Release);
        telemetry
            .haptics_test_active
            .store(hap_test_on, Ordering::Release);

        let want_mic = prof.virtual_audio_device
            && bridge.mic_endpoint_open.load(Ordering::Acquire)
            && cloak_enabled.load(Ordering::Acquire);

        // Drain incoming BT microphone Opus packets regardless of speaker/haptics idle state.
        let mut mic_peak = 0u8;
        while let Ok(pkt) = bridge.mic_opus_rx.try_recv() {
            match mic_dec.decode(&pkt, OPUS_FRAME_SAMPLES, &mut mic_pcm_480) {
                Ok(n) if n > 0 => {
                    let gain = prof.bt_mic_gain.clamp(0.0, 2.0);
                    for s in &mut mic_pcm_480[..n] {
                        *s = (*s * gain).clamp(-1.0, 1.0);
                    }
                    mic_peak = mic_peak.max(peak_u8(&mic_pcm_480[..n]));
                    last_mic_rx = Some(Instant::now());
                    if want_mic {
                        bridge.push_mic_mono_f32(&mic_pcm_480[..n]);
                    }
                }
                _ => {}
            }
        }
        let mic_live = want_mic
            && last_mic_rx.is_some_and(|t| t.elapsed() <= Duration::from_millis(500));
        telemetry.mic_streaming.store(mic_live, Ordering::Release);
        if mic_peak > 0 {
            telemetry.mic_level.store(mic_peak, Ordering::Release);
        } else if !mic_live {
            telemetry.mic_level.store(0, Ordering::Release);
        }

        let want_wasapi =
            prof.bt_haptics_enabled && !bridge.speaker_endpoint_open.load(Ordering::Acquire);
        let want_virt_spk = prof.virtual_audio_device;
        if !want_wasapi && !want_virt_spk && !spk_test_on && !hap_test_on {
            if wasapi.is_some() {
                wasapi = None;
                ring.buf.clear();
            }
            telemetry.speaker_streaming.store(false, Ordering::Release);
            telemetry.haptics_streaming.store(false, Ordering::Release);
            telemetry.speaker_level.store(0, Ordering::Release);
            telemetry.haptics_level.store(0, Ordering::Release);

            // Manage standalone BT mic open/close toggle when no audio/haptics stream is running.
            let need_open_toggle = want_mic
                && (!mic_session_open
                    || (!mic_live
                        && last_mic_toggle_sent
                            .is_none_or(|t| t.elapsed() >= Duration::from_millis(500))));
            let need_close_toggle = !want_mic && mic_session_open;
            if need_open_toggle || need_close_toggle {
                if encode_bt_mic_toggle_report_32(
                    pkt_counter,
                    DS_BT_AUDIO_BUFFER_LEN_DEFAULT,
                    want_mic,
                    write_handle.write_buf_mut(),
                )
                .is_ok()
                {
                    pkt_counter = pkt_counter.wrapping_add(1);
                    let _ = write_handle.write_report(50);
                    mic_session_open = want_mic;
                    last_mic_toggle_sent = Some(Instant::now());
                }
            }

            thread::sleep(Duration::from_millis(if want_mic { 5 } else { 30 }));
            next_tick = Instant::now() + tick_dur;
            continue;
        }

        if want_wasapi {
            if wasapi.is_none() && last_wasapi_retry.elapsed() >= Duration::from_secs(2) {
                last_wasapi_retry = Instant::now();
                match WasapiLoopback::open() {
                    Ok(w) => {
                        debug!(rate = w.src_rate, ch = w.channels, "Opened WASAPI loopback");
                        wasapi = Some(w);
                    }
                    Err(e) => {
                        debug!(error = %e, "WASAPI loopback open failed; will retry");
                    }
                }
            }
            if let Some(w) = wasapi.as_mut() {
                if let Err(e) = w.drain_to_48k_stereo(&mut ring) {
                    debug!(error = %e, "WASAPI loopback invalidated; resetting");
                    wasapi = None;
                    ring.buf.clear();
                }
            }
            let _ = ring.read_512(&mut pull_512);
        } else {
            wasapi = None;
            ring.buf.clear();
            pull_512.fill(0.0);
        }

        if want_virt_spk {
            let _ = bridge.pull_speaker_512(&mut virt_spk_512);
        } else {
            virt_spk_512.fill(0.0);
        }

        // Speaker path: virtual USB audio device (when enabled) + test chime
        let spk_vol = prof.bt_speaker_volume.clamp(0.0, 1.0);
        if want_virt_spk {
            for i in 0..TICK_FRAMES * 2 {
                spk_512[i] = (virt_spk_512[i] * spk_vol).clamp(-1.0, 1.0);
            }
        } else {
            spk_512.fill(0.0);
        }
        let chime_mixed = spk_chime.as_mut().is_some_and(|c| c.mix_into(&mut spk_512));
        if !chime_mixed {
            spk_chime = None;
        }
        let spk_peak = peak_u8(&spk_512);
        let now = Instant::now();
        if spk_peak > 1 {
            last_spk_audible = Some(now);
        }
        let send_speaker = (want_virt_spk || chime_mixed)
            && last_spk_audible.is_some_and(|t| now.duration_since(t) <= SILENCE_HANGOVER);

        // Haptics path (LPF on virtual speaker or WASAPI loopback, crisp direct mix for test pulse)
        if prof.bt_haptics_enabled {
            let src_slice = if want_virt_spk && bridge.speaker_endpoint_open.load(Ordering::Acquire) {
                &virt_spk_512
            } else {
                &pull_512
            };
            hap_512.copy_from_slice(src_slice);
            haptics_lpf.process_in_place(&mut hap_512);
            for s in &mut hap_512 {
                if s.abs() < 0.004 {
                    *s = 0.0;
                }
            }
        } else {
            hap_512.fill(0.0);
        }
        let pulse_mixed = hap_pulse.as_mut().is_some_and(|p| p.mix_into(&mut hap_512));
        if !pulse_mixed {
            hap_pulse = None;
        }

        let hap_gain = if pulse_mixed && !prof.bt_haptics_enabled {
            1.0
        } else {
            prof.bt_haptics_gain.clamp(0.0, 1.0)
        };
        let hap_signal = decimate_haptics_48k_to_3k(&hap_512, hap_gain, &mut haptics_64);
        if hap_signal {
            last_hap_audible = Some(now);
        }
        let send_haptics = (prof.bt_haptics_enabled || pulse_mixed)
            && last_hap_audible.is_some_and(|t| now.duration_since(t) <= SILENCE_HANGOVER);

        let hap_peak = if send_haptics {
            let mut max_v = 0u8;
            for &b in &haptics_64 {
                let a = (b as i8).unsigned_abs().saturating_mul(2);
                if a > max_v {
                    max_v = a;
                }
            }
            max_v
        } else {
            0
        };

        telemetry
            .speaker_streaming
            .store(send_speaker, Ordering::Release);
        telemetry
            .haptics_streaming
            .store(send_haptics, Ordering::Release);
        telemetry
            .speaker_level
            .store(if send_speaker { spk_peak } else { 0 }, Ordering::Release);
        telemetry.haptics_level.store(hap_peak, Ordering::Release);

        if send_speaker || send_haptics {
            let mut opus_len = 0usize;
            if send_speaker {
                if prof.bt_audio_route.needs_mono_fold() {
                    fold_stereo_to_mono_in_place(&mut spk_512);
                }
                time_compress_512_to_480(&spk_512, &mut spk_480);
                match opus_enc.encode(&spk_480, OPUS_FRAME_SAMPLES, &mut opus_out) {
                    Ok(n) => opus_len = n.min(DS_BT_AUDIO_OPUS_BYTES),
                    Err(e) => {
                        debug!(error = ?e, "Opus encode failed");
                        opus_len = 0;
                    }
                }
            }

            let encoded_ok = if send_speaker && send_haptics && opus_len > 0 {
                encode_bt_combined_report_35(
                    bt_seq,
                    pkt_counter,
                    DS_BT_AUDIO_BUFFER_LEN_DEFAULT,
                    want_mic,
                    prof.bt_audio_route,
                    &haptics_64,
                    &opus_out[..opus_len],
                    write_handle.write_buf_mut(),
                )
                .is_ok()
            } else if send_speaker && opus_len > 0 {
                encode_bt_speaker_report_35(
                    bt_seq,
                    pkt_counter,
                    DS_BT_AUDIO_BUFFER_LEN_DEFAULT,
                    want_mic,
                    prof.bt_audio_route,
                    &opus_out[..opus_len],
                    write_handle.write_buf_mut(),
                )
                .is_ok()
            } else if send_haptics {
                encode_bt_haptics_report_32(
                    bt_seq,
                    pkt_counter,
                    DS_BT_AUDIO_BUFFER_LEN_DEFAULT,
                    want_mic,
                    &haptics_64,
                    write_handle.write_buf_mut(),
                )
                .is_ok()
            } else if want_mic && !mic_session_open {
                encode_bt_mic_toggle_report_32(
                    pkt_counter,
                    DS_BT_AUDIO_BUFFER_LEN_DEFAULT,
                    true,
                    write_handle.write_buf_mut(),
                )
                .is_ok()
            } else if !want_mic && mic_session_open {
                encode_bt_mic_toggle_report_32(
                    pkt_counter,
                    DS_BT_AUDIO_BUFFER_LEN_DEFAULT,
                    false,
                    write_handle.write_buf_mut(),
                )
                .is_ok()
            } else {
                false
            };

            if encoded_ok {
                bt_seq = (bt_seq + 1) & 0x0F;
                pkt_counter = pkt_counter.wrapping_add(1);
                mic_session_open = want_mic;
                last_mic_toggle_sent = Some(Instant::now());
                if let Err(e) = write_handle.write_report(50) {
                    if !write_failed_logged {
                        warn!(error = %e, "DualSense BT audio HID write failed");
                        write_failed_logged = true;
                    }
                    thread::sleep(Duration::from_millis(20));
                } else {
                    write_failed_logged = false;
                }
            }
        } else if (want_mic
            && (!mic_session_open
                || (!mic_live
                    && last_mic_toggle_sent
                        .is_none_or(|t| t.elapsed() >= Duration::from_millis(500)))))
            || (!want_mic && mic_session_open)
        {
            if encode_bt_mic_toggle_report_32(
                pkt_counter,
                DS_BT_AUDIO_BUFFER_LEN_DEFAULT,
                want_mic,
                write_handle.write_buf_mut(),
            )
            .is_ok()
            {
                pkt_counter = pkt_counter.wrapping_add(1);
                mic_session_open = want_mic;
                last_mic_toggle_sent = Some(Instant::now());
                let _ = write_handle.write_report(50);
            }
        }

        // Absolute schedule pacing; never burst catch-up frames.
        let now_after = Instant::now();
        if next_tick <= now_after {
            next_tick = now_after + tick_dur;
        }
        let wait_us = next_tick
            .saturating_duration_since(now_after)
            .as_micros()
            .max(500) as u64;
        if let Some(t) = timer.as_ref() {
            t.wait_us(wait_us);
        } else {
            thread::sleep(Duration::from_micros(wait_us));
        }
        next_tick += tick_dur;
    }

    telemetry.speaker_streaming.store(false, Ordering::Release);
    telemetry.haptics_streaming.store(false, Ordering::Release);
    telemetry.mic_streaming.store(false, Ordering::Release);
    telemetry.speaker_test_active.store(false, Ordering::Release);
    telemetry.haptics_test_active.store(false, Ordering::Release);
    telemetry.speaker_level.store(0, Ordering::Release);
    telemetry.haptics_level.store(0, Ordering::Release);
    telemetry.mic_level.store(0, Ordering::Release);

    if com_owned {
        unsafe { CoUninitialize() };
    }
    info!("Bluetooth audio/haptics worker stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn biquad_lpf_passes_bass_and_attenuates_highs() {
        let mut lpf_low = StereoBiquadLpf::new(48_000.0, 220.0);
        let mut lpf_high = StereoBiquadLpf::new(48_000.0, 220.0);

        let mut low_buf = [0.0_f32; 4800 * 2];
        let mut high_buf = [0.0_f32; 4800 * 2];
        for i in 0..4800 {
            let t = i as f32 / 48_000.0;
            let s_low = (2.0 * PI * 60.0 * t).sin();
            let s_high = (2.0 * PI * 4000.0 * t).sin();
            low_buf[i * 2] = s_low;
            low_buf[i * 2 + 1] = s_low;
            high_buf[i * 2] = s_high;
            high_buf[i * 2 + 1] = s_high;
        }
        lpf_low.process_in_place(&mut low_buf);
        lpf_high.process_in_place(&mut high_buf);

        let low_peak = low_buf[4800..]
            .iter()
            .fold(0.0_f32, |m, &v| m.max(v.abs()));
        let high_peak = high_buf[4800..]
            .iter()
            .fold(0.0_f32, |m, &v| m.max(v.abs()));
        assert!(low_peak > 0.9, "60 Hz should pass cleanly, got {low_peak}");
        assert!(
            high_peak < 0.05,
            "4 kHz should be strongly attenuated, got {high_peak}"
        );
    }

    #[test]
    fn rusty_opus_cbr_48k_10ms_fits_200_byte_slot() {
        let mut enc = create_ds5_opus_encoder().unwrap();
        let mut pcm = [0.0_f32; OPUS_FRAME_SAMPLES * 2];
        for i in 0..OPUS_FRAME_SAMPLES {
            let t = i as f32 / 48_000.0;
            let s = (2.0 * PI * 440.0 * t).sin() * 0.5;
            pcm[i * 2] = s;
            pcm[i * 2 + 1] = s;
        }
        let mut out = [0u8; 256];
        let len = enc.encode(&pcm, OPUS_FRAME_SAMPLES, &mut out).unwrap();
        assert!(
            len > 0 && len <= DS_BT_AUDIO_OPUS_BYTES,
            "Expected 1..=200 byte Opus frame, got {len}"
        );
    }

    #[test]
    fn rusty_opus_mono_decoder_decodes_48k_10ms_frame() {
        let mut enc = OpusEncoder::new(TARGET_RATE as i32, 1, Application::Voip).unwrap();
        enc.bitrate_bps = 56_800;
        enc.use_cbr = true;
        let mut in_pcm = [0.0_f32; OPUS_FRAME_SAMPLES];
        for i in 0..OPUS_FRAME_SAMPLES {
            in_pcm[i] = (2.0 * PI * 440.0 * (i as f32) / 48_000.0).sin() * 0.5;
        }
        let mut pkt = [0u8; 128];
        let n_bytes = enc.encode(&in_pcm, OPUS_FRAME_SAMPLES, &mut pkt).unwrap();
        assert!(n_bytes > 0);

        let mut dec = OpusDecoder::new(TARGET_RATE as i32, 1).unwrap();
        let mut out_pcm = [0.0_f32; OPUS_FRAME_SAMPLES];
        let decoded = dec
            .decode(&pkt[..n_bytes], OPUS_FRAME_SAMPLES, &mut out_pcm)
            .unwrap();
        assert_eq!(decoded, OPUS_FRAME_SAMPLES);
        assert!(peak_u8(&out_pcm) > 50);
    }
}
