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

pub fn generated_key() -> Result<String, getrandom::Error> {
    let mut bytes = [0_u8; 64];
    getrandom::fill(&mut bytes)?;
    Ok(bytes
        .into_iter()
        // 256 is divisible by the 32-character Crockford alphabet.
        .map(|byte| ALPHABET.as_bytes()[(byte & 31) as usize] as char)
        .collect())
}
