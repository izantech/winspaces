//! UTF-16 conversions for Win32 wide-string APIs.

/// Encode a Rust string as a null-terminated UTF-16 buffer suitable for
/// `LPCWSTR` parameters.
pub fn encode_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Decode a null-terminated (or unterminated) UTF-16 buffer back to a
/// `String`, stopping at the first NUL.
pub fn wide_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}
