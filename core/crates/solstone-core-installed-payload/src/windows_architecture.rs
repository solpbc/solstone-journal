// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Closed native Windows artifact identities, independent of the builder host.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsArchitecture {
    X86_64,
    Aarch64,
}

/// Runtime package admission follows the binary's compile target. Cross-host
/// producers pass their selected architecture explicitly instead.
pub const fn compiled_windows_architecture() -> WindowsArchitecture {
    if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
        WindowsArchitecture::Aarch64
    } else {
        WindowsArchitecture::X86_64
    }
}

impl WindowsArchitecture {
    pub const ALL: [Self; 2] = [Self::X86_64, Self::Aarch64];

    pub const fn target_id(self) -> &'static str {
        match self {
            Self::X86_64 => "windows-x86_64",
            Self::Aarch64 => "windows-aarch64",
        }
    }

    pub const fn architecture(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
        }
    }

    pub const fn rust_triple(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64-pc-windows-msvc",
            Self::Aarch64 => "aarch64-pc-windows-msvc",
        }
    }

    pub const fn pe_machine(self) -> u16 {
        match self {
            Self::X86_64 => 0x8664,
            Self::Aarch64 => 0xaa64,
        }
    }

    pub const fn pe_name(self) -> &'static str {
        match self {
            Self::X86_64 => "AMD64",
            Self::Aarch64 => "ARM64",
        }
    }

    pub fn from_target_id(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|arch| arch.target_id() == value)
    }

    pub fn from_pe_machine(value: u16) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|arch| arch.pe_machine() == value)
    }

    pub fn from_target_tuple(id: &str, architecture: &str, triple: &str) -> Result<Self, String> {
        let expected =
            Self::from_target_id(id).ok_or_else(|| format!("unknown Windows target: {id}"))?;
        if architecture != expected.architecture() || triple != expected.rust_triple() {
            return Err(format!(
                "inconsistent Windows target {id}: architecture={architecture}, triple_windows={triple}"
            ));
        }
        Ok(expected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_identities_and_substitution_refusals() {
        for (arch, id, field, triple, machine) in [
            (
                WindowsArchitecture::X86_64,
                "windows-x86_64",
                "x86_64",
                "x86_64-pc-windows-msvc",
                0x8664,
            ),
            (
                WindowsArchitecture::Aarch64,
                "windows-aarch64",
                "aarch64",
                "aarch64-pc-windows-msvc",
                0xaa64,
            ),
        ] {
            assert_eq!(arch.target_id(), id);
            assert_eq!(arch.architecture(), field);
            assert_eq!(arch.rust_triple(), triple);
            assert_eq!(arch.pe_machine(), machine);
            assert_eq!(WindowsArchitecture::from_pe_machine(machine), Some(arch));
            assert_eq!(
                WindowsArchitecture::from_target_tuple(id, field, triple),
                Ok(arch)
            );
            let foreign = if arch == WindowsArchitecture::X86_64 {
                WindowsArchitecture::Aarch64
            } else {
                WindowsArchitecture::X86_64
            };
            assert!(
                WindowsArchitecture::from_target_tuple(foreign.target_id(), field, triple).is_err()
            );
            assert!(
                WindowsArchitecture::from_target_tuple(id, foreign.architecture(), triple).is_err()
            );
            assert!(
                WindowsArchitecture::from_target_tuple(id, field, foreign.rust_triple()).is_err()
            );
        }
        for machine in [0, 0x14c, 0xa641, 0xa64e] {
            assert_eq!(WindowsArchitecture::from_pe_machine(machine), None);
        }
        assert!(
            WindowsArchitecture::from_target_tuple(
                "windows-unknown",
                "aarch64",
                "aarch64-pc-windows-msvc"
            )
            .is_err()
        );
    }
}
