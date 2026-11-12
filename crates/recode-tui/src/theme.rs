//! Colour themes for the terminal UI.
//!
//! A theme is the palette the TUI draws with. The names match the ones
//! opencode ships; `recode` is the default and keeps the look Re:Code had
//! before themes existed.

use ratatui::style::Color;

/// The colours the UI uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    /// Highlights: the model name, the active picker row, links.
    pub accent: Color,
    /// Success: the agent name, finished todo items.
    pub green: Color,
    /// Secondary text: hints, timestamps, inactive rows.
    pub dim: Color,
    /// Body text.
    pub text: Color,
    /// Tool calls and their output.
    pub tool: Color,
    /// Errors.
    pub error: Color,
    /// Warnings and retries.
    pub warn: Color,
    /// Background of code blocks.
    pub code_bg: Color,
    /// Background of the selected row in a picker.
    pub selected_bg: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self::RECODE
    }
}

impl Theme {
    /// The built-in theme names.
    pub const NAMES: &'static [&'static str] =
        &["recode", "tokyonight", "gruvbox", "catppuccin", "solarized"];

    /// The default palette.
    pub const RECODE: Theme = Theme {
        accent: Color::Rgb(122, 162, 247),
        green: Color::Rgb(158, 206, 106),
        dim: Color::Rgb(96, 104, 128),
        text: Color::Rgb(220, 224, 240),
        tool: Color::Rgb(187, 154, 247),
        error: Color::Rgb(247, 118, 142),
        warn: Color::Rgb(224, 175, 104),
        code_bg: Color::Rgb(36, 40, 59),
        selected_bg: Color::Rgb(45, 52, 78),
    };

    /// Tokyo Night.
    pub const TOKYONIGHT: Theme = Theme {
        accent: Color::Rgb(122, 162, 247),
        green: Color::Rgb(158, 206, 106),
        dim: Color::Rgb(86, 95, 137),
        text: Color::Rgb(192, 202, 245),
        tool: Color::Rgb(187, 154, 247),
        error: Color::Rgb(247, 118, 142),
        warn: Color::Rgb(224, 175, 104),
        code_bg: Color::Rgb(41, 46, 66),
        selected_bg: Color::Rgb(52, 59, 88),
    };

    /// Gruvbox (dark).
    pub const GRUVBOX: Theme = Theme {
        accent: Color::Rgb(131, 165, 152),
        green: Color::Rgb(184, 187, 38),
        dim: Color::Rgb(146, 131, 116),
        text: Color::Rgb(235, 219, 178),
        tool: Color::Rgb(211, 134, 155),
        error: Color::Rgb(251, 73, 52),
        warn: Color::Rgb(250, 189, 47),
        code_bg: Color::Rgb(60, 56, 54),
        selected_bg: Color::Rgb(80, 73, 69),
    };

    /// Catppuccin Mocha.
    pub const CATPPUCCIN: Theme = Theme {
        accent: Color::Rgb(137, 180, 250),
        green: Color::Rgb(166, 227, 161),
        dim: Color::Rgb(108, 112, 134),
        text: Color::Rgb(205, 214, 244),
        tool: Color::Rgb(203, 166, 247),
        error: Color::Rgb(243, 139, 168),
        warn: Color::Rgb(249, 226, 175),
        code_bg: Color::Rgb(49, 50, 68),
        selected_bg: Color::Rgb(69, 71, 90),
    };

    /// Solarized dark.
    pub const SOLARIZED: Theme = Theme {
        accent: Color::Rgb(38, 139, 210),
        green: Color::Rgb(133, 153, 0),
        dim: Color::Rgb(88, 110, 117),
        text: Color::Rgb(147, 161, 161),
        tool: Color::Rgb(108, 113, 196),
        error: Color::Rgb(220, 50, 47),
        warn: Color::Rgb(181, 137, 0),
        code_bg: Color::Rgb(7, 54, 66),
        selected_bg: Color::Rgb(13, 71, 86),
    };

    /// Look a theme up by name, ignoring case and surrounding spaces.
    pub fn by_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "recode" | "default" => Some(Self::RECODE),
            "tokyonight" | "tokyo-night" => Some(Self::TOKYONIGHT),
            "gruvbox" => Some(Self::GRUVBOX),
            "catppuccin" | "mocha" => Some(Self::CATPPUCCIN),
            "solarized" | "solarized-dark" => Some(Self::SOLARIZED),
            _ => None,
        }
    }

    /// The name of a built-in theme, if the palette is one of them.
    pub fn name(&self) -> Option<&'static str> {
        Self::NAMES
            .iter()
            .copied()
            .find(|name| Self::by_name(name) == Some(*self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_up_built_in_themes() {
        assert_eq!(Theme::by_name("recode"), Some(Theme::RECODE));
        assert_eq!(Theme::by_name("TokyoNight"), Some(Theme::TOKYONIGHT));
        assert_eq!(Theme::by_name(" gruvbox "), Some(Theme::GRUVBOX));
        assert_eq!(Theme::by_name("catppuccin"), Some(Theme::CATPPUCCIN));
        assert_eq!(Theme::by_name("solarized"), Some(Theme::SOLARIZED));
        assert_eq!(Theme::by_name("dracula"), None);
    }

    #[test]
    fn names_the_built_in_palettes() {
        assert_eq!(Theme::RECODE.name(), Some("recode"));
        assert_eq!(Theme::GRUVBOX.name(), Some("gruvbox"));
        assert_eq!(Theme::default(), Theme::RECODE);
    }
}
