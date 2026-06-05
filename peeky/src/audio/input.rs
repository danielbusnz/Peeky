use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

/// Latest RMS of the mic input, stored as `f32::to_bits()` so it can live in
/// an atomic for lock-free reads from the GTK overlay thread. Updated on every
/// cpal callback. Range is 0.0 (silence) to ~1.0 (clipping).
pub static AUDIO_LEVEL: AtomicU32 = AtomicU32::new(0);

use crate::tuning::{AUDIO_POST_RELEASE_GRACE_MS, AUDIO_PREROLL_MS};

/// Briefly open an audio input stream to trigger macOS microphone permission
/// prompt. The stream is opened for ~100ms then dropped. Safe to call even if
/// permission is already granted (just a no-op from the user's perspective).
#[cfg(target_os = "macos")]
pub fn trigger_mic_permission() {
    let host = cpal::default_host();
    let device = match host.default_input_device() {
        Some(d) => d,
        None => {
            eprintln!("[audio] no input device for permission trigger");
            return;
        }
    };
    let config = match device.default_input_config() {
        Ok(c) => c.config(),
        Err(e) => {
            eprintln!(
                "[audio] failed to get input config for permission trigger: {}",
                e
            );
            return;
        }
    };
    let stream = device.build_input_stream(
        &config,
        |_data: &[f32], _: &cpal::InputCallbackInfo| {},
        |err| eprintln!("[audio] permission trigger stream error: {}", err),
        None,
    );
    match stream {
        Ok(s) => {
            if let Err(e) = s.play() {
                eprintln!("[audio] permission trigger play failed: {}", e);
                return;
            }
            thread::sleep(Duration::from_millis(100));
            eprintln!("[startup] microphone permission check triggered");
        }
        Err(e) => {
            eprintln!("[audio] permission trigger stream build failed: {}", e);
        }
    }
}

/// Cold mic handle: device + cached config. Picked at startup on the main
/// thread so device-enumeration errors surface before any hotkey is held.
/// Convert to a `LiveMic` on the voice thread via `Mic::start()` to actually
/// open the cpal stream (cpal::Stream is !Send so it must live on whichever
/// thread owns it).
pub struct Mic {
    device: cpal::Device,
    pub sample_rate: u32,
    pub channels: u16,
}

impl Mic {
    /// Pick the input device and probe its default config. Panics with a
    /// clear message if no mic is available.
    pub fn init() -> Self {
        let device = pick_input_device();
        #[allow(deprecated)]
        let name = device.name().unwrap_or_else(|_| "<unknown>".to_string());
        let supported = device
            .default_input_config()
            .expect("no default input config");
        let sample_rate = supported.sample_rate();
        let channels = supported.channels();
        eprintln!(
            "[audio] mic found: {} ({}Hz, {}ch)",
            name, sample_rate, channels
        );
        Mic {
            sample_rate,
            channels,
            device,
        }
    }

    /// Open the cpal stream and start capturing 24/7. Must be called on the
    /// thread that will own the stream for the rest of the app's life
    /// (cpal::Stream is !Send).
    ///
    /// While no capture is installed, samples from the mic are silently
    /// dropped. Per-turn, `LiveMic::capture_until_release` installs a
    /// sender, forwards samples through it until the hotkey is released,
    /// then drops the sender (closing the downstream channel).
    pub fn start(self) -> LiveMic {
        let supported_config = self
            .device
            .default_input_config()
            .expect("no default input config");
        let sample_format = supported_config.sample_format();
        let config = supported_config.config();

        // Downmix to mono. Deepgram nova-3 (and most STT models) are
        // mono-trained; multi-channel input degrades recognition on weak
        // phonemes. The cpal callback averages interleaved frames into a
        // single channel before quantizing to i16.
        let input_channels = self.channels;
        let output_channels: u16 = 1;

        let preroll_max_samples =
            (self.sample_rate as usize) * (output_channels as usize) * (AUDIO_PREROLL_MS as usize)
                / 1000;
        let state = Arc::new(Mutex::new(MicState {
            current_tx: None,
            preroll: Preroll::new(preroll_max_samples),
        }));

        let err_cb = |err| eprintln!("audio stream error: {}", err);

        // cpal demands a closure typed for the hardware's native sample
        // format. Some devices (Arctis Nova, ALSA hw:) only deliver I16;
        // others deliver F32. Branch and produce a unified i16 PCM stream
        // for Deepgram either way.
        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                let state_cb = state.clone();
                self.device.build_input_stream(
                    &config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        let rms = if data.is_empty() {
                            0.0
                        } else {
                            (data.iter().map(|&s| s * s).sum::<f32>() / data.len() as f32).sqrt()
                        };
                        AUDIO_LEVEL.store(rms.to_bits(), Ordering::Relaxed);

                        let samples: Vec<i16> = if input_channels <= 1 {
                            data.iter()
                                .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                                .collect()
                        } else {
                            data.chunks(input_channels as usize)
                                .map(|frame| {
                                    let avg = frame.iter().sum::<f32>() / frame.len() as f32;
                                    (avg.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
                                })
                                .collect()
                        };
                        route_samples(&state_cb, samples);
                    },
                    err_cb,
                    None,
                )
            }
            cpal::SampleFormat::I16 => {
                let state_cb = state.clone();
                self.device.build_input_stream(
                    &config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        let scale = i16::MAX as f32;
                        let rms = if data.is_empty() {
                            0.0
                        } else {
                            let sum_sq: f32 = data
                                .iter()
                                .map(|&s| {
                                    let f = s as f32 / scale;
                                    f * f
                                })
                                .sum();
                            (sum_sq / data.len() as f32).sqrt()
                        };
                        AUDIO_LEVEL.store(rms.to_bits(), Ordering::Relaxed);

                        let samples: Vec<i16> = if input_channels <= 1 {
                            data.to_vec()
                        } else {
                            data.chunks(input_channels as usize)
                                .map(|frame| {
                                    let sum: i32 = frame.iter().map(|&s| s as i32).sum();
                                    (sum / frame.len() as i32) as i16
                                })
                                .collect()
                        };
                        route_samples(&state_cb, samples);
                    },
                    err_cb,
                    None,
                )
            }
            other => panic!("unsupported cpal sample format: {:?}", other),
        }
        .expect("failed to build input stream");

        stream.play().expect("failed to start stream");
        eprintln!(
            "[audio] cpal stream warmed up, {}ms pre-roll buffer active ({}ch {:?} → 1ch mono i16)",
            AUDIO_PREROLL_MS, input_channels, sample_format
        );

        LiveMic {
            sample_rate: self.sample_rate,
            channels: output_channels,
            state,
            _stream: stream,
        }
    }
}

/// Per-callback routing: forward to the active sender if one is installed,
/// otherwise tail-buffer into the preroll ring. Shared between the F32 and
/// I16 cpal callback variants in `Mic::start`.
fn route_samples(state: &Arc<Mutex<MicState>>, samples: Vec<i16>) {
    let mut s = state.lock().unwrap();
    if let Some(tx) = s.current_tx.as_ref() {
        let _ = tx.send(samples);
    } else {
        s.preroll.push(samples);
    }
}

/// State shared between the cpal audio thread and the voice thread.
/// One Mutex covers both fields so the press-time "flush preroll AND
/// install tx" transition is atomic.
struct MicState {
    current_tx: Option<UnboundedSender<Vec<i16>>>,
    preroll: Preroll,
}

/// Fixed-budget ring buffer of audio chunks, evicting the oldest chunk
/// whenever the total sample count exceeds `max_samples`.
struct Preroll {
    chunks: VecDeque<Vec<i16>>,
    total_samples: usize,
    max_samples: usize,
    warmed_once: bool,
}

impl Preroll {
    fn new(max_samples: usize) -> Self {
        Self {
            chunks: VecDeque::new(),
            total_samples: 0,
            max_samples,
            warmed_once: false,
        }
    }

    fn push(&mut self, samples: Vec<i16>) {
        self.total_samples += samples.len();
        self.chunks.push_back(samples);
        while self.total_samples > self.max_samples {
            match self.chunks.pop_front() {
                Some(removed) => self.total_samples -= removed.len(),
                None => break,
            }
        }
        if !self.warmed_once && self.total_samples >= self.max_samples {
            self.warmed_once = true;
            eprintln!(
                "[audio] preroll filled to capacity ({} samples across {} chunks)",
                self.total_samples,
                self.chunks.len()
            );
        }
    }
}

/// Hot mic: the cpal stream is running 24/7 in the background. Installing
/// a sender via `capture_until_release` makes the callback forward chunks
/// to that sender until the hotkey is released.
pub struct LiveMic {
    pub sample_rate: u32,
    pub channels: u16,
    state: Arc<Mutex<MicState>>,
    _stream: cpal::Stream,
}

impl LiveMic {
    /// Flush the pre-roll buffer into `tx`, install `tx` as the active
    /// forwarding target, wait for the hotkey to release, then drop `tx`
    /// (which closes the downstream channel and triggers Deepgram's
    /// Strategy B return path).
    ///
    /// The flush + install are atomic under the state Mutex so cpal can't
    /// drop a chunk between the two steps.
    pub fn capture_until_release(&self, tx: UnboundedSender<Vec<i16>>) {
        let preroll_chunks;
        let preroll_samples;
        {
            let mut s = self.state.lock().unwrap();
            // Drain pre-roll into a local Vec first so we can release the
            // borrow on `s.preroll` before mutating `s.current_tx`.
            let drained: Vec<Vec<i16>> = s.preroll.chunks.drain(..).collect();
            s.preroll.total_samples = 0;
            preroll_chunks = drained.len();
            preroll_samples = drained.iter().map(|c| c.len()).sum::<usize>();
            for chunk in drained {
                let _ = tx.send(chunk);
            }
            s.current_tx = Some(tx);
        }
        let frames_per_ms = (self.sample_rate as usize) * (self.channels as usize) / 1000;
        let preroll_ms = preroll_samples.checked_div(frames_per_ms).unwrap_or(0);
        eprintln!(
            "[audio] forwarding to STT (flushed {} pre-roll chunks, {} samples, ~{}ms)...",
            preroll_chunks, preroll_samples, preroll_ms
        );
        while crate::hotkey::is_recording() {
            thread::sleep(Duration::from_millis(1));
        }
        // See AUDIO_POST_RELEASE_GRACE_MS docs for the why.
        thread::sleep(Duration::from_millis(AUDIO_POST_RELEASE_GRACE_MS));
        self.state.lock().unwrap().current_tx = None;
        eprintln!(
            "[audio] forwarding stopped ({}ms post-release grace included)",
            AUDIO_POST_RELEASE_GRACE_MS
        );
    }
}

/// Find the input device we want to capture from.
///
/// Selection order:
///   1. `AEGIS_INPUT_DEVICE` env var: case-insensitive substring match
///      against device names. Manual override for edge cases (multi-DEV
///      cards, weird routings).
///   2. Auto-detect via `pactl`: query pipewire/pulse's default source,
///      read its `alsa.id`, then pick the matching `front:CARD=<id>` or
///      `hw:CARD=<id>` cpal device. Skips pipewire's resampling/wrapping
///      and gives you the mic's native format. Auto-follows the user's
///      system default source.
///   3. The "pulse" or "pipewire" virtual device. Always works, but may
///      resample and channel-duplicate.
///   4. cpal's default input.
///
/// Run `cargo run --example list_audio_devices` to see what your system
/// exposes if you need to set the env var manually.
fn pick_input_device() -> cpal::Device {
    let host = cpal::default_host();

    if let Ok(wanted) = std::env::var("AEGIS_INPUT_DEVICE") {
        let needle = wanted.to_lowercase();
        let matched = host
            .input_devices()
            .expect("could not list input devices")
            .find(|d| {
                #[allow(deprecated)]
                d.name()
                    .map(|n| n.to_lowercase().contains(&needle))
                    .unwrap_or(false)
            });
        if let Some(d) = matched {
            #[allow(deprecated)]
            let name = d.name().unwrap_or_else(|_| "<unknown>".into());
            eprintln!("[audio] AEGIS_INPUT_DEVICE='{}' matched {}", wanted, name);
            return d;
        }
        eprintln!(
            "[audio] AEGIS_INPUT_DEVICE='{}' matched nothing, falling back",
            wanted
        );
    }

    if let Some(card_id) = default_source_alsa_card_id() {
        let needle = format!("CARD={}", card_id);
        let matched = host
            .input_devices()
            .expect("could not list input devices")
            .find(|d| {
                #[allow(deprecated)]
                d.name()
                    .map(|n| {
                        (n.starts_with("front:") || n.starts_with("hw:")) && n.contains(&needle)
                    })
                    .unwrap_or(false)
            });
        if let Some(d) = matched {
            #[allow(deprecated)]
            let name = d.name().unwrap_or_else(|_| "<unknown>".into());
            eprintln!(
                "[audio] auto-detected default source (alsa.id={}) → {}",
                card_id, name
            );
            return d;
        }
        eprintln!(
            "[audio] pactl reports alsa.id={} but no matching cpal front:/hw: device, falling back",
            card_id
        );
    }

    let picked = host
        .input_devices()
        .expect("could not list input devices")
        .find(|d| {
            #[allow(deprecated)]
            d.name()
                .map(|n| n == "pulse" || n == "pipewire")
                .unwrap_or(false)
        })
        .or_else(|| host.default_input_device());
    match picked {
        Some(d) => d,
        None => {
            eprintln!("[audio] mic NOT found. no input device available");
            panic!("no input device available");
        }
    }
}

/// Ask pactl for the ALSA card id (e.g. "Wireless") backing pipewire/pulse's
/// current default source. Returns None if pactl is missing, the default
/// source has no ALSA backing (Bluetooth, virtual), or output parsing fails.
fn default_source_alsa_card_id() -> Option<String> {
    let info = Command::new("pactl").arg("info").output().ok()?;
    if !info.status.success() {
        return None;
    }
    let default_source = String::from_utf8_lossy(&info.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("Default Source: ").map(str::to_string))?;

    let sources = Command::new("pactl")
        .args(["list", "sources"])
        .output()
        .ok()?;
    if !sources.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&sources.stdout);

    // Walk source blocks; capture alsa.id from the block whose Name matches.
    let mut in_target_block = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("Source #") {
            in_target_block = false;
            continue;
        }
        if let Some(name) = trimmed.strip_prefix("Name: ") {
            in_target_block = name == default_source;
            continue;
        }
        if in_target_block && let Some(rest) = trimmed.strip_prefix("alsa.id = ") {
            return Some(rest.trim_matches('"').to_string());
        }
    }
    None
}
