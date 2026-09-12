//! Embeds the Windows resources: the application icon (resource 1, which
//! `winspaces_win32::window_class` loads for every window class) and a
//! VERSIONINFO block so Explorer, SmartScreen and crash dumps see the product
//! name, version and copyright. The version comes from Cargo.toml. The icon
//! lives inside this crate so `cargo package` ships it.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest_dir.join("winspaces.ico");
    let icon = icon.canonicalize().unwrap_or(icon);
    // rc.exe wants a plain path with escaped backslashes, not the `\\?\`
    // form `canonicalize` returns on Windows.
    let icon = icon
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('\\', "\\\\");

    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts: Vec<u16> = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|p| p.parse().unwrap_or(0))
        .collect();
    parts.resize(4, 0);
    let comma = format!("{},{},{},{}", parts[0], parts[1], parts[2], parts[3]);
    let dotted = format!("{}.{}.{}.{}", parts[0], parts[1], parts[2], parts[3]);

    let rc = format!(
        r#"1 ICON "{icon}"

1 VERSIONINFO
FILEVERSION {comma}
PRODUCTVERSION {comma}
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x40004L
FILETYPE 0x1L
FILESUBTYPE 0x0L
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "izantech"
      VALUE "FileDescription", "WinSpaces - independent spaces per monitor, Overview and tiling for Windows"
      VALUE "FileVersion", "{dotted}"
      VALUE "InternalName", "winspaces"
      VALUE "LegalCopyright", "Copyright (C) 2026 izantech. Licensed under the GNU GPL v3."
      VALUE "OriginalFilename", "winspaces.exe"
      VALUE "ProductName", "WinSpaces"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );

    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("winspaces.rc");
    fs::write(&out, rc).unwrap();
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=winspaces.ico");
    embed_resource::compile(&out, embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
