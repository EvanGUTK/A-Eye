mod config;

#[cfg(windows)]
mod win;

fn main() {
    let config = match config::parse(std::env::args().skip(1)) {
        Ok(Some(config)) => config,
        Ok(None) => {
            println!(
                "Usage: spike-07-windows-hotkey [--hotkey Ctrl+Alt+A] [--hold-secs 120]\n\
                Chords: Ctrl/Alt/Shift plus one ASCII letter or digit.\n\
                Overlay starts OFF; press the chord to toggle the primary-monitor border.\n\
                Ctrl+Alt+Q exits; Ctrl+C or the bounded timeout also exits.\n\
                Overlay demonstration only: no capture, microphone, or provider calls."
            );
            return;
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    #[cfg(windows)]
    if let Err((code, error)) = win::run(config) {
        eprintln!("{error}");
        std::process::exit(code);
    }
    #[cfg(not(windows))]
    {
        let _ = config;
        eprintln!("spike 07 uses Win32 RegisterHotKey and requires Windows");
        std::process::exit(2);
    }
}
