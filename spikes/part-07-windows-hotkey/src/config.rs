use std::time::Duration;

pub const STOP_HOTKEY: &str = "Ctrl+Alt+Q";
const DEFAULT_HOTKEY: &str = "Ctrl+Alt+A";
const DEFAULT_HOLD_SECS: u64 = 120;
const MAX_HOLD_SECS: u64 = 600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: u8,
}

impl Hotkey {
    pub fn parse(value: &str) -> Result<Self, String> {
        let mut parts: Vec<_> = value.split('+').map(str::trim).collect();
        let key = parts.pop().unwrap_or_default();
        if key.len() != 1 || !key.as_bytes()[0].is_ascii_alphanumeric() {
            return Err("hotkey must end in one ASCII letter or digit".into());
        }
        let mut hotkey = Self {
            ctrl: false,
            alt: false,
            shift: false,
            key: key.as_bytes()[0].to_ascii_uppercase(),
        };
        for part in parts {
            let flag = match part.to_ascii_lowercase().as_str() {
                "ctrl" => &mut hotkey.ctrl,
                "alt" => &mut hotkey.alt,
                "shift" => &mut hotkey.shift,
                _ => return Err(format!("unsupported hotkey modifier: {part}")),
            };
            if *flag {
                return Err(format!("duplicate hotkey modifier: {part}"));
            }
            *flag = true;
        }
        if !hotkey.ctrl && !hotkey.alt && !hotkey.shift {
            return Err("hotkey needs at least one modifier (Ctrl, Alt, Shift)".into());
        }
        Ok(hotkey)
    }

    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        parts.push(char::from(self.key).to_string());
        parts.join("+")
    }
}

#[derive(Debug)]
pub struct Config {
    pub hotkey: Hotkey,
    pub hold: Duration,
}

pub fn parse(args: impl Iterator<Item = String>) -> Result<Option<Config>, String> {
    let mut config = Config {
        hotkey: Hotkey::parse(DEFAULT_HOTKEY)?,
        hold: Duration::from_secs(DEFAULT_HOLD_SECS),
    };
    let mut args = args;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => return Ok(None),
            "--hotkey" => {
                config.hotkey = Hotkey::parse(&args.next().ok_or("--hotkey needs a chord")?)?;
            }
            "--hold-secs" => {
                let seconds = args
                    .next()
                    .ok_or("--hold-secs needs a value")?
                    .parse::<u64>()
                    .map_err(|_| "--hold-secs needs an integer")?;
                if !(1..=MAX_HOLD_SECS).contains(&seconds) {
                    return Err(format!("--hold-secs must be 1..={MAX_HOLD_SECS}"));
                }
                config.hold = Duration::from_secs(seconds);
            }
            _ => return Err(format!("unknown argument: {arg}")),
        }
    }
    if config.hotkey == Hotkey::parse(STOP_HOTKEY)? {
        return Err(format!(
            "toggle chord conflicts with the exit chord {STOP_HOTKEY}"
        ));
    }
    Ok(Some(config))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Result<Option<Config>, String> {
        parse(values.iter().map(|value| (*value).to_string()))
    }

    #[test]
    fn defaults_and_custom_chord() {
        let defaults = args(&[]).unwrap().unwrap();
        assert_eq!(defaults.hotkey.label(), "Ctrl+Alt+A");
        assert_eq!(defaults.hold.as_secs(), 120);
        let custom = args(&["--hotkey", "shift+ctrl+7", "--hold-secs", "5"])
            .unwrap()
            .unwrap();
        assert_eq!(custom.hotkey.label(), "Ctrl+Shift+7");
        assert_eq!(custom.hold.as_secs(), 5);
    }

    #[test]
    fn normalizes_order_case_and_whitespace() {
        assert_eq!(
            Hotkey::parse("alt + CTRL + a").unwrap(),
            Hotkey::parse(DEFAULT_HOTKEY).unwrap()
        );
    }

    #[test]
    fn rejects_ambiguous_or_unsupported_chords() {
        for chord in [
            "",
            "A",
            "Ctrl+",
            "Ctrl++A",
            "Ctrl+Ctrl+A",
            "Win+A",
            "Ctrl+F12",
            "Alt+é",
            "Ctrl+A+B",
        ] {
            assert!(Hotkey::parse(chord).is_err(), "accepted {chord}");
        }
    }

    #[test]
    fn reserves_the_exit_chord_after_normalization() {
        assert!(args(&["--hotkey", "alt+ctrl+q"]).is_err());
    }

    #[test]
    fn rejects_missing_unknown_and_unbounded_arguments() {
        for values in [
            vec!["--hotkey"],
            vec!["--hold-secs"],
            vec!["--unknown"],
            vec!["--hold-secs", "0"],
            vec!["--hold-secs", "601"],
            vec!["--hold-secs", "-1"],
            vec!["--hold-secs", "NaN"],
        ] {
            assert!(args(&values).is_err(), "accepted {values:?}");
        }
        assert!(args(&["--hold-secs", "600"]).is_ok());
    }

    #[test]
    fn help_does_not_start_a_session() {
        assert!(args(&["--help"]).unwrap().is_none());
    }
}
