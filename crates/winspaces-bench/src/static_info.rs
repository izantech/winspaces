//! Group `static`: facts about the binary itself.
//!
//! The PE reader below is deliberately small: it walks the DOS/COFF/optional
//! headers and the section table by hand (no crate does this, so this one
//! doesn't need a dependency for a hundred lines of byte offsets), maps the
//! import directory's RVAs through the section table, and counts thunks.
//! Every offset is bounds-checked; a short or malformed image returns `Err`
//! rather than panicking.

use std::collections::BTreeMap;
use std::path::Path;

use crate::report::{Import, Section, StaticInfo};

// --- bounds-checked little-endian reads -------------------------------------

fn u16_at(buf: &[u8], off: usize) -> Result<u16, String> {
    let end = off
        .checked_add(2)
        .ok_or_else(|| "offset overflow reading u16".to_string())?;
    let bytes = buf
        .get(off..end)
        .ok_or_else(|| format!("truncated PE image: expected 2 bytes at offset {off}"))?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn u32_at(buf: &[u8], off: usize) -> Result<u32, String> {
    let end = off
        .checked_add(4)
        .ok_or_else(|| "offset overflow reading u32".to_string())?;
    let bytes = buf
        .get(off..end)
        .ok_or_else(|| format!("truncated PE image: expected 4 bytes at offset {off}"))?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn u64_at(buf: &[u8], off: usize) -> Result<u64, String> {
    let end = off
        .checked_add(8)
        .ok_or_else(|| "offset overflow reading u64".to_string())?;
    let bytes = buf
        .get(off..end)
        .ok_or_else(|| format!("truncated PE image: expected 8 bytes at offset {off}"))?;
    let mut arr = [0u8; 8];
    arr.copy_from_slice(bytes);
    Ok(u64::from_le_bytes(arr))
}

fn bytes_at(buf: &[u8], off: usize, len: usize) -> Result<&[u8], String> {
    let end = off
        .checked_add(len)
        .ok_or_else(|| "offset overflow reading bytes".to_string())?;
    buf.get(off..end)
        .ok_or_else(|| format!("truncated PE image: expected {len} bytes at offset {off}"))
}

/// A NUL-terminated ASCII string starting at `off`; stops at the buffer's
/// end if no NUL is found (defensive, never panics).
fn c_string_at(buf: &[u8], off: usize) -> Result<String, String> {
    let tail = buf
        .get(off..)
        .ok_or_else(|| format!("truncated PE image: string at offset {off} is out of range"))?;
    let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
    Ok(String::from_utf8_lossy(&tail[..end]).into_owned())
}

// --- section table + RVA mapping --------------------------------------------

struct RawSection {
    virtual_address: u32,
    virtual_size: u32,
    raw_size: u32,
    pointer_to_raw_data: u32,
}

fn rva_to_offset(sections: &[RawSection], rva: u32) -> Option<u32> {
    for s in sections {
        // A section with VirtualSize 0 (rare, some linkers) falls back to
        // its raw size for the range test.
        let span = if s.virtual_size > 0 {
            s.virtual_size
        } else {
            s.raw_size
        };
        if rva >= s.virtual_address && rva < s.virtual_address.saturating_add(span) {
            return Some(s.pointer_to_raw_data + (rva - s.virtual_address));
        }
    }
    None
}

/// Every non-zero thunk in the import name/address table at `rva`, using
/// 8-byte thunks for PE32+ and 4-byte thunks for PE32.
fn count_thunks(
    buf: &[u8],
    sections: &[RawSection],
    rva: u32,
    is_pe32_plus: bool,
) -> Result<u32, String> {
    if rva == 0 {
        return Ok(0);
    }
    let mut offset = rva_to_offset(sections, rva)
        .ok_or_else(|| format!("thunk table RVA 0x{rva:x} maps to no section"))?
        as usize;
    let step = if is_pe32_plus { 8 } else { 4 };
    // A hard cap: a well-formed import table is a few dozen entries at
    // most, and this guards against walking off into garbage on a
    // malformed image instead of looping forever.
    for count in 0..65_536u32 {
        let value = if is_pe32_plus {
            u64_at(buf, offset)?
        } else {
            u32_at(buf, offset)? as u64
        };
        if value == 0 {
            return Ok(count);
        }
        offset += step;
    }
    Err("import thunk table did not terminate within a sane bound".to_string())
}

/// Parses the PE headers of an executable image: section table and imported
/// DLLs with their function counts (imports sorted by DLL name).
fn parse_pe(buf: &[u8]) -> Result<(Vec<Section>, Vec<Import>), String> {
    if buf.len() < 0x40 || buf.get(0..2) != Some(b"MZ") {
        return Err("not a PE image (missing MZ signature)".to_string());
    }
    let e_lfanew = u32_at(buf, 0x3C)? as usize;
    let pe_sig = bytes_at(buf, e_lfanew, 4)?;
    if pe_sig != b"PE\0\0" {
        return Err(format!(
            "not a PE image (no PE signature at offset {e_lfanew})"
        ));
    }

    let coff_offset = e_lfanew + 4;
    let number_of_sections = u16_at(buf, coff_offset + 2)? as usize;
    let size_of_optional_header = u16_at(buf, coff_offset + 16)? as usize;
    let optional_offset = coff_offset + 20;

    let magic = u16_at(buf, optional_offset)?;
    let (is_pe32_plus, rva_count_offset, directories_offset) = match magic {
        0x10B => (false, optional_offset + 92, optional_offset + 96),
        0x20B => (true, optional_offset + 108, optional_offset + 112),
        other => return Err(format!("unsupported optional header magic 0x{other:x}")),
    };

    let section_table_offset = optional_offset + size_of_optional_header;
    // Bounds-check the whole section table up front so every entry read
    // below is guaranteed in range.
    bytes_at(buf, section_table_offset, number_of_sections * 40)?;

    let mut sections = Vec::with_capacity(number_of_sections);
    let mut raw_sections = Vec::with_capacity(number_of_sections);
    for i in 0..number_of_sections {
        let off = section_table_offset + i * 40;
        let name_bytes = bytes_at(buf, off, 8)?;
        let name_end = name_bytes.iter().position(|&b| b == 0).unwrap_or(8);
        let name = String::from_utf8_lossy(&name_bytes[..name_end]).into_owned();
        let virtual_size = u32_at(buf, off + 8)?;
        let virtual_address = u32_at(buf, off + 12)?;
        let raw_size = u32_at(buf, off + 16)?;
        let pointer_to_raw_data = u32_at(buf, off + 20)?;
        sections.push(Section {
            name,
            virtual_size,
            raw_size,
        });
        raw_sections.push(RawSection {
            virtual_address,
            virtual_size,
            raw_size,
            pointer_to_raw_data,
        });
    }

    // Data directory index 1 is the import directory. A number-of-entries
    // below 2 means there is no import directory to read.
    let number_of_rva_and_sizes = u32_at(buf, rva_count_offset)? as usize;
    let mut imports = Vec::new();
    if number_of_rva_and_sizes >= 2 {
        let import_dir_offset = directories_offset + 8;
        let import_rva = u32_at(buf, import_dir_offset)?;
        if import_rva != 0 {
            let mut desc_offset = rva_to_offset(&raw_sections, import_rva).ok_or_else(|| {
                format!("import directory RVA 0x{import_rva:x} maps to no section")
            })? as usize;
            // Same defensive cap as count_thunks: a real import table never
            // has anywhere near this many descriptors.
            for _ in 0..4_096 {
                let original_first_thunk = u32_at(buf, desc_offset)?;
                let time_date_stamp = u32_at(buf, desc_offset + 4)?;
                let forwarder_chain = u32_at(buf, desc_offset + 8)?;
                let name_rva = u32_at(buf, desc_offset + 12)?;
                let first_thunk = u32_at(buf, desc_offset + 16)?;
                if original_first_thunk == 0
                    && time_date_stamp == 0
                    && forwarder_chain == 0
                    && name_rva == 0
                    && first_thunk == 0
                {
                    break;
                }
                let name_offset = rva_to_offset(&raw_sections, name_rva)
                    .ok_or_else(|| format!("import name RVA 0x{name_rva:x} maps to no section"))?;
                let dll = c_string_at(buf, name_offset as usize)?;
                let thunk_rva = if original_first_thunk != 0 {
                    original_first_thunk
                } else {
                    first_thunk
                };
                let functions = count_thunks(buf, &raw_sections, thunk_rva, is_pe32_plus)?;
                imports.push(Import { dll, functions });
                desc_offset += 20;
            }
        }
    }
    imports.sort_by(|a, b| a.dll.cmp(&b.dll));

    Ok((sections, imports))
}

// --- Cargo.lock / Cargo.toml parsing (plain text, no toml crate) -----------

/// The number of `[[package]]` table headers in a `Cargo.lock`.
fn count_lock_packages(text: &str) -> u32 {
    text.lines()
        .filter(|line| line.trim() == "[[package]]")
        .count() as u32
}

/// The key/value pairs of `[profile.release]` as written in `Cargo.toml`.
/// A simple line parser: it stops at the next `[section]` header and strips
/// one layer of surrounding quotes from each value.
fn parse_release_profile(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut in_section = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_section = trimmed == "[profile.release]";
            continue;
        }
        if !in_section || trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once('=') {
            let key = key.trim().to_string();
            let value = value.trim().trim_matches('"').to_string();
            out.insert(key, value);
        }
    }
    out
}

// --- entry point -------------------------------------------------------------

pub fn collect(exe: &Path) -> Result<StaticInfo, String> {
    if !exe.exists() {
        return Err(format!(
            "{} does not exist; build it first with: cargo build --release -p winspaces",
            exe.display()
        ));
    }
    let bytes = std::fs::read(exe).map_err(|e| format!("{}: {e}", exe.display()))?;
    let exe_size = bytes.len() as u64;
    let (sections, imports) = parse_pe(&bytes)?;

    let root = crate::report::repo_root();
    let debug_exe = root.join("target").join("debug").join("winspaces.exe");
    let debug_exe_size = std::fs::metadata(&debug_exe).ok().map(|m| m.len());

    let lock_text = std::fs::read_to_string(root.join("Cargo.lock"))
        .map_err(|e| format!("{}: {e}", root.join("Cargo.lock").display()))?;
    let lock_packages = count_lock_packages(&lock_text);

    let manifest_text = std::fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|e| format!("{}: {e}", root.join("Cargo.toml").display()))?;
    let profile = parse_release_profile(&manifest_text);

    Ok(StaticInfo {
        exe_path: exe.display().to_string(),
        exe_size,
        debug_exe_size,
        sections,
        imports,
        lock_packages,
        profile,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(buf: &mut Vec<u8>, off: usize, data: &[u8]) {
        if buf.len() < off + data.len() {
            buf.resize(off + data.len(), 0);
        }
        buf[off..off + data.len()].copy_from_slice(data);
    }

    /// A minimal synthetic PE32+ image: DOS stub, COFF header, a PE32+
    /// optional header naming one import directory entry, one `.idata`
    /// section, two import descriptors (KERNEL32.dll with 3 thunks,
    /// USER32.dll with 1) and their name strings.
    fn synthetic_pe32_plus() -> Vec<u8> {
        let mut buf = vec![0u8; 0x40];
        buf[0] = b'M';
        buf[1] = b'Z';
        put(&mut buf, 0x3C, &0x40u32.to_le_bytes()); // e_lfanew

        // PE signature at 0x40.
        put(&mut buf, 0x40, b"PE\0\0");

        // COFF header at 0x44 (20 bytes).
        let coff = 0x44;
        put(&mut buf, coff, &0x8664u16.to_le_bytes()); // Machine (x64)
        put(&mut buf, coff + 2, &1u16.to_le_bytes()); // NumberOfSections
        put(&mut buf, coff + 16, &128u16.to_le_bytes()); // SizeOfOptionalHeader

        // Optional header at 0x58 (128 bytes: PE32+ standard/windows
        // fields up to +108, then NumberOfRvaAndSizes, then 2 data
        // directory entries).
        let opt = coff + 20;
        put(&mut buf, opt, &0x20Bu16.to_le_bytes()); // Magic: PE32+
        put(&mut buf, opt + 108, &2u32.to_le_bytes()); // NumberOfRvaAndSizes
                                                       // Directory 0 (export): zero. Directory 1 (import):
        let import_rva = 0x2000u32;
        put(&mut buf, opt + 112 + 8, &import_rva.to_le_bytes());
        put(&mut buf, opt + 112 + 12, &60u32.to_le_bytes()); // size

        // Section table at opt+128 = 0xD8: one ".idata" entry, 40 bytes.
        let sect = opt + 128;
        let mut name = [0u8; 8];
        name[..6].copy_from_slice(b".idata");
        put(&mut buf, sect, &name);
        put(&mut buf, sect + 8, &0x84u32.to_le_bytes()); // VirtualSize
        put(&mut buf, sect + 12, &0x2000u32.to_le_bytes()); // VirtualAddress
        put(&mut buf, sect + 16, &0x84u32.to_le_bytes()); // SizeOfRawData
        put(&mut buf, sect + 20, &0x100u32.to_le_bytes()); // PointerToRawData

        // .idata content, at file offset 0x100 (== VirtualAddress 0x2000).
        let base_off = 0x100usize;
        let base_rva = 0x2000u32;

        // KERNEL32.dll descriptor at RVA 0x2000 (file 0x100).
        let k32_thunks_rva = base_rva + 0x3C;
        let k32_name_rva = base_rva + 0x6C;
        put(&mut buf, base_off, &k32_thunks_rva.to_le_bytes()); // OriginalFirstThunk
        put(&mut buf, base_off + 4, &0u32.to_le_bytes()); // TimeDateStamp
        put(&mut buf, base_off + 8, &0u32.to_le_bytes()); // ForwarderChain
        put(&mut buf, base_off + 12, &k32_name_rva.to_le_bytes()); // Name
        put(&mut buf, base_off + 16, &k32_thunks_rva.to_le_bytes()); // FirstThunk

        // USER32.dll descriptor at RVA 0x2014 (file 0x114).
        let user32_thunks_rva = base_rva + 0x5C;
        let user32_name_rva = base_rva + 0x79;
        let user_off = base_off + 0x14;
        put(&mut buf, user_off, &user32_thunks_rva.to_le_bytes());
        put(&mut buf, user_off + 4, &0u32.to_le_bytes());
        put(&mut buf, user_off + 8, &0u32.to_le_bytes());
        put(&mut buf, user_off + 12, &user32_name_rva.to_le_bytes());
        put(&mut buf, user_off + 16, &user32_thunks_rva.to_le_bytes());

        // Null terminator descriptor at RVA 0x2028 (file 0x128): all zero,
        // already the buffer's default.
        let terminator_off = base_off + 0x28;
        put(&mut buf, terminator_off, &[0u8; 20]);

        // KERNEL32 thunks at file 0x13C: 3 non-zero + 1 zero terminator.
        let k32_thunk_off = base_off + 0x3C;
        put(&mut buf, k32_thunk_off, &1u64.to_le_bytes());
        put(&mut buf, k32_thunk_off + 8, &2u64.to_le_bytes());
        put(&mut buf, k32_thunk_off + 16, &3u64.to_le_bytes());
        put(&mut buf, k32_thunk_off + 24, &0u64.to_le_bytes());

        // USER32 thunks at file 0x15C: 1 non-zero + 1 zero terminator.
        let user32_thunk_off = base_off + 0x5C;
        put(&mut buf, user32_thunk_off, &1u64.to_le_bytes());
        put(&mut buf, user32_thunk_off + 8, &0u64.to_le_bytes());

        // Names.
        let k32_name_off = base_off + 0x6C;
        put(&mut buf, k32_name_off, b"KERNEL32.dll\0");
        let user32_name_off = base_off + 0x79;
        put(&mut buf, user32_name_off, b"USER32.dll\0");

        buf
    }

    #[test]
    fn parses_sections_and_imports() {
        let image = synthetic_pe32_plus();
        let (sections, imports) = parse_pe(&image).unwrap();

        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].name, ".idata");
        assert_eq!(sections[0].virtual_size, 0x84);
        assert_eq!(sections[0].raw_size, 0x84);

        assert_eq!(imports.len(), 2);
        // Sorted by DLL name.
        assert_eq!(imports[0].dll, "KERNEL32.dll");
        assert_eq!(imports[0].functions, 3);
        assert_eq!(imports[1].dll, "USER32.dll");
        assert_eq!(imports[1].functions, 1);
    }

    #[test]
    fn truncated_image_is_an_error() {
        let image = synthetic_pe32_plus();
        // Cut the buffer off partway through the section table.
        let truncated = &image[..0xE0];
        assert!(parse_pe(truncated).is_err());

        // Not even a valid MZ header.
        assert!(parse_pe(&[0u8; 4]).is_err());
    }

    #[test]
    fn counts_lock_packages_from_text() {
        let text = "\
# comment
version = 4

[[package]]
name = \"a\"

[[package]]
name = \"b\"
";
        assert_eq!(count_lock_packages(text), 2);
        assert_eq!(count_lock_packages(""), 0);
    }

    #[test]
    fn parses_release_profile_from_text() {
        let text = "\
[workspace]
members = []

[profile.release]
opt-level = \"z\"
lto = true
codegen-units = 1
panic = \"abort\"
strip = true

[profile.dev]
opt-level = 0
";
        let profile = parse_release_profile(text);
        assert_eq!(profile.get("opt-level").map(String::as_str), Some("z"));
        assert_eq!(profile.get("lto").map(String::as_str), Some("true"));
        assert_eq!(profile.get("codegen-units").map(String::as_str), Some("1"));
        assert_eq!(profile.get("panic").map(String::as_str), Some("abort"));
        assert_eq!(profile.get("strip").map(String::as_str), Some("true"));
        // The next section's keys must not leak in.
        assert_eq!(profile.len(), 5);
    }
}
