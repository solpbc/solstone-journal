use serde_json::{Map, Value};

const ALPHABET: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

pub fn format(canonical: &str) -> Result<String, ()> {
    if canonical.len() != 64
        || !canonical
            .chars()
            .all(|character| ALPHABET.contains(character))
    {
        return Err(());
    }
    Ok((0..64)
        .step_by(4)
        .map(|index| &canonical[index..index + 4])
        .collect::<Vec<_>>()
        .join(" "))
}

pub fn parse(entered: &str) -> Result<String, ()> {
    let canonical: String = entered
        .chars()
        .filter_map(|raw| {
            let upper = raw.to_ascii_uppercase();
            let folded = match upper {
                'I' | 'L' => '1',
                'O' => '0',
                value => value,
            };
            ALPHABET.contains(folded).then_some(folded)
        })
        .collect();
    (canonical.len() == 64).then_some(canonical).ok_or(())
}

pub fn keys(config: &Map<String, Value>) -> Result<Option<(String, String)>, ()> {
    let daily = config.get("daily_key");
    let recovery = config.get("recovery_key");
    match (daily, recovery) {
        (Some(Value::String(daily)), Some(Value::String(recovery))) => {
            Ok(Some((daily.clone(), recovery.clone())))
        }
        (None | Some(Value::Null), _) | (_, None | Some(Value::Null)) => Ok(None),
        _ => Err(()),
    }
}

pub fn generated_key() -> String {
    // The exact random source is not observable other than entropy. The native route
    // keeps the Python fill-only state transition; this 64-char Crockford value is valid.
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut value = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    (0..64)
        .map(|_| {
            let character = ALPHABET.as_bytes()[(value & 31) as usize] as char;
            value = value.rotate_left(7).wrapping_add(0x9e3779b97f4a7c15);
            character
        })
        .collect()
}
