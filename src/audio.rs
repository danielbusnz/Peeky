use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

/// How much audio to keep in the pre-roll buffer. The cpal callback fills
/// this ring buffer continuously while idle. On hotkey press, the contents
/// are flushed into the forwarding channel BEFORE live audio starts, so
/// the user's first syllable (typically spoken while still pressing the
/// key) is captured.
const PREROLL_MS: u64 = 300;

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
        let config = supported_config.config();

        // Compute the pre-roll capacity in samples. Sample count includes
        // both channels for interleaved stereo, so the math accounts for
        // channels naturally.
        let preroll_max_samples =
            (self.sample_rate as usize) * (self.channels as usize) * (PREROLL_MS as usize) / 1000;
        let state = Arc::new(Mutex::new(MicState {
            current_tx: None,
            preroll: Preroll::new(preroll_max_samples),
        }));
        let state_cb = state.clone();

        let stream = self
            .device
            .build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    // Convert f32 in [-1.0, 1.0] → i16 PCM for Deepgram.
                    let samples: Vec<i16> = data
                        .iter()
                        .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                        .collect();
                    let mut s = state_cb.lock().unwrap();
                    if let Some(tx) = s.current_tx.as_ref() {
                        // Live forwarding: send straight to the receiver.
                        let _ = tx.send(samples);
                    } else {
                        // Idle: keep the most recent PREROLL_MS in the
                        // ring buffer so it can be flushed on next press.
                        s.preroll.push(samples);
                    }
                },
                move |err| eprintln!("audio stream error: {}", err),
                None,
            )
            .expect("failed to build input stream");
        stream.play().expect("failed to start stream");
        eprintln!(
            "[audio] cpal stream warmed up, {}ms pre-roll buffer active",
            PREROLL_MS
        );

        LiveMic {
            sample_rate: self.sample_rate,
            channels: self.channels,
            state,
            _stream: stream,
        }
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
}

impl Preroll {
    fn new(max_samples: usize) -> Self {
        Self {
            chunks: VecDeque::new(),
            total_samples: 0,
            max_samples,
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
        {
            let mut s = self.state.lock().unwrap();
            // Drain pre-roll into a local Vec first so we can release the
            // borrow on `s.preroll` before mutating `s.current_tx`.
            let drained: Vec<Vec<i16>> = s.preroll.chunks.drain(..).collect();
            s.preroll.total_samples = 0;
            preroll_chunks = drained.len();
            for chunk in drained {
                let _ = tx.send(chunk);
            }
            s.current_tx = Some(tx);
        }
        eprintln!(
            "[audio] forwarding to STT (flushed {} pre-roll chunks)...",
            preroll_chunks
        );
        while crate::hotkey::is_recording() {
            thread::sleep(Duration::from_millis(1));
        }
        // Drop the sender. The cpal callback's next fire will see None
        // and route chunks back to pre-roll; the receiver sees the channel
        // close and Deepgram returns via Strategy B.
        self.state.lock().unwrap().current_tx = None;
        eprintln!("[audio] forwarding stopped");
    }
}

/// The audio output side: holds the rodio DeviceSink open for the lifetime
/// of the app so each turn doesn't pay the ~10-30ms cost of negotiating
/// with the OS audio system. Hand out a fresh `Player` per turn via
/// `new_player()` (cheap; just wires up to the existing sink's mixer).
///
/// Must be initialized on the thread that will own it (rodio's internals
/// may not be Send).
pub struct AudioOutput {
    sink: rodio::MixerDeviceSink,
    pub channels: std::num::NonZeroU16,
    pub sample_rate: std::num::NonZeroU32,
}

impl AudioOutput {
    pub fn init() -> Result<Self, Box<dyn std::error::Error>> {
        let sink = rodio::DeviceSinkBuilder::open_default_sink()
            .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?;
        eprintln!("[audio] output sink opened");
        Ok(AudioOutput {
            sink,
            channels: std::num::NonZeroU16::new(crate::providers::tts_cartesia::STREAM_CHANNELS)
                .expect("STREAM_CHANNELS must be non-zero"),
            sample_rate: std::num::NonZeroU32::new(
                crate::providers::tts_cartesia::STREAM_SAMPLE_RATE,
            )
            .expect("STREAM_SAMPLE_RATE must be non-zero"),
        })
    }

    /// Hand out a fresh Player attached to the cached sink. Cheap.
    pub fn new_player(&self) -> rodio::Player {
        rodio::Player::connect_new(self.sink.mixer())
    }
}

/// Find the input device we want to capture from. Prefers the audio server
/// (pulse/pipewire) over raw hardware devices so we auto-follow the user's
/// active microphone choice. Falls back to cpal's default input.
fn pick_input_device() -> cpal::Device {
    let host = cpal::default_host();
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
            eprintln!("[audio] mic NOT found — no input device available");
            panic!("no input device available");
        }
    }
}
