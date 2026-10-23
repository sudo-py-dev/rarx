/// IBM code page 437 characters for bytes 0x80–0xFF, the ZIP default name encoding.
const HIGH: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u{A0}";

/// Decodes CP437 bytes. Bytes below 0x80 map to ASCII.
pub(super) fn decode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| {
            if b < 0x80 {
                char::from(b)
            } else {
                HIGH.chars()
                    .nth(usize::from(b - 0x80))
                    .unwrap_or('\u{FFFD}')
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_has_128_entries() {
        assert_eq!(HIGH.chars().count(), 128);
    }

    #[test]
    fn decodes_mixed_bytes() {
        assert_eq!(decode(b"caf\x82 \x9C"), "café £");
        assert_eq!(decode(&[0xFF]), "\u{A0}");
    }
}
