use anyhow::{Context, Result};
use evdev::{uinput::VirtualDeviceBuilder, AttributeSet, EventType, InputEvent, Key, RelativeAxisType};

pub struct Injector {
    dev: evdev::uinput::VirtualDevice,
    pub x: i32,
    pub y: i32,
    pub screen_w: i32,
    pub screen_h: i32,
}

impl Injector {
    pub fn new() -> Result<Self> {
        let mut keys = AttributeSet::<Key>::new();
        for k in ALL_KEYS {
            keys.insert(*k);
        }
        let mut rel = AttributeSet::<RelativeAxisType>::new();
        rel.insert(RelativeAxisType::REL_X);
        rel.insert(RelativeAxisType::REL_Y);
        rel.insert(RelativeAxisType::REL_WHEEL);
        rel.insert(RelativeAxisType::REL_HWHEEL);
        rel.insert(RelativeAxisType::REL_WHEEL_HI_RES);
        rel.insert(RelativeAxisType::REL_HWHEEL_HI_RES);

        let dev = VirtualDeviceBuilder::new()
            .context("open /dev/uinput — add udev rule and group 'input'")?
            .name("DDRDesk Remote")
            .with_keys(&keys)?
            .with_relative_axes(&rel)?
            .build()?;
        // Give udev/logind a moment to attach the device to seat0.
        std::thread::sleep(std::time::Duration::from_millis(80));
        let (sw, sh) = crate::display::logical_size();
        Ok(Self {
            dev,
            x: (sw / 2) as i32,
            y: (sh / 2) as i32,
            screen_w: sw.max(1) as i32,
            screen_h: sh.max(1) as i32,
        })
    }

    pub fn move_rel(&mut self, dx: i32, dy: i32) -> Result<()> {
        if dx == 0 && dy == 0 {
            return Ok(());
        }
        self.x = (self.x + dx).clamp(0, self.screen_w.saturating_sub(1));
        self.y = (self.y + dy).clamp(0, self.screen_h.saturating_sub(1));
        let mut evs = Vec::new();
        if dx != 0 {
            evs.push(rel(RelativeAxisType::REL_X, dx));
        }
        if dy != 0 {
            evs.push(rel(RelativeAxisType::REL_Y, dy));
        }
        evs.push(syn());
        self.dev.emit(&evs)?;
        Ok(())
    }

    pub fn button(&mut self, which: &str, down: bool) -> Result<()> {
        let key = match which {
            "left" => Key::BTN_LEFT,
            "right" => Key::BTN_RIGHT,
            "middle" => Key::BTN_MIDDLE,
            _ => Key::BTN_LEFT,
        };
        let v = if down { 1 } else { 0 };
        self.dev.emit(&[key_ev(key, v), syn()])?;
        Ok(())
    }

    pub fn wheel(&mut self, dx: i32, dy: i32) -> Result<()> {
        let mut evs = Vec::new();
        if dy != 0 {
            evs.push(rel(RelativeAxisType::REL_WHEEL, dy));
        }
        if dx != 0 {
            evs.push(rel(RelativeAxisType::REL_HWHEEL, dx));
        }
        if evs.is_empty() {
            return Ok(());
        }
        evs.push(syn());
        self.dev.emit(&evs)?;
        Ok(())
    }

    pub fn key(&mut self, name: &str, down: bool) -> Result<()> {
        tracing::info!("key {name} down={down}");
        let key = match name.to_ascii_lowercase().as_str() {
            "return" | "enter" => Key::KEY_ENTER,
            "backspace" => Key::KEY_BACKSPACE,
            "tab" => Key::KEY_TAB,
            "escape" | "esc" => Key::KEY_ESC,
            "space" => Key::KEY_SPACE,
            "up" => Key::KEY_UP,
            "down" => Key::KEY_DOWN,
            "left" => Key::KEY_LEFT,
            "right" => Key::KEY_RIGHT,
            "delete" | "del" => Key::KEY_DELETE,
            "home" => Key::KEY_HOME,
            "end" => Key::KEY_END,
            "pageup" => Key::KEY_PAGEUP,
            "pagedown" => Key::KEY_PAGEDOWN,
            "ctrl" | "control" => Key::KEY_LEFTCTRL,
            "shift" => Key::KEY_LEFTSHIFT,
            "alt" => Key::KEY_LEFTALT,
            "meta" | "cmd" | "win" => Key::KEY_LEFTMETA,
            other => {
                if let Some(k) = ascii_key(other.chars().next().unwrap_or('\0')) {
                    k.0
                } else {
                    tracing::debug!("unknown key {other}");
                    return Ok(());
                }
            }
        };
        self.dev
            .emit(&[key_ev(key, if down { 1 } else { 0 }), syn()])?;
        Ok(())
    }

    pub fn text(&mut self, s: &str) -> Result<()> {
        tracing::info!("type {s:?}");
        for ch in s.chars() {
            self.type_char(ch)?;
        }
        Ok(())
    }

    fn type_char(&mut self, ch: char) -> Result<()> {
        if ch == '\n' || ch == '\r' {
            return self.tap(Key::KEY_ENTER, false);
        }
        if ch == '\t' {
            return self.tap(Key::KEY_TAB, false);
        }
        if ch == '\u{8}' || ch == '\u{7f}' {
            return self.tap(Key::KEY_BACKSPACE, false);
        }
        if let Some((key, shift)) = ascii_key(ch) {
            self.tap(key, shift)?;
            return Ok(());
        }
        // Linux Unicode: Ctrl+Shift+U, hex, enter. Works in many Qt/GTK fields.
        let hex = format!("{:x}", ch as u32);
        self.dev.emit(&[
            key_ev(Key::KEY_LEFTCTRL, 1),
            key_ev(Key::KEY_LEFTSHIFT, 1),
            syn(),
        ])?;
        self.tap(Key::KEY_U, false)?;
        self.dev.emit(&[
            key_ev(Key::KEY_LEFTCTRL, 0),
            key_ev(Key::KEY_LEFTSHIFT, 0),
            syn(),
        ])?;
        for h in hex.chars() {
            if let Some((k, _)) = ascii_key(h) {
                self.tap(k, false)?;
            }
        }
        self.tap(Key::KEY_ENTER, false)?;
        Ok(())
    }

    fn tap(&mut self, key: Key, shift: bool) -> Result<()> {
        let mut evs = Vec::new();
        if shift {
            evs.push(key_ev(Key::KEY_LEFTSHIFT, 1));
        }
        evs.push(key_ev(key, 1));
        evs.push(syn());
        evs.push(key_ev(key, 0));
        if shift {
            evs.push(key_ev(Key::KEY_LEFTSHIFT, 0));
        }
        evs.push(syn());
        self.dev.emit(&evs)?;
        Ok(())
    }
}

fn syn() -> InputEvent {
    InputEvent::new(EventType::SYNCHRONIZATION, 0, 0)
}

fn rel(axis: RelativeAxisType, v: i32) -> InputEvent {
    InputEvent::new(EventType::RELATIVE, axis.0, v)
}

fn key_ev(key: Key, v: i32) -> InputEvent {
    InputEvent::new(EventType::KEY, key.code(), v)
}

fn ascii_key(ch: char) -> Option<(Key, bool)> {
    match ch {
        'a'..='z' => Some((letter(ch), false)),
        'A'..='Z' => Some((letter(ch.to_ascii_lowercase()), true)),
        '0' => Some((Key::KEY_0, false)),
        '1' => Some((Key::KEY_1, false)),
        '2' => Some((Key::KEY_2, false)),
        '3' => Some((Key::KEY_3, false)),
        '4' => Some((Key::KEY_4, false)),
        '5' => Some((Key::KEY_5, false)),
        '6' => Some((Key::KEY_6, false)),
        '7' => Some((Key::KEY_7, false)),
        '8' => Some((Key::KEY_8, false)),
        '9' => Some((Key::KEY_9, false)),
        ' ' => Some((Key::KEY_SPACE, false)),
        '-' => Some((Key::KEY_MINUS, false)),
        '_' => Some((Key::KEY_MINUS, true)),
        '=' => Some((Key::KEY_EQUAL, false)),
        '+' => Some((Key::KEY_EQUAL, true)),
        '[' => Some((Key::KEY_LEFTBRACE, false)),
        '{' => Some((Key::KEY_LEFTBRACE, true)),
        ']' => Some((Key::KEY_RIGHTBRACE, false)),
        '}' => Some((Key::KEY_RIGHTBRACE, true)),
        '\\' => Some((Key::KEY_BACKSLASH, false)),
        '|' => Some((Key::KEY_BACKSLASH, true)),
        ';' => Some((Key::KEY_SEMICOLON, false)),
        ':' => Some((Key::KEY_SEMICOLON, true)),
        '\'' => Some((Key::KEY_APOSTROPHE, false)),
        '"' => Some((Key::KEY_APOSTROPHE, true)),
        '`' => Some((Key::KEY_GRAVE, false)),
        '~' => Some((Key::KEY_GRAVE, true)),
        ',' => Some((Key::KEY_COMMA, false)),
        '<' => Some((Key::KEY_COMMA, true)),
        '.' => Some((Key::KEY_DOT, false)),
        '>' => Some((Key::KEY_DOT, true)),
        '/' => Some((Key::KEY_SLASH, false)),
        '?' => Some((Key::KEY_SLASH, true)),
        '!' => Some((Key::KEY_1, true)),
        '@' => Some((Key::KEY_2, true)),
        '#' => Some((Key::KEY_3, true)),
        '$' => Some((Key::KEY_4, true)),
        '%' => Some((Key::KEY_5, true)),
        '^' => Some((Key::KEY_6, true)),
        '&' => Some((Key::KEY_7, true)),
        '*' => Some((Key::KEY_8, true)),
        '(' => Some((Key::KEY_9, true)),
        ')' => Some((Key::KEY_0, true)),
        _ => None,
    }
}

fn letter(ch: char) -> Key {
    match ch {
        'a' => Key::KEY_A,
        'b' => Key::KEY_B,
        'c' => Key::KEY_C,
        'd' => Key::KEY_D,
        'e' => Key::KEY_E,
        'f' => Key::KEY_F,
        'g' => Key::KEY_G,
        'h' => Key::KEY_H,
        'i' => Key::KEY_I,
        'j' => Key::KEY_J,
        'k' => Key::KEY_K,
        'l' => Key::KEY_L,
        'm' => Key::KEY_M,
        'n' => Key::KEY_N,
        'o' => Key::KEY_O,
        'p' => Key::KEY_P,
        'q' => Key::KEY_Q,
        'r' => Key::KEY_R,
        's' => Key::KEY_S,
        't' => Key::KEY_T,
        'u' => Key::KEY_U,
        'v' => Key::KEY_V,
        'w' => Key::KEY_W,
        'x' => Key::KEY_X,
        'y' => Key::KEY_Y,
        'z' => Key::KEY_Z,
        _ => Key::KEY_A,
    }
}

const ALL_KEYS: &[Key] = &[
    Key::KEY_ESC,
    Key::KEY_1,
    Key::KEY_2,
    Key::KEY_3,
    Key::KEY_4,
    Key::KEY_5,
    Key::KEY_6,
    Key::KEY_7,
    Key::KEY_8,
    Key::KEY_9,
    Key::KEY_0,
    Key::KEY_MINUS,
    Key::KEY_EQUAL,
    Key::KEY_BACKSPACE,
    Key::KEY_TAB,
    Key::KEY_Q,
    Key::KEY_W,
    Key::KEY_E,
    Key::KEY_R,
    Key::KEY_T,
    Key::KEY_Y,
    Key::KEY_U,
    Key::KEY_I,
    Key::KEY_O,
    Key::KEY_P,
    Key::KEY_LEFTBRACE,
    Key::KEY_RIGHTBRACE,
    Key::KEY_ENTER,
    Key::KEY_LEFTCTRL,
    Key::KEY_A,
    Key::KEY_S,
    Key::KEY_D,
    Key::KEY_F,
    Key::KEY_G,
    Key::KEY_H,
    Key::KEY_J,
    Key::KEY_K,
    Key::KEY_L,
    Key::KEY_SEMICOLON,
    Key::KEY_APOSTROPHE,
    Key::KEY_GRAVE,
    Key::KEY_LEFTSHIFT,
    Key::KEY_BACKSLASH,
    Key::KEY_Z,
    Key::KEY_X,
    Key::KEY_C,
    Key::KEY_V,
    Key::KEY_B,
    Key::KEY_N,
    Key::KEY_M,
    Key::KEY_COMMA,
    Key::KEY_DOT,
    Key::KEY_SLASH,
    Key::KEY_RIGHTSHIFT,
    Key::KEY_LEFTALT,
    Key::KEY_SPACE,
    Key::KEY_RIGHTALT,
    Key::KEY_LEFTMETA,
    Key::KEY_RIGHTMETA,
    Key::KEY_UP,
    Key::KEY_DOWN,
    Key::KEY_LEFT,
    Key::KEY_RIGHT,
    Key::KEY_DELETE,
    Key::KEY_HOME,
    Key::KEY_END,
    Key::KEY_PAGEUP,
    Key::KEY_PAGEDOWN,
    Key::BTN_LEFT,
    Key::BTN_RIGHT,
    Key::BTN_MIDDLE,
];
