/// Sanitizes text for safe rendering in the TUI.
///
/// Many CLI tools emit ANSI escape sequences (colors, cursor movement) and other
/// control characters (like carriage returns for spinners). If rendered verbatim
/// inside the TUI, these sequences can corrupt the terminal state and cause
/// "text all over the screen".
///
/// This function strips:
/// - ANSI CSI sequences (ESC [ ... <final>)
/// - ANSI OSC sequences (ESC ] ... BEL / ESC \\)
/// - DCS/SOS/PM/APC sequences (ESC P / ESC X / ESC ^ / ESC _ ... ESC \\)
/// - Other control characters (except newline and tab)
pub(crate) fn sanitize_text_for_tui(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());

    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\x1b' => {
                // Escape sequence
                if i + 1 >= bytes.len() {
                    break;
                }
                match bytes[i + 1] {
                    b'[' => {
                        // CSI: ESC [ ... <final byte 0x40-0x7E>
                        i += 2;
                        while i < bytes.len() {
                            let b = bytes[i];
                            i += 1;
                            if (0x40..=0x7e).contains(&b) {
                                break;
                            }
                        }
                    }
                    b']' => {
                        // OSC: ESC ] ... BEL or ESC \
                        i += 2;
                        while i < bytes.len() {
                            match bytes[i] {
                                0x07 => {
                                    // BEL
                                    i += 1;
                                    break;
                                }
                                b'\x1b' if i + 1 < bytes.len() && bytes[i + 1] == b'\\' => {
                                    // ESC \
                                    i += 2;
                                    break;
                                }
                                _ => i += 1,
                            }
                        }
                    }
                    b'P' | b'X' | b'^' | b'_' => {
                        // DCS/SOS/PM/APC: ESC P ... ESC \
                        i += 2;
                        while i < bytes.len() {
                            if bytes[i] == b'\x1b' && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                                i += 2;
                                break;
                            }
                            i += 1;
                        }
                    }
                    b'(' | b')' | b'*' | b'+' => {
                        // Character set selection sequences are short: ESC ( B
                        i += 2;
                        if i < bytes.len() {
                            i += 1;
                        }
                    }
                    _ => {
                        // Unknown escape - drop ESC + one byte.
                        i += 2;
                    }
                }
            }
            b'\n' | b'\t' => {
                out.push(bytes[i]);
                i += 1;
            }
            b'\r' => {
                // Carriage returns are commonly used for progress spinners.
                i += 1;
            }
            b if b < 0x20 || b == 0x7f => {
                // Other control characters.
                i += 1;
            }
            _ => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }

    String::from_utf8_lossy(&out).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_csi_color() {
        let input = "hi \u{1b}[31mred\u{1b}[0m!";
        assert_eq!(sanitize_text_for_tui(input), "hi red!");
    }

    #[test]
    fn strips_osc_title() {
        let input = "a\u{1b}]0;title\u{7}b";
        assert_eq!(sanitize_text_for_tui(input), "ab");
    }

    #[test]
    fn strips_carriage_returns() {
        let input = "a\rb\rc";
        assert_eq!(sanitize_text_for_tui(input), "abc");
    }

    #[test]
    fn preserves_newlines_and_tabs() {
        let input = "a\tb\nc";
        assert_eq!(sanitize_text_for_tui(input), "a\tb\nc");
    }
}
