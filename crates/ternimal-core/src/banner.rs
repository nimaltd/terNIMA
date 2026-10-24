//! The welcome text shown in a new, empty tab.
//!
//! It is plain text with ANSI escape codes, the same as a device would send, so
//! it also shows that the terminal draws every kind of color and style.

use std::fmt::Write;

use serde::Deserialize;

use crate::VERSION;

/// "TERNIMAL" in block letters with a shadow edge: one entry per letter, six rows each.
///
/// `█` is the face of a letter and the line characters are its shadow, drawn
/// in a darker color so the letters stand out from the screen.
#[rustfmt::skip]
const LOGO: [[&str; 6]; 8] = [
    ["████████╗", "╚══██╔══╝", "   ██║   ", "   ██║   ", "   ██║   ", "   ╚═╝   "],
    ["███████╗", "██╔════╝", "█████╗  ", "██╔══╝  ", "███████╗", "╚══════╝"],
    ["██████╗ ", "██╔══██╗", "██████╔╝", "██╔══██╗", "██║  ██║", "╚═╝  ╚═╝"],
    ["███╗   ██╗", "████╗  ██║", "██╔██╗ ██║", "██║╚██╗██║", "██║ ╚████║", "╚═╝  ╚═══╝"],
    ["██╗", "██║", "██║", "██║", "██║", "╚═╝"],
    ["███╗   ███╗", "████╗ ████║", "██╔████╔██║", "██║╚██╔╝██║", "██║ ╚═╝ ██║", "╚═╝     ╚═╝"],
    [" █████╗ ", "██╔══██╗", "███████║", "██╔══██║", "██║  ██║", "╚═╝  ╚═╝"],
    ["██╗     ", "██║     ", "██║     ", "██║     ", "███████╗", "╚══════╝"],
];

/// The letters of "nima" inside "ternimal", drawn in orange.
const NIMA: std::ops::Range<usize> = 3..7;

const RESET: &str = "\x1b[0m";
const CYAN: &str = "\x1b[36m";
const DIM: &str = "\x1b[2m";

/// Width of the label column in front of the color samples.
const LABEL: usize = 14;

/// A color as 8-bit red, green and blue.
type Rgb = (u8, u8, u8);

/// The terminal's background, which the logo's colors are picked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Background {
    Dark,
    Light,
}

/// The logo's face and shadow colors on one background.
struct LogoColors {
    teal: Rgb,
    teal_shadow: Rgb,
    orange: Rgb,
    orange_shadow: Rgb,
}

impl Background {
    fn logo_colors(self) -> LogoColors {
        match self {
            // Bright faces, with shadows in the same colors at 42% brightness.
            Background::Dark => LogoColors {
                teal: (45, 212, 191),
                teal_shadow: (19, 89, 80),
                orange: (251, 146, 60),
                orange_shadow: (105, 61, 25),
            },
            // Deeper faces that keep their contrast on white, with soft shadows.
            Background::Light => LogoColors {
                teal: (13, 148, 136),
                teal_shadow: (153, 246, 228),
                orange: (234, 88, 12),
                orange_shadow: (254, 215, 170),
            },
        }
    }
}

/// Builds the welcome text, with CR LF line ends as a terminal expects.
pub fn welcome_banner(background: Background) -> String {
    let mut out = String::from("\r\n");
    push_logo(&mut out, &background.logo_colors());

    out.push_str("\r\n");
    let _ = write!(
        out,
        "  {CYAN}v{VERSION}{RESET}  serial, RTT and Zephyr terminal\r\n"
    );
    let _ = write!(
        out,
        "  {DIM}https://github.com/nimaltd/ternimal{RESET}\r\n\r\n"
    );

    // The 16 basic colors, normal and bright.
    label(&mut out, "16 colors");
    for code in 40..48 {
        let _ = write!(out, "\x1b[{code}m   ");
    }
    out.push_str(RESET);
    out.push_str("\r\n");
    label(&mut out, "");
    for code in 100..108 {
        let _ = write!(out, "\x1b[{code}m   ");
    }
    out.push_str(RESET);
    out.push_str("\r\n");

    // The grey ramp at the end of the 256 color table.
    label(&mut out, "256 colors");
    for index in 232..=255 {
        let _ = write!(out, "\x1b[48;5;{index}m ");
    }
    out.push_str(RESET);
    out.push_str("\r\n");

    // A rainbow in 24-bit color.
    label(&mut out, "24-bit");
    for step in 0..48 {
        let (r, g, b) = rainbow(step as f32 * 7.5);
        let _ = write!(out, "\x1b[48;2;{r};{g};{b}m ");
    }
    out.push_str(RESET);
    out.push_str("\r\n");

    label(&mut out, "styles");
    out.push_str(
        "\x1b[1mbold\x1b[0m  \x1b[2mdim\x1b[0m  \x1b[3mitalic\x1b[0m  \
         \x1b[4munderline\x1b[0m  \x1b[7minverse\x1b[0m  \x1b[9mstrikethrough\x1b[0m\r\n",
    );

    out.push_str("\r\n");
    let _ = write!(
        out,
        "  {DIM}Open a serial port with the + button above, or File > New serial session.{RESET}\r\n"
    );
    let _ = write!(
        out,
        "  {DIM}What you type in this tab is only echoed back.{RESET}\r\n\r\n"
    );

    out
}

/// Writes the logo, with "nima" in orange and every shadow in a darker color.
fn push_logo(out: &mut String, colors: &LogoColors) {
    for row in 0..LOGO[0].len() {
        out.push_str("  ");
        // The color now set, so an escape is only written where it changes.
        let mut current = None;
        for (i, letter) in LOGO.iter().enumerate() {
            let (face, shadow) = if NIMA.contains(&i) {
                (colors.orange, colors.orange_shadow)
            } else {
                (colors.teal, colors.teal_shadow)
            };
            for ch in letter[row].chars() {
                let color = match ch {
                    ' ' => None,
                    '█' => Some(face),
                    _ => Some(shadow),
                };
                if let Some((r, g, b)) = color
                    && current != color
                {
                    let _ = write!(out, "\x1b[38;2;{r};{g};{b}m");
                    current = color;
                }
                out.push(ch);
            }
        }
        out.push_str(RESET);
        out.push_str("\r\n");
    }
}

/// Writes the indented label in front of a row of samples.
fn label(out: &mut String, text: &str) {
    let _ = write!(out, "  {DIM}{text:<width$}{RESET}", width = LABEL - 2);
}

/// A bright color of the rainbow at `hue` degrees, as 8-bit red, green and blue.
fn rainbow(hue: f32) -> (u8, u8, u8) {
    const SATURATION: f32 = 0.7;
    const VALUE: f32 = 0.95;

    // The usual HSV to RGB steps: pick the sixth of the color wheel, then mix.
    let chroma = VALUE * SATURATION;
    let sixth = (hue % 360.0) / 60.0;
    let x = chroma * (1.0 - ((sixth % 2.0) - 1.0).abs());
    let (r, g, b) = match sixth as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let base = VALUE - chroma;
    let to_byte = |part: f32| ((part + base) * 255.0).round() as u8;
    (to_byte(r), to_byte(g), to_byte(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: [Background; 2] = [Background::Dark, Background::Light];

    #[test]
    fn shows_the_version() {
        for background in BOTH {
            assert!(welcome_banner(background).contains(VERSION));
        }
    }

    #[test]
    fn every_line_feed_has_a_carriage_return() {
        // LF alone moves down without going back to the first column.
        for background in BOTH {
            let text = welcome_banner(background);
            for (i, _) in text.match_indices('\n') {
                assert_eq!(&text[i - 1..i], "\r", "bare LF at byte {i}");
            }
        }
    }

    #[test]
    fn logo_uses_the_colors_for_its_background() {
        for background in BOTH {
            let text = welcome_banner(background);
            let colors = background.logo_colors();
            for (r, g, b) in [
                colors.teal,
                colors.teal_shadow,
                colors.orange,
                colors.orange_shadow,
            ] {
                let escape = format!("\x1b[38;2;{r};{g};{b}m");
                assert!(text.contains(&escape), "{background:?} misses {escape:?}");
            }
        }
    }

    #[test]
    fn background_comes_from_lowercase_names() {
        // The UI sends "dark" or "light".
        use serde::de::{IntoDeserializer, value::Error};
        for (name, expected) in [("dark", Background::Dark), ("light", Background::Light)] {
            let from: serde::de::value::StrDeserializer<Error> = name.into_deserializer();
            assert_eq!(Background::deserialize(from).unwrap(), expected);
        }
    }

    #[test]
    fn logo_letters_keep_their_width_on_every_row() {
        // Counted in characters: the block letters are several bytes each.
        for (i, letter) in LOGO.iter().enumerate() {
            let width = letter[0].chars().count();
            for row in letter {
                assert_eq!(row.chars().count(), width, "letter {i} is not square");
            }
        }
    }

    #[test]
    fn rainbow_starts_red_and_stays_in_range() {
        assert_eq!(rainbow(0.0), (242, 73, 73));
        for step in 0..360 {
            let (r, g, b) = rainbow(step as f32);
            assert!(r >= 72 && g >= 72 && b >= 72, "hue {step} is too dark");
        }
    }
}
