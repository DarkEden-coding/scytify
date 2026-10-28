//! Keyboard and mouse shortcuts. Keys typed into a focused text field are captured by the
//! field first and never reach this mapping, so Space and "/" still type normally there.
use iced::keyboard::{Key, Modifiers, key::Named};

/// An application command bound to a key or mouse button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    TogglePlayback,
    Previous,
    Next,
    VolumeUp,
    VolumeDown,
    Shuffle,
    Repeat,
    FocusSearch,
    /// Close Settings or the queue panel.
    Escape,
    Back,
    Forward,
    ZoomIn,
    ZoomOut,
    ZoomReset,
}

/// Bindings shown in Settings, in display order.
#[cfg(not(target_os = "macos"))]
pub const HINTS: &[(&str, &str)] = &[
    ("Space", "Play or pause"),
    ("Up / Down, Enter", "Select a loaded song, play selected"),
    ("Ctrl + Left / Right", "Previous or next track"),
    ("Ctrl + Up / Down", "Volume up or down 5%"),
    ("Ctrl + S", "Shuffle on or off"),
    ("Ctrl + R", "Cycle repeat"),
    ("/ or Ctrl + L", "Search"),
    (
        "Esc",
        "Close the device picker, Settings or the queue, leave search",
    ),
    ("Alt + Left / Right", "Back or forward"),
    ("Ctrl + = / -", "Zoom in or out"),
    ("Ctrl + 0", "Reset zoom"),
];

/// On macOS, Command and Control both invoke these shortcuts.
#[cfg(target_os = "macos")]
pub const HINTS: &[(&str, &str)] = &[
    ("Space", "Play or pause"),
    ("Up / Down, Enter", "Select a loaded song, play selected"),
    ("Cmd or Ctrl + Left / Right", "Previous or next track"),
    ("Cmd or Ctrl + Up / Down", "Volume up or down 5%"),
    ("Cmd or Ctrl + S", "Shuffle on or off"),
    ("Cmd or Ctrl + R", "Cycle repeat"),
    ("/ or Cmd or Ctrl + L", "Search"),
    (
        "Esc",
        "Close the device picker, Settings or the queue, leave search",
    ),
    ("Option + Left / Right", "Back or forward"),
    ("Cmd or Ctrl + = / -", "Zoom in or out"),
    ("Cmd or Ctrl + 0", "Reset zoom"),
];

/// Volume change of one Ctrl+Up or Ctrl+Down press.
pub const VOLUME_STEP: f32 = 0.05;

/// Command for a key press that no widget captured.
pub fn map(key: &Key, modifiers: Modifiers) -> Option<Shortcut> {
    if modifiers.alt() {
        return match key {
            Key::Named(Named::ArrowLeft) => Some(Shortcut::Back),
            Key::Named(Named::ArrowRight) => Some(Shortcut::Forward),
            _ => None,
        };
    }
    if modifiers.control() || (cfg!(target_os = "macos") && modifiers.logo()) {
        return match key {
            Key::Named(Named::ArrowLeft) => Some(Shortcut::Previous),
            Key::Named(Named::ArrowRight) => Some(Shortcut::Next),
            Key::Named(Named::ArrowUp) => Some(Shortcut::VolumeUp),
            Key::Named(Named::ArrowDown) => Some(Shortcut::VolumeDown),
            Key::Character(c) => match c.to_ascii_lowercase().as_str() {
                "s" => Some(Shortcut::Shuffle),
                "r" => Some(Shortcut::Repeat),
                "l" | "f" => Some(Shortcut::FocusSearch),
                "=" | "+" => Some(Shortcut::ZoomIn),
                "-" => Some(Shortcut::ZoomOut),
                "0" => Some(Shortcut::ZoomReset),
                _ => None,
            },
            _ => None,
        };
    }
    match key {
        Key::Named(Named::Space) => Some(Shortcut::TogglePlayback),
        Key::Named(Named::Escape) => Some(Shortcut::Escape),
        Key::Character(c) if c.as_str() == "/" => Some(Shortcut::FocusSearch),
        _ => None,
    }
}

/// Volume after one step up or down, kept on whole percents within 0..=1.
pub fn step_volume(volume: f32, up: bool) -> f32 {
    let next: f32 = if up {
        volume + VOLUME_STEP
    } else {
        volume - VOLUME_STEP
    };
    (next.clamp(0.0, 1.0) * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(c: &str) -> Key {
        Key::Character(c.into())
    }
    #[test]
    fn bindings_respect_modifiers() {
        let none = Modifiers::empty();
        let ctrl = Modifiers::CTRL;
        assert_eq!(
            map(&Key::Named(Named::Space), none),
            Some(Shortcut::TogglePlayback)
        );
        assert_eq!(map(&key("/"), none), Some(Shortcut::FocusSearch));
        assert_eq!(map(&key("s"), none), None);
        assert_eq!(map(&key("S"), ctrl), Some(Shortcut::Shuffle));
        assert_eq!(map(&key("r"), ctrl), Some(Shortcut::Repeat));
        assert_eq!(
            map(&Key::Named(Named::ArrowUp), ctrl),
            Some(Shortcut::VolumeUp)
        );
        assert_eq!(
            map(&Key::Named(Named::ArrowLeft), ctrl),
            Some(Shortcut::Previous)
        );
        assert_eq!(
            map(&Key::Named(Named::ArrowLeft), Modifiers::ALT),
            Some(Shortcut::Back)
        );
        assert_eq!(map(&Key::Named(Named::ArrowLeft), none), None);
        assert_eq!(map(&key("="), ctrl), Some(Shortcut::ZoomIn));
        assert_eq!(
            map(&key("+"), ctrl | Modifiers::SHIFT),
            Some(Shortcut::ZoomIn)
        );
        assert_eq!(map(&key("-"), ctrl), Some(Shortcut::ZoomOut));
        assert_eq!(map(&key("0"), ctrl), Some(Shortcut::ZoomReset));
        assert_eq!(map(&key("-"), none), None);
    }
    #[test]
    fn command_shortcuts_are_macos_only() {
        let command = Modifiers::LOGO;
        let expected = if cfg!(target_os = "macos") {
            Some(Shortcut::FocusSearch)
        } else {
            None
        };
        assert_eq!(map(&key("l"), command), expected);
        assert_eq!(map(&key("l"), Modifiers::CTRL), Some(Shortcut::FocusSearch));
        if cfg!(target_os = "macos") {
            assert_eq!(
                map(&Key::Named(Named::ArrowRight), command),
                Some(Shortcut::Next)
            );
            assert_eq!(
                map(&key("+"), command | Modifiers::SHIFT),
                Some(Shortcut::ZoomIn)
            );
        }
    }
    #[test]
    fn volume_steps_are_clamped() {
        assert_eq!(step_volume(0.5, true), 0.55);
        assert_eq!(step_volume(0.98, true), 1.0);
        assert_eq!(step_volume(0.02, false), 0.0);
    }
}
