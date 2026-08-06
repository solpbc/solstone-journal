// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use unicode_normalization::UnicodeNormalization;

#[derive(Debug)]
struct Atom {
    text: String,
    quoted: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operator {
    And,
    Or,
    Not,
}

impl Operator {
    fn as_str(self) -> &'static str {
        match self {
            Self::And => "AND",
            Self::Or => "OR",
            Self::Not => "NOT",
        }
    }
}

pub(super) fn compile_expression(text: &str) -> Option<String> {
    let normalized = normalize_compile_text(text);
    let atoms = atoms(&normalized);
    let valid_operators: Vec<Option<Operator>> = (0..atoms.len())
        .map(|index| operator_at(&atoms, index))
        .collect();
    let mut output = String::new();
    let mut previous_was_term = false;
    let mut has_term = false;

    for (atom, operator) in atoms.iter().zip(valid_operators) {
        if let Some(operator) = operator {
            if !output.is_empty() {
                output.push(' ');
            }
            output.push_str(operator.as_str());
            previous_was_term = false;
            continue;
        }
        if !is_indexable(&atom.text) {
            continue;
        }
        if previous_was_term {
            output.push_str(" AND ");
        } else if !output.is_empty() {
            output.push(' ');
        }
        output.push_str(&quote_atom(atom));
        previous_was_term = true;
        has_term = true;
    }

    has_term.then_some(output)
}

/// Normalize only the FTS input: temporal echo text retains its raw bytes.
fn normalize_compile_text(text: &str) -> String {
    text.nfc()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn atoms(text: &str) -> Vec<Atom> {
    let mut atoms = Vec::new();
    let mut cursor = 0;
    while cursor < text.len() {
        while let Some(character) = text[cursor..].chars().next() {
            if !character.is_whitespace() {
                break;
            }
            cursor += character.len_utf8();
        }
        if cursor == text.len() {
            break;
        }
        if text[cursor..].starts_with('"') {
            let start = cursor + 1;
            let end = text[start..]
                .find('"')
                .map_or(text.len(), |offset| start + offset);
            atoms.push(Atom {
                text: text[start..end].to_string(),
                quoted: true,
            });
            cursor = if end == text.len() { end } else { end + 1 };
            continue;
        }
        let end = text[cursor..]
            .find(char::is_whitespace)
            .map_or(text.len(), |offset| cursor + offset);
        atoms.push(Atom {
            text: text[cursor..end].to_string(),
            quoted: false,
        });
        cursor = end;
    }
    atoms
}

fn operator_at(atoms: &[Atom], index: usize) -> Option<Operator> {
    let atom = &atoms[index];
    if atom.quoted {
        return None;
    }
    let operator = match atom.text.as_str() {
        "AND" => Operator::And,
        "OR" => Operator::Or,
        "NOT" => Operator::Not,
        _ => return None,
    };
    let before = index
        .checked_sub(1)
        .is_some_and(|before| operand(&atoms[before]));
    let after = atoms.get(index + 1).is_some_and(operand);
    (before && after).then_some(operator)
}

fn operand(atom: &Atom) -> bool {
    !(!atom.quoted && matches!(atom.text.as_str(), "AND" | "OR" | "NOT"))
        && is_indexable(&atom.text)
}

fn is_indexable(text: &str) -> bool {
    text.chars().any(char::is_alphanumeric)
}

fn quote_atom(atom: &Atom) -> String {
    let (body, suffix) = if !atom.quoted && atom.text.ends_with('*') {
        (&atom.text[..atom.text.len() - 1], "*")
    } else {
        (atom.text.as_str(), "")
    };
    format!("\"{}\"{suffix}", body.replace('"', "\"\""))
}
