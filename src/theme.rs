use ratatui::style::Color;

/// A palette. Every color the UI uses comes from here, so adding a theme is adding a constructor.
#[derive(Clone)]
pub struct Theme {
    pub bg: Color,
    pub surface: Color,
    pub border: Color,
    pub dim: Color,
    pub subtext: Color,
    pub text: Color,
    pub accent: Color,
    pub busy: Color,
    pub ok: Color,
    pub warn: Color,
    pub err: Color,
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl Theme {
    pub fn catppuccin_mocha() -> Self {
        Self {
            bg: rgb(0x1e1e2e),
            surface: rgb(0x313244),
            border: rgb(0x45475a),
            dim: rgb(0x6c7086),
            subtext: rgb(0xa6adc8),
            text: rgb(0xcdd6f4),
            accent: rgb(0xb4befe),
            busy: rgb(0xcba6f7),
            ok: rgb(0xa6e3a1),
            warn: rgb(0xf9e2af),
            err: rgb(0xf38ba8),
        }
    }
}
