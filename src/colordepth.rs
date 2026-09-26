//! Terminals without 24-bit color: every `Color::Rgb` is replaced by the nearest xterm-256 index in a final
//! pass over the frame, so a theme still looks like itself (terminals that don't know `38;2;r;g;b` would
//! otherwise show something arbitrary, or nothing). `Indexed` and `Reset` are the terminal's own and stay.

use ratatui::{buffer::Buffer, style::Color};

/// The steps of the 6x6x6 color cube (indices 16..=231).
const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Depth {
    TrueColor,
    Indexed256,
}

impl Depth {
    /// From the environment. `PLATOON_COLORS=truecolor|256` decides outright; otherwise `COLORTERM` says
    /// so, or the terminal is one that is known to do 24-bit color. Anything unknown gets the safe answer.
    pub fn detect() -> Self {
        Self::detect_with(&|k| std::env::var(k).ok())
    }

    pub fn detect_with(env: &dyn Fn(&str) -> Option<String>) -> Self {
        match env("PLATOON_COLORS").map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("truecolor" | "24bit") => return Self::TrueColor,
            Some("256") => return Self::Indexed256,
            _ => {}
        }
        let lower = |k: &str| env(k).unwrap_or_default().to_ascii_lowercase();
        let (colorterm, program, term) = (lower("COLORTERM"), lower("TERM_PROGRAM"), lower("TERM"));
        let known = ["ghostty", "wezterm", "iterm.app", "alacritty", "kitty"].contains(&program.as_str())
            || ["xterm-ghostty", "xterm-kitty", "alacritty", "wezterm"].contains(&term.as_str())
            || ["GHOSTTY_RESOURCES_DIR", "KITTY_WINDOW_ID", "WEZTERM_EXECUTABLE", "ITERM_SESSION_ID"].iter().any(|k| env(k).is_some());
        if colorterm == "truecolor" || colorterm == "24bit" || known { Self::TrueColor } else { Self::Indexed256 }
    }

    pub fn is_truecolor(self) -> bool {
        self == Self::TrueColor
    }

    /// The color as this terminal will get it.
    pub fn color(self, c: Color) -> Color {
        match (self, c) {
            (Self::Indexed256, Color::Rgb(r, g, b)) => Color::Indexed(rgb_to_256(r, g, b)),
            _ => c,
        }
    }

    /// The last thing done to a frame before it is flushed. Nothing at all when the terminal has truecolor.
    pub fn apply(self, buf: &mut Buffer) {
        if self.is_truecolor() {
            return;
        }
        for cell in &mut buf.content {
            cell.fg = self.color(cell.fg);
            cell.bg = self.color(cell.bg);
            cell.underline_color = self.color(cell.underline_color);
        }
    }
}

/// `NO_COLOR` is set to something (<https://no-color.org>; empty counts as unset, as the terminal library has it).
pub fn no_color() -> bool {
    no_color_with(&|k| std::env::var(k).ok())
}

pub fn no_color_with(env: &dyn Fn(&str) -> Option<String>) -> bool {
    env("NO_COLOR").is_some_and(|v| !v.is_empty())
}

/// With `NO_COLOR`, platoon takes the colors out of the frame itself, as the last thing before it is flushed, and
/// tells the terminal library to leave its own color handling alone. Left to the library, every color change
/// becomes a bare `ESC [ m` - a full reset - which takes bold, underline and reverse video with it.
pub fn strip(buf: &mut Buffer) {
    for cell in &mut buf.content {
        cell.fg = Color::Reset;
        cell.bg = Color::Reset;
        cell.underline_color = Color::Reset;
    }
}

/// The nearest of the 6x6x6 cube and the 24-step gray ramp (232..=255), by squared distance in RGB. The
/// sixteen terminal colors are never chosen: their look belongs to the user's terminal theme.
pub fn rgb_to_256(r: u8, g: u8, b: u8) -> u8 {
    let level = |v: u8| CUBE.iter().enumerate().min_by_key(|&(_, &c)| (i32::from(c) - i32::from(v)).abs()).map_or(0, |(i, _)| i);
    let (ri, gi, bi) = (level(r), level(g), level(b));
    let cube = (CUBE[ri], CUBE[gi], CUBE[bi]);
    let cube_index = 16 + 36 * ri + 6 * gi + bi;

    let mean = (u32::from(r) + u32::from(g) + u32::from(b)) / 3;
    let step = ((mean.saturating_sub(8) + 5) / 10).min(23); // the ramp value is 8 + 10 * step
    let gray = (8 + 10 * step) as u8;

    let dist = |(cr, cg, cb): (u8, u8, u8)| {
        let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).pow(2);
        d(cr, r) + d(cg, g) + d(cb, b)
    };
    if dist((gray, gray, gray)) < dist(cube) { (232 + step) as u8 } else { cube_index as u8 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{layout::Rect, style::Style};

    #[test]
    fn no_color_takes_every_color_out_and_keeps_the_modifiers() {
        use ratatui::style::Modifier;
        let env = |v: Option<&'static str>| move |k: &str| (k == "NO_COLOR").then_some(v).flatten().map(String::from);
        assert!(no_color_with(&env(Some("1"))));
        assert!(!no_color_with(&env(Some(""))), "empty counts as unset");
        assert!(!no_color_with(&env(None)));
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 1));
        buf.set_style(Rect::new(0, 0, 3, 1), Style::new().fg(Color::Rgb(1, 2, 3)).bg(Color::Indexed(9)).underline_color(Color::Red).add_modifier(Modifier::BOLD | Modifier::UNDERLINED | Modifier::REVERSED));
        strip(&mut buf);
        for cell in &buf.content {
            assert_eq!((cell.fg, cell.bg, cell.underline_color), (Color::Reset, Color::Reset, Color::Reset));
            assert_eq!(cell.modifier, Modifier::BOLD | Modifier::UNDERLINED | Modifier::REVERSED);
        }
    }

    #[test]
    fn black_and_white_are_the_cube_corners_not_the_ramp_ends() {
        assert_eq!(rgb_to_256(0, 0, 0), 16);
        assert_eq!(rgb_to_256(255, 255, 255), 231);
    }

    #[test]
    fn pure_grays_use_the_ramp_unless_the_cube_has_the_exact_step() {
        assert_eq!(rgb_to_256(8, 8, 8), 232, "the first ramp step");
        assert_eq!(rgb_to_256(238, 238, 238), 255, "the last ramp step");
        assert_eq!(rgb_to_256(128, 128, 128), 232 + 12, "128 = 8 + 10 * 12");
        assert_eq!(rgb_to_256(95, 95, 95), 59, "the cube has 95 exactly, and the ramp doesn't");
        assert_eq!(rgb_to_256(135, 135, 135), 102);
        assert_eq!(rgb_to_256(250, 250, 250), 231, "nearer the cube's white than the ramp's last step");
    }

    #[test]
    fn cube_corners_and_exact_cube_colors_map_to_their_own_index() {
        assert_eq!(rgb_to_256(255, 0, 0), 196);
        assert_eq!(rgb_to_256(0, 255, 0), 46);
        assert_eq!(rgb_to_256(0, 0, 255), 21);
        assert_eq!(rgb_to_256(255, 255, 0), 226);
        assert_eq!(rgb_to_256(0, 255, 255), 51);
        assert_eq!(rgb_to_256(255, 0, 255), 201);
        assert_eq!(rgb_to_256(95, 135, 175), 16 + 36 + 6 * 2 + 3);
    }

    #[test]
    fn a_mid_color_goes_to_the_nearest_cube_step_in_each_channel() {
        // 215,119,87 (Claude's terracotta): 215 -> 215, 119 -> 135 (nearer than 95), 87 -> 95.
        assert_eq!(rgb_to_256(215, 119, 87), 16 + 36 * 4 + 6 * 2 + 1);
        // A dark, slightly blue color: the ramp's 38,38,38 is nearer than the cube's black.
        assert_eq!(rgb_to_256(0x1e, 0x1e, 0x2e), 235);
        // A saturated one stays in the cube.
        assert_eq!(rgb_to_256(0xcb, 0xa6, 0xf7), 16 + 36 * 4 + 6 * 3 + 5);
    }

    #[test]
    fn never_picks_one_of_the_sixteen_terminal_colors() {
        for r in (0..=255).step_by(15) {
            for g in (0..=255).step_by(15) {
                for b in (0..=255).step_by(15) {
                    assert!(rgb_to_256(r as u8, g as u8, b as u8) >= 16);
                }
            }
        }
    }

    #[test]
    fn indexed_and_reset_are_left_alone_and_rgb_is_converted() {
        let d = Depth::Indexed256;
        assert_eq!(d.color(Color::Indexed(9)), Color::Indexed(9));
        assert_eq!(d.color(Color::Reset), Color::Reset);
        assert_eq!(d.color(Color::Green), Color::Green);
        assert_eq!(d.color(Color::Rgb(255, 0, 0)), Color::Indexed(196));
        assert_eq!(d.color(d.color(Color::Rgb(215, 119, 87))), d.color(Color::Rgb(215, 119, 87)), "converting twice changes nothing");
        assert_eq!(Depth::TrueColor.color(Color::Rgb(1, 2, 3)), Color::Rgb(1, 2, 3));
    }

    #[test]
    fn the_buffer_pass_converts_every_cell_and_keeps_everything_else() {
        let area = Rect::new(0, 0, 3, 1);
        let mut buf = Buffer::empty(area);
        buf[(0, 0)].set_symbol("a").set_style(Style::new().fg(Color::Rgb(255, 0, 0)).bg(Color::Rgb(0, 0, 255)));
        buf[(1, 0)].set_symbol("b").set_style(Style::new().fg(Color::Indexed(7)).bg(Color::Reset));
        buf[(2, 0)].set_symbol("c").set_style(Style::new().fg(Color::Rgb(8, 8, 8)));
        let plain = buf.clone();
        Depth::TrueColor.apply(&mut buf);
        assert_eq!(buf, plain, "truecolor: the frame is not touched");
        Depth::Indexed256.apply(&mut buf);
        assert_eq!((buf[(0, 0)].fg, buf[(0, 0)].bg), (Color::Indexed(196), Color::Indexed(21)));
        assert_eq!((buf[(1, 0)].fg, buf[(1, 0)].bg), (Color::Indexed(7), Color::Reset));
        assert_eq!(buf[(2, 0)].fg, Color::Indexed(232));
        assert_eq!(buf[(0, 0)].symbol(), "a");
        let once = buf.clone();
        Depth::Indexed256.apply(&mut buf);
        assert_eq!(buf, once, "a second pass changes nothing");
    }

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn detection_reads_the_override_then_colorterm_then_known_terminals() {
        let d = |pairs: &[(&str, &str)]| Depth::detect_with(&env(pairs));
        assert_eq!(d(&[]), Depth::Indexed256, "nothing known: the safe choice");
        assert_eq!(d(&[("TERM", "xterm-256color")]), Depth::Indexed256);
        assert_eq!(d(&[("COLORTERM", "truecolor")]), Depth::TrueColor);
        assert_eq!(d(&[("COLORTERM", "24bit")]), Depth::TrueColor);
        assert_eq!(d(&[("COLORTERM", "yes")]), Depth::Indexed256);
        for known in ["ghostty", "WezTerm", "iTerm.app", "Alacritty", "kitty"] {
            assert_eq!(d(&[("TERM_PROGRAM", known)]), Depth::TrueColor, "{known}");
        }
        assert_eq!(d(&[("TERM", "xterm-kitty")]), Depth::TrueColor);
        assert_eq!(d(&[("TERM", "alacritty")]), Depth::TrueColor);
        assert_eq!(d(&[("GHOSTTY_RESOURCES_DIR", "/x")]), Depth::TrueColor);
        assert_eq!(d(&[("TERM_PROGRAM", "Apple_Terminal")]), Depth::Indexed256);
        // The override wins both ways.
        assert_eq!(d(&[("PLATOON_COLORS", "256"), ("COLORTERM", "truecolor")]), Depth::Indexed256);
        assert_eq!(d(&[("PLATOON_COLORS", "TrueColor")]), Depth::TrueColor);
        assert_eq!(d(&[("PLATOON_COLORS", "bogus"), ("COLORTERM", "truecolor")]), Depth::TrueColor, "a value that means nothing is ignored");
    }
}
