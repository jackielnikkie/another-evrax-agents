use std::path::Path;

/// Reads an image from the OS clipboard and stages it as a PNG in the temp dir.
/// Windows terminals (conhost / Windows Terminal) paste a *file path* for
/// screenshots, so the real pixels must be pulled from the clipboard the way
/// opencode does (`clipboard.read({ preferredTypes: ["image/png"] })`).
/// Returns the staged path on success.
#[cfg(windows)]
pub fn clipboard_image_png() -> Option<String> {
    use std::process::{Command, Stdio};

    let out = std::env::temp_dir().join(format!(
        "evrax-clip-{}.png",
        std::process::id()
    ));
    let escaped = out.display().to_string().replace('\'', "''");
    let script = format!(
        "$img = Get-Clipboard -Format Image; \
         if ($img -ne $null) {{ $img.Save('{escaped}', [System.Drawing.Imaging.ImageFormat]::Png); 'ok' }} \
         else {{ 'none' }}"
    );
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !String::from_utf8_lossy(&output.stdout).contains("ok") {
        return None;
    }
    (out.is_file() && out.metadata().map(|m| m.len()).unwrap_or(0) > 0)
        .then(|| out.display().to_string())
}

#[cfg(not(windows))]
pub fn clipboard_image_png() -> Option<String> {
    None
}

/// Terminal image support detected from the environment. Windows Terminal and
/// conhost cannot draw pixel graphics inline, so they fall back to the
/// placeholder chip; kitty renders real thumbnails via the graphics protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProtocol {
    Kitty,
    None,
}

pub fn detect_protocol() -> ImageProtocol {
    // kitty sets TERM=xterm-kitty / KITTY_WINDOW_ID; ghostty sets TERM=ghostty
    // and speaks kitty graphics too.
    let term = std::env::var("TERM").unwrap_or_default();
    if std::env::var_os("KITTY_WINDOW_ID").is_some()
        || term.contains("kitty")
        || term.contains("ghostty")
    {
        return ImageProtocol::Kitty;
    }
    ImageProtocol::None
}

pub fn is_image_path(text: &str) -> bool {
    let path = Path::new(text.trim().trim_matches(['"', '\'']));
    if !path.is_file() {
        return false;
    }
    matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp")
    )
}

/// Reads the IHDR chunk of a PNG file. Returns (width, height).
pub fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.len() < 24 || bytes[..8] != SIG {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

/// Standard base64 encoding (std-only implementation).
#[allow(dead_code)] // used by kitty thumbnails in U7
pub fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = u32::from_be_bytes([0, b[0], b[1], b[2]]);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Builds a kitty graphics protocol escape that transmits the PNG bytes and
/// displays them scaled to `cols`x`rows` cells at the current cursor position.
/// The payload is chunked (4096 base64 chars per chunk) with m=1 continuation.
#[allow(dead_code)] // used by kitty thumbnails in U7
pub fn kitty_transmit(id: u32, png: &[u8], cols: u16, rows: u16) -> String {
    let b64 = base64_encode(png);
    let mut out = String::with_capacity(b64.len() + 128);
    let total = b64.len().div_ceil(4096);
    for (i, chunk) in b64.as_bytes().chunks(4096).enumerate() {
        let m = if i + 1 < total { 1 } else { 0 };
        if i == 0 {
            out.push_str(&format!(
                "\x1b_Ga=T,f=100,q=1,i={id},c={cols},r={rows},m={m};"
            ));
        } else {
            out.push_str(&format!("\x1b_Gm={m};"));
        }
        out.push_str(std::str::from_utf8(chunk).unwrap_or(""));
        out.push_str("\x1b\\");
    }
    out
}

/// Deletes a previously displayed kitty image by id.
#[allow(dead_code)] // used by kitty thumbnails in U7
pub fn kitty_delete(id: u32) -> String {
    format!("\x1b_Ga=d,d=i,q=1,i={id}\x1b\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_image_paths_only_when_file_exists() {
        assert!(!is_image_path("definitely-missing.png"));
        let dir = std::env::temp_dir().join("evrax-image-test.png");
        std::fs::write(&dir, b"not really a png").unwrap();
        assert!(is_image_path(dir.to_str().unwrap()));
        assert!(is_image_path(format!("{}\n", dir.display()).as_str()));
        assert!(!is_image_path(dir.with_extension("txt").to_str().unwrap()));
        std::fs::remove_file(&dir).unwrap();
    }

    #[test]
    fn parses_png_ihdr_dimensions() {
        let png: Vec<u8> = [
            0x89u8, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, b'I', b'H', b'D', b'R',
        ]
        .into_iter()
        .chain(1920u32.to_be_bytes())
        .chain(1080u32.to_be_bytes())
        .collect();
        assert_eq!(png_dimensions(&png), Some((1920, 1080)));
        assert_eq!(png_dimensions(b"nope"), None);
        assert_eq!(png_dimensions(&[]), None);
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn kitty_transmit_chunks_long_payloads() {
        let png = vec![0xABu8; 5000];
        let esc = kitty_transmit(7, &png, 12, 4);
        assert_eq!(esc.matches("\x1b\\").count(), 2, "two chunks: {esc}");
        assert!(esc.starts_with("\x1b_Ga=T,f=100,q=1,i=7,c=12,r=4,m=1;"));
        assert!(esc.ends_with("\x1b\\"));
    }

    #[test]
    fn kitty_delete_is_wellformed() {
        assert_eq!(kitty_delete(3), "\x1b_Ga=d,d=i,q=1,i=3\x1b\\");
    }
}
