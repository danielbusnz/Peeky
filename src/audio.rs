use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::thread;
use std::time::Duration;

pub fn test_record() {
    let host = cpal::default_host();

    let device = host
        .default_input_device()
        .expect("no input device available");

    let mut supported_configs_range = device
        .supported_input_configs()
        .expect("error while querying configs");

    let config = supported_configs_range
        .next()
        .expect("no supported config?!")
        .with_max_sample_rate()
        .config();

    let stream = device
        .build_input_stream(
            &config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                // react to stream events and read or write stream data here.
            },
            move |err| {
                // react to errors here.
            },
            None,
        )
        .expect("failed to build input stream");

    stream.play().expect("failed to start stream");

    thread::sleep(Duration::from_secs(3));
}
