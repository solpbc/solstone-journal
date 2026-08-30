// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! MSVC-compatible command-line encoding for Windows launches.

use thiserror::Error;

const BACKSLASH: u16 = b'\\' as u16;
const QUOTE: u16 = b'"' as u16;
const SPACE: u16 = b' ' as u16;
const TAB: u16 = b'\t' as u16;

#[derive(Debug, Error, PartialEq, Eq)]
pub(super) enum WindowsCommandLineError {
    #[error("Windows command-line input contains an interior NUL")]
    InteriorNul,
}

/// Construct an MSVC-compatible command line without its terminating NUL.
///
/// Derived from Rust 1.97.1 `sys/args/windows.rs::append_arg` and
/// `sys/process/windows.rs::make_command_line`.
pub(super) fn make_command_line(
    program_spelling: &[u16],
    arguments: &[Vec<u16>],
) -> Result<Vec<u16>, WindowsCommandLineError> {
    validate(program_spelling)?;
    for argument in arguments {
        validate(argument)?;
    }

    let mut command_line = Vec::new();
    // Rust's Windows Command implementation always quotes argv[0]. Windows file names cannot
    // contain a quote, so this is intentionally not an escaped argument encoding.
    command_line.push(QUOTE);
    command_line.extend_from_slice(program_spelling);
    command_line.push(QUOTE);

    for argument in arguments {
        command_line.push(SPACE);
        append_argument(&mut command_line, argument);
    }
    Ok(command_line)
}

fn validate(value: &[u16]) -> Result<(), WindowsCommandLineError> {
    if value.contains(&0) {
        Err(WindowsCommandLineError::InteriorNul)
    } else {
        Ok(())
    }
}

fn append_argument(command_line: &mut Vec<u16>, argument: &[u16]) {
    let quote = argument.is_empty() || argument.iter().any(|&unit| matches!(unit, SPACE | TAB));
    if quote {
        command_line.push(QUOTE);
    }

    let mut backslashes = 0;
    for &unit in argument {
        if unit == BACKSLASH {
            backslashes += 1;
        } else {
            if unit == QUOTE {
                command_line.extend(std::iter::repeat_n(BACKSLASH, backslashes + 1));
            }
            backslashes = 0;
        }
        command_line.push(unit);
    }

    if quote {
        command_line.extend(std::iter::repeat_n(BACKSLASH, backslashes));
        command_line.push(QUOTE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().collect()
    }

    // Independent test-only implementation of the documented MSVC argv decoding rules. It does
    // not call the production encoder so an encoder defect cannot self-confirm in these tests.
    fn parse_msvc_argv(command_line: &[u16]) -> Vec<Vec<u16>> {
        let mut arguments = Vec::new();
        let mut index = 0;
        while index < command_line.len() {
            while index < command_line.len() && matches!(command_line[index], SPACE | TAB) {
                index += 1;
            }
            if index == command_line.len() {
                break;
            }

            let mut argument = Vec::new();
            let mut in_quotes = false;
            while index < command_line.len() {
                let mut slash_count = 0;
                while index < command_line.len() && command_line[index] == BACKSLASH {
                    slash_count += 1;
                    index += 1;
                }

                if index < command_line.len() && command_line[index] == QUOTE {
                    argument.extend(std::iter::repeat_n(BACKSLASH, slash_count / 2));
                    if slash_count % 2 == 1 {
                        argument.push(QUOTE);
                        index += 1;
                        continue;
                    }
                    if in_quotes
                        && index + 1 < command_line.len()
                        && command_line[index + 1] == QUOTE
                    {
                        argument.push(QUOTE);
                        index += 2;
                    } else {
                        in_quotes = !in_quotes;
                        index += 1;
                    }
                    continue;
                }

                argument.extend(std::iter::repeat_n(BACKSLASH, slash_count));
                if index == command_line.len()
                    || (!in_quotes && matches!(command_line[index], SPACE | TAB))
                {
                    break;
                }
                argument.push(command_line[index]);
                index += 1;
            }
            arguments.push(argument);
        }
        arguments
    }

    #[test]
    fn msvc_oracle_round_trips_empty_space_quote_backslash_and_unicode_arguments() {
        let program = wide(r"C:\original spelling\tool.exe");
        let arguments = vec![
            wide(""),
            wide("space tab\tvalue"),
            wide(r#"embedded "quote""#),
            wide(r#"slashes\\before"quote"#),
            wide(r"trailing\\"),
            wide("λ\u{4e16}\u{754c}"),
        ];
        let command_line = make_command_line(&program, &arguments).expect("inputs have no NUL");
        let mut expected = vec![program];
        expected.extend(arguments);
        assert_eq!(parse_msvc_argv(&command_line), expected);
    }

    #[test]
    fn argv_zero_is_the_original_program_spelling() {
        let original = wide("tool");
        let resolved = wide(r"C:\resolved\tool.exe");
        let command_line = make_command_line(&original, &[]).unwrap();
        assert_eq!(parse_msvc_argv(&command_line), vec![original]);
        assert_ne!(parse_msvc_argv(&command_line), vec![resolved]);
    }

    #[test]
    fn interior_nul_is_rejected_before_output_is_produced() {
        assert_eq!(
            make_command_line(&[b't' as u16, 0], &[]),
            Err(WindowsCommandLineError::InteriorNul)
        );
        assert_eq!(
            make_command_line(&wide("tool"), &[vec![b'x' as u16, 0]]),
            Err(WindowsCommandLineError::InteriorNul)
        );
    }
}
