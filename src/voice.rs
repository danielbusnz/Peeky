use crate::audio;
use crate::cursor;
use crate::hotkey;
use crate::providers::claude::Claude;
use crate::providers::tts_cartesia::TtsCartesia;
use crate::providers::whisper_openai::WhisperOpenAi;
use crate::providers::{Stt, Tts};
use crate::screenshot;

pub fn run_loop(whisper: WhisperOpenAi, claude: Claude, cartesia: TtsCartesia) {
    println!("aegis ready — hold SUPER+space to talk");
    loop {
        hotkey::wait_for_press();
        if let Err(e) = run_one_turn(&whisper, &claude, &cartesia) {
            eprintln!("voice turn failed: {}", e);
        }
    }
}

fn run_one_turn(
    whisper: &WhisperOpenAi,
    claude: &Claude,
    cartesia: &TtsCartesia,
) -> Result<(), Box<dyn std::error::Error>> {
    let (samples, sr, ch) = audio::record_until_release();
    let transcript = whisper.transcribe(&samples, sr, ch)?;
    println!("you said: {}", transcript);

    let (x, y, w, h) = screenshot::active_workspace_geometry()?;
    let (text, point) = claude.detect_element_location(
        &transcript,
        x as i64,
        y as i64,
        w as i64,
        h as i64,
    )?;
    println!("claude: {}", text);

    // Coords are already clamped to screen bounds inside detect_element_location.
    if let Some((px, py)) = point {
        cursor::point_at(px as i32, py as i32);
    }

    if !text.is_empty() {
        let wav = cartesia.synthesize(&text)?;
        audio::play(&wav)?;
    }

    Ok(())
}
