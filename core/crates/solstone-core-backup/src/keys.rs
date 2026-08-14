// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::BackupError;

pub const RECOVERY_KEY_LENGTH: usize = 64;
pub const CROCKFORD_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

type EntropyFill = fn(&mut [u8]) -> Result<(), getrandom::Error>;
const OS_ENTROPY_FILL: EntropyFill = getrandom::fill;

pub fn generate_daily_key() -> Result<String, BackupError> {
    generate_daily_key_with(OS_ENTROPY_FILL)
}

pub fn generate_recovery_key() -> Result<String, BackupError> {
    generate_recovery_key_with(OS_ENTROPY_FILL)
}

fn generate_daily_key_with(fill: EntropyFill) -> Result<String, BackupError> {
    let mut bytes = [0_u8; 32];
    fill(&mut bytes).map_err(|_| BackupError::Entropy)?;
    Ok(base64_url_no_pad(&bytes))
}

fn generate_recovery_key_with(fill: EntropyFill) -> Result<String, BackupError> {
    let mut bytes = [0_u8; RECOVERY_KEY_LENGTH];
    fill(&mut bytes).map_err(|_| BackupError::Entropy)?;
    Ok(bytes
        .iter()
        .map(|byte| char::from(CROCKFORD_ALPHABET[usize::from(byte & 0x1f)]))
        .collect())
}

pub fn format_recovery_key_display(canonical: &str) -> Result<String, BackupError> {
    validate_canonical_recovery_key(canonical)?;
    Ok(canonical
        .as_bytes()
        .chunks(4)
        .map(|chunk| std::str::from_utf8(chunk).expect("Crockford alphabet is UTF-8"))
        .collect::<Vec<_>>()
        .join(" "))
}

pub fn parse_recovery_key(entered: &str) -> Result<String, BackupError> {
    let mut output = String::with_capacity(RECOVERY_KEY_LENGTH);
    for raw in entered.chars() {
        let folded = match raw.to_ascii_uppercase() {
            'I' | 'L' => '1',
            'O' => '0',
            value => value,
        };
        if folded.is_ascii() && CROCKFORD_ALPHABET.contains(&(folded as u8)) {
            output.push(folded);
        }
    }
    if output.len() != RECOVERY_KEY_LENGTH {
        return Err(BackupError::RecoveryParse);
    }
    Ok(output)
}

pub fn confirm_recovery_key(entered: &str, canonical: &str) -> bool {
    parse_recovery_key(entered).is_ok_and(|parsed| parsed == canonical)
}

fn validate_canonical_recovery_key(canonical: &str) -> Result<(), BackupError> {
    if canonical.len() != RECOVERY_KEY_LENGTH {
        return Err(BackupError::CanonicalRecoveryLength);
    }
    if canonical
        .bytes()
        .any(|byte| !CROCKFORD_ALPHABET.contains(&byte))
    {
        return Err(BackupError::CanonicalRecoveryCharacters);
    }
    Ok(())
}

fn base64_url_no_pad(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(char::from(ALPHABET[usize::from(first >> 2)]));
        output.push(char::from(
            ALPHABET[usize::from(((first & 0x03) << 4) | (second >> 4))],
        ));
        if chunk.len() > 1 {
            output.push(char::from(
                ALPHABET[usize::from(((second & 0x0f) << 2) | (third >> 6))],
            ));
        }
        if chunk.len() > 2 {
            output.push(char::from(ALPHABET[usize::from(third & 0x3f)]));
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn daily_fill(bytes: &mut [u8]) -> Result<(), getrandom::Error> {
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = index as u8;
        }
        Ok(())
    }
    fn recovery_fill(bytes: &mut [u8]) -> Result<(), getrandom::Error> {
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = index as u8;
        }
        Ok(())
    }

    #[test]
    fn fixed_entropy_has_exact_oracles() {
        assert_eq!(
            generate_daily_key_with(daily_fill).unwrap(),
            "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8"
        );
        assert_eq!(
            generate_recovery_key_with(recovery_fill).unwrap(),
            "0123456789ABCDEFGHJKMNPQRSTVWXYZ0123456789ABCDEFGHJKMNPQRSTVWXYZ"
        );
    }

    #[test]
    fn production_entropy_is_os_entropy_and_has_expected_shapes() {
        assert_eq!(OS_ENTROPY_FILL as *const (), getrandom::fill as *const ());
        let daily = generate_daily_key().unwrap();
        assert_eq!(daily.len(), 43);
        assert!(!daily.contains('='));
        assert!(
            daily
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        );
        let recovery = generate_recovery_key().unwrap();
        assert_eq!(recovery.len(), RECOVERY_KEY_LENGTH);
        assert!(
            recovery
                .bytes()
                .all(|byte| CROCKFORD_ALPHABET.contains(&byte))
        );
    }

    #[test]
    fn format_parse_and_confirm_match_reference() {
        let value = "0123456789ABCDEFGHJKMNPQRSTVWXYZ0123456789ABCDEFGHJKMNPQRSTVWXYZ";
        assert_eq!(
            format_recovery_key_display(value).unwrap(),
            "0123 4567 89AB CDEF GHJK MNPQ RSTV WXYZ 0123 4567 89AB CDEF GHJK MNPQ RSTV WXYZ"
        );
        assert_eq!(
            parse_recovery_key(&value.replace('0', "O").to_lowercase()).unwrap(),
            value
        );
        assert!(confirm_recovery_key(
            &format_recovery_key_display(value).unwrap(),
            value
        ));
        assert!(!confirm_recovery_key("short", value));
    }
}
