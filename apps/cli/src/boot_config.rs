use std::io::{self, Read, Write};
use wor_domain::boot_config::{MAX_INPUT_CHARACTERS, process_json};

pub fn run() -> anyhow::Result<i32> {
    let mut bytes = Vec::new();
    let result = io::stdin()
        .lock()
        .take((4 * (MAX_INPUT_CHARACTERS + 1)) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())
        .and_then(|_| String::from_utf8(bytes).map_err(|error| error.to_string()))
        .and_then(|input| {
            let input = input.replace("\r\n", "\n").replace('\r', "\n");
            process_json(&input).map_err(|error| error.to_string())
        });
    let (status, response) = match result {
        Ok(mut response) => {
            response
                .as_object_mut()
                .unwrap()
                .insert("ok".into(), true.into());
            (0, response)
        }
        Err(error) => (1, serde_json::json!({"ok": false, "error": error})),
    };
    let mut output = io::stdout().lock();
    serde_json::to_writer(&mut output, &response)?;
    writeln!(output)?;
    Ok(status)
}
