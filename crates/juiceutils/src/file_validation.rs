use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProtectionLevel {
    None = 0,
    Low = 1,
    Medium = 2,
    High = 3,
}

impl ProtectionLevel {
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "none" => Self::None,
            "low" => Self::Low,
            "medium" => Self::Medium,
            "high" => Self::High,
            _ => Self::High,
        }
    }

    #[must_use]
    pub fn blocks(self, tier: DangerTier) -> bool {
        self >= tier.level()
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DangerTier {
    Low,
    Medium,
    High,
}

impl DangerTier {
    #[must_use]
    pub const fn level(self) -> ProtectionLevel {
        match self {
            Self::Low => ProtectionLevel::Low,
            Self::Medium => ProtectionLevel::Medium,
            Self::High => ProtectionLevel::High,
        }
    }
}

const LOW_TIER_EXTENSIONS: &[&str] = &[
    "exe",
    "msi",
    "msp",
    "mst",
    "pif",
    "scr",
    "com",
    "cpl",
    "hta",
    "application",
    "gadget",
    "dll",
    "so",
    "dylib",
    "ko",
    "sys",
    "drv",
    "iso",
    "img",
    "vhd",
    "vmdk",
    "vdi",
    "pyc",
    "pyo",
    "class",
    "jar",
];

const MEDIUM_TIER_EXTENSIONS: &[&str] = &[
    "bat", "cmd", "inf", "jse", "lnk", "vbs", "vbe", "wsf", "wsh", "ws", "reg", "rgs", "sct",
    "shb", "shs", "ps1", "psm1", "psd1", "psc1", "psc2", "ps1xml", "psc1xml", "sh", "bash", "csh",
    "ksh", "zsh", "fish", "app", "command", "terminal", "url", "website", "xnk", "xbap",
];

const HIGH_TIER_EXTENSIONS: &[&str] = &[
    "js", "mjs", "cjs", "jsx", "ts", "tsx", "html", "htm", "xhtml", "xht", "shtml", "svg", "php",
    "php3", "php4", "php5", "phtml", "phar", "py", "pyw", "pyi", "rb", "erb", "rake", "pl", "pm",
    "cgi", "asp", "aspx", "ascx", "ashx", "asmx", "cfm", "cfc", "lua", "tcl", "groovy", "gradle",
    "jsp", "jspx", "wss",
];

const DANGEROUS_MAGIC: &[(&[u8], &str, DangerTier)] = &[
    (&[0x4D, 0x5A], "PE executable", DangerTier::Low),
    (&[0x7F, 0x45, 0x4C, 0x46], "ELF executable", DangerTier::Low),
    (
        &[0xFE, 0xED, 0xFA, 0xCE],
        "Mach-O executable",
        DangerTier::Low,
    ),
    (
        &[0xFE, 0xED, 0xFA, 0xCF],
        "Mach-O 64-bit executable",
        DangerTier::Low,
    ),
    (
        &[0xCE, 0xFA, 0xED, 0xFE],
        "Mach-O executable",
        DangerTier::Low,
    ),
    (
        &[0xCF, 0xFA, 0xED, 0xFE],
        "Mach-O 64-bit executable",
        DangerTier::Low,
    ),
    (
        &[0xCA, 0xFE, 0xBA, 0xBE],
        "Java class file",
        DangerTier::Low,
    ),
    (
        &[0x40, 0x65, 0x63, 0x68, 0x6F],
        "batch script",
        DangerTier::Medium,
    ),
    (&[0x23, 0x21], "shell script", DangerTier::Medium),
    (
        &[0x66, 0x75, 0x6E, 0x63, 0x74, 0x69, 0x6F, 0x6E],
        "JavaScript",
        DangerTier::High,
    ),
    (
        &[0x3C, 0x21, 0x44, 0x4F, 0x43, 0x54, 0x59, 0x50, 0x45],
        "HTML document",
        DangerTier::High,
    ),
    (
        &[0x3C, 0x68, 0x74, 0x6D, 0x6C],
        "HTML document",
        DangerTier::High,
    ),
    (
        &[0x3C, 0x3F, 0x78, 0x6D, 0x6C],
        "XML/SVG document",
        DangerTier::High,
    ),
    (
        &[0x3C, 0x3F, 0x70, 0x68, 0x70],
        "PHP script",
        DangerTier::High,
    ),
];

#[derive(Debug)]
pub enum FileValidation {
    Allowed,

    BlockedExtension {
        ext: String,
        tier: DangerTier,
    },

    BlockedMagic {
        description: String,
        tier: DangerTier,
    },

    Empty,
}

#[must_use]
pub fn friendly_block_reason(tier: DangerTier) -> String {
    match tier {
        DangerTier::Low => "Executable files, installers, and disk images are not allowed".into(),
        DangerTier::Medium => "Script files (batch, shell, PowerShell) are not allowed".into(),
        DangerTier::High => "HTML, JavaScript, and scripting files are not allowed".into(),
    }
}

#[must_use]
pub fn validate_filename(filename: &str, level: ProtectionLevel) -> FileValidation {
    if level == ProtectionLevel::None {
        return FileValidation::Allowed;
    }

    if let Some(ext) = Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
    {
        if level.blocks(DangerTier::Low) && LOW_TIER_EXTENSIONS.contains(&ext.as_str()) {
            return FileValidation::BlockedExtension {
                ext,
                tier: DangerTier::Low,
            };
        }
        if level.blocks(DangerTier::Medium) && MEDIUM_TIER_EXTENSIONS.contains(&ext.as_str()) {
            return FileValidation::BlockedExtension {
                ext,
                tier: DangerTier::Medium,
            };
        }
        if level.blocks(DangerTier::High) && HIGH_TIER_EXTENSIONS.contains(&ext.as_str()) {
            return FileValidation::BlockedExtension {
                ext,
                tier: DangerTier::High,
            };
        }
    }

    FileValidation::Allowed
}

#[must_use]
pub fn validate_file(filename: &str, data: &[u8], level: ProtectionLevel) -> FileValidation {
    let filename_result = validate_filename(filename, level);
    if !matches!(filename_result, FileValidation::Allowed) {
        return filename_result;
    }

    if level == ProtectionLevel::None {
        return FileValidation::Allowed;
    }

    if data.is_empty() {
        return FileValidation::Empty;
    }

    for (magic, desc, tier) in DANGEROUS_MAGIC {
        if level.blocks(*tier) && data.len() >= magic.len() && &data[..magic.len()] == *magic {
            return FileValidation::BlockedMagic {
                description: desc.to_string(),
                tier: *tier,
            };
        }
    }

    FileValidation::Allowed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_allows_everything() {
        assert!(matches!(
            validate_file("malware.exe", b"MZ\x90\x00", ProtectionLevel::None),
            FileValidation::Allowed
        ));
    }

    #[test]
    fn low_blocks_exe() {
        assert!(matches!(
            validate_file("app.exe", b"MZ\x90\x00", ProtectionLevel::Low),
            FileValidation::BlockedExtension {
                tier: DangerTier::Low,
                ..
            }
        ));
    }

    #[test]
    fn filename_only_validation_blocks_dangerous_extensions() {
        assert!(matches!(
            validate_filename("app.exe", ProtectionLevel::Low),
            FileValidation::BlockedExtension {
                tier: DangerTier::Low,
                ..
            }
        ));
    }

    #[test]
    fn low_allows_sh() {
        assert!(matches!(
            validate_file("script.sh", b"#!/bin/bash", ProtectionLevel::Low),
            FileValidation::Allowed
        ));
    }

    #[test]
    fn medium_blocks_sh() {
        assert!(matches!(
            validate_file("script.sh", b"#!/bin/bash", ProtectionLevel::Medium),
            FileValidation::BlockedExtension {
                tier: DangerTier::Medium,
                ..
            }
        ));
    }

    #[test]
    fn medium_blocks_bat() {
        assert!(matches!(
            validate_file("autoexec.bat", b"@echo off", ProtectionLevel::Medium),
            FileValidation::BlockedExtension {
                tier: DangerTier::Medium,
                ..
            }
        ));
    }

    #[test]
    fn medium_allows_js() {
        assert!(matches!(
            validate_file("app.js", b"console.log('hi')", ProtectionLevel::Medium),
            FileValidation::Allowed
        ));
    }

    #[test]
    fn high_blocks_js() {
        assert!(matches!(
            validate_file("app.js", b"function() {", ProtectionLevel::High),
            FileValidation::BlockedExtension {
                tier: DangerTier::High,
                ..
            }
        ));
    }

    #[test]
    fn high_blocks_html() {
        assert!(matches!(
            validate_file("page.html", b"<!DOCTYPE html>", ProtectionLevel::High),
            FileValidation::BlockedExtension {
                tier: DangerTier::High,
                ..
            }
        ));
    }

    #[test]
    fn high_blocks_svg() {
        assert!(matches!(
            validate_file("image.svg", b"<svg xmlns", ProtectionLevel::High),
            FileValidation::BlockedExtension {
                tier: DangerTier::High,
                ..
            }
        ));
    }

    #[test]
    fn blocks_pe_magic_at_low() {
        assert!(matches!(
            validate_file("renamed.txt", b"MZ\x90\x00\x03", ProtectionLevel::Low),
            FileValidation::BlockedMagic {
                tier: DangerTier::Low,
                ..
            }
        ));
    }

    #[test]
    fn blocks_elf_magic_at_low() {
        assert!(matches!(
            validate_file("renamed.txt", b"\x7FELF", ProtectionLevel::Low),
            FileValidation::BlockedMagic {
                tier: DangerTier::Low,
                ..
            }
        ));
    }

    #[test]
    fn blocks_shebang_magic_at_medium() {
        assert!(matches!(
            validate_file("renamed.txt", b"#!/bin/bash", ProtectionLevel::Medium),
            FileValidation::BlockedMagic {
                tier: DangerTier::Medium,
                ..
            }
        ));
    }

    #[test]
    fn shebang_allowed_at_low() {
        assert!(matches!(
            validate_file("renamed.txt", b"#!/bin/bash", ProtectionLevel::Low),
            FileValidation::Allowed
        ));
    }

    #[test]
    fn allows_safe_file() {
        assert!(matches!(
            validate_file("photo.jpg", b"\xFF\xD8\xFF", ProtectionLevel::High),
            FileValidation::Allowed
        ));
    }

    #[test]
    fn rejects_empty_file() {
        assert!(matches!(
            validate_file("empty.txt", b"", ProtectionLevel::Low),
            FileValidation::Empty
        ));
    }

    #[test]
    fn tier_blocks_include_lower() {
        assert!(matches!(
            validate_file("app.exe", b"MZ", ProtectionLevel::Medium),
            FileValidation::BlockedExtension {
                tier: DangerTier::Low,
                ..
            }
        ));

        assert!(matches!(
            validate_file("script.sh", b"#!/bin/bash", ProtectionLevel::High),
            FileValidation::BlockedExtension {
                tier: DangerTier::Medium,
                ..
            }
        ));
        assert!(matches!(
            validate_file("app.exe", b"MZ", ProtectionLevel::High),
            FileValidation::BlockedExtension {
                tier: DangerTier::Low,
                ..
            }
        ));
    }
}
