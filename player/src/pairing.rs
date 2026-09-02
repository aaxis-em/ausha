//! The pairing link and the QR code that carries it.

use std::io::IsTerminal;

use qrcode::QrCode;
use qrcode::render::unicode;

/// `ausha://<host>:<port>?token=<token>&name=<sender>`, the same shape the
/// Android app parses from a scanned code or an opened link.
pub fn link(host: &str, port: u16, token: &str, name: &str) -> String {
    format!("ausha://{host}:{port}?token={token}&name={}", escape(name))
}

/// Renders the link for a terminal.
///
/// Half-block characters put two QR modules in one character cell, which is
/// what makes the code square: terminal cells are about twice as tall as they
/// are wide.
///
/// Polarity matters more than it looks. A scanner expects dark modules on a
/// light field, so on a terminal the code is drawn in black on an explicit
/// white background rather than inheriting the user's colour scheme — on a
/// dark theme that would produce an inverted code, which many scanners,
/// including ML Kit, will not read.
pub fn qr(link: &str) -> Option<String> {
    let colored = std::io::stdout().is_terminal();
    let code = QrCode::new(link.as_bytes()).ok()?;
    let rendered = code
        .render::<unicode::Dense1x2>()
        .quiet_zone(true)
        .dark_color(unicode::Dense1x2::Dark)
        .light_color(unicode::Dense1x2::Light)
        .build();

    if !colored {
        return Some(rendered);
    }
    Some(
        rendered
            .lines()
            .map(|line| format!("{BLACK_ON_WHITE}{line}{RESET}"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

const BLACK_ON_WHITE: &str = "\x1b[30;47m";
const RESET: &str = "\x1b[0m";

fn escape(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            ' ' => "%20".to_string(),
            other => format!("%{:02X}", other as u32),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_link_the_app_can_parse() {
        let link = link("192.168.1.5", 6996, "abc123", "my desktop");
        assert_eq!(
            link,
            "ausha://192.168.1.5:6996?token=abc123&name=my%20desktop"
        );
    }

    /// Renders the code to an image and reads it back with a real decoder, so
    /// the thing a phone points at is known to carry the right link.
    #[test]
    fn a_scanner_reads_the_link_back_out() {
        let original = link("192.168.0.12", 6996, "c83887a93b03", "aaxis desktop");
        let image = bitmap(&original, 8, 4);

        let mut prepared = rqrr::PreparedImage::prepare(image);
        let grids = prepared.detect_grids();
        assert_eq!(grids.len(), 1, "exactly one code should be found");

        let (_meta, decoded) = grids[0].decode().expect("decodes");
        assert_eq!(decoded, original);
    }

    /// Paints the code as a grayscale image the way a camera would see it:
    /// black modules on white, with a quiet zone around the outside.
    fn bitmap(link: &str, scale: u32, quiet: u32) -> image::GrayImage {
        let code = QrCode::new(link.as_bytes()).unwrap();
        let modules = code.to_colors();
        let width = code.width() as u32;
        let side = (width + quiet * 2) * scale;

        image::GrayImage::from_fn(side, side, |x, y| {
            let (mx, my) = (x / scale, y / scale);
            if mx < quiet || my < quiet || mx >= width + quiet || my >= width + quiet {
                return image::Luma([255]);
            }
            let module = modules[((my - quiet) * width + (mx - quiet)) as usize];
            match module {
                qrcode::Color::Dark => image::Luma([0]),
                qrcode::Color::Light => image::Luma([255]),
            }
        })
    }

    #[test]
    fn renders_square_and_quiet_zoned_for_a_terminal() {
        let rendered = qr("ausha://10.0.0.1:6996?token=deadbeef").expect("qr");
        let rows: Vec<&str> = rendered.lines().collect();
        assert!(rows.len() > 10, "should be a block of rows");

        // Two modules per row of characters, plus the quiet zone on each side.
        let width = rows[0].chars().filter(|c| !c.is_ascii_control()).count();
        assert!(
            width > rows.len(),
            "cells are taller than wide, so rows < columns"
        );
    }
}
