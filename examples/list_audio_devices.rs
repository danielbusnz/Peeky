use cpal::traits::{DeviceTrait, HostTrait};
fn main() {
    let host = cpal::default_host();
    #[allow(deprecated)]
    {
        println!("default input: {:?}", host.default_input_device().and_then(|d| d.name().ok()));
        println!();
        println!("all input devices visible to cpal:");
        for d in host.input_devices().unwrap() {
            let name = d.name().unwrap_or_else(|_| "<no name>".into());
            let cfg = d.default_input_config().ok();
            let cfg_str = cfg.map(|c| format!("{}Hz {}ch {:?}", c.sample_rate(), c.channels(), c.sample_format()));
            println!("  {:?} -> {:?}", name, cfg_str);
        }
    }
}
