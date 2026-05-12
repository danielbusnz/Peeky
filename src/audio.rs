use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub fn record_until_release() -> (Vec<f32>, u32, u16) {
    let buffer = Arc::new(Mutex::new(Vec::<f32>::new()));
    let buffer_for_callback = Arc::clone(&buffer);

    let host = cpal::default_host();
    let device = host
        .input_devices()
        .expect("could not list input devices")
        .find(|d| {
            d.name()
                .map(|n| n == "pulse" || n == "pipewire")
                .unwrap_or(false)
        })
        .or_else(|| host.default_input_device())
        .expect("no input device available");

    let supported_config = device
        .default_input_config()
        .expect("no default input config");
    let sample_rate = supported_config.sample_rate();
    let channels = supported_config.channels();
    let config = supported_config.config();

    let stream = device
        .build_input_stream(
            &config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let mut buf = buffer_for_callback.lock().unwrap();
                buf.extend_from_slice(data);
            },
            move |err| eprintln!("audio stream error: {}", err),
            None,
        )
        .expect("failed to build input stream");

    stream.play().expect("failed to start stream");
    println!("recording... (release to stop)");

    while crate::hotkey::is_recording() {
        thread::sleep(Duration::from_millis(10));
    }
    println!("recording stopped");

    let samples = buffer.lock().unwrap().clone();
    (samples, sample_rate, channels)
}
