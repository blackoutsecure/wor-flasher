//! Loss-aware firmware configuration editing, with no image or device access.

use serde_json::{Map, Value, json};
use std::collections::{BTreeSet, HashMap, HashSet};
use thiserror::Error;

pub const MAX_INPUT_CHARACTERS: usize = 512 * 1024;
const MAX_CONFIG_BYTES: usize = 64 * 1024;
const MANAGED_START: &str = "# >>> WoR-Flasher managed boot settings >>>";
const MANAGED_END: &str = "# <<< WoR-Flasher managed boot settings <<<";
const USER_HEADER: &str = "# User additions - use Advanced Text Edit (Not Recommended)";
const MANAGED_NOTE: &str =
    "# Required boot entries are protected; use Boot Config Editor for supported settings.";
const LEGACY_HEADER: &str = "# don't change anything below this point #";
const PROTECTED: &[&str] = &[
    "arm_64bit",
    "armstub",
    "device_tree_address",
    "device_tree_end",
    "disable_commandline_tags",
    "enable_gic",
    "pciex4_reset",
    "framebuffer_depth",
];
const REPEATABLE: &[&str] = &["dtoverlay", "dtparam", "gpio"];
const HDMI_KEYS: &[&str] = &[
    "hdmi_force_hotplug",
    "hdmi_group",
    "hdmi_mode",
    "hdmi_drive",
    "hdmi_cvt",
];
const BOOLEANS: &[(&str, &str)] = &[
    ("disable_overscan", "Remove display borders (overscan)"),
    ("enable_uart", "Enable serial console"),
    ("uart_2ndstage", "Show early boot serial messages"),
    ("arm_boost", "Enable normal CPU boost"),
    (
        "usb_max_current_enable",
        "Allow higher USB power (requires a suitable supply)",
    ),
    ("force_turbo", "Keep maximum clock speed (uses more power)"),
    (
        "hdmi_force_hotplug",
        "Use HDMI even when no display is detected",
    ),
];
const NUMERIC: &[&str] = &[
    "core_freq",
    "arm_freq",
    "gpu_mem",
    "boot_delay",
    "boot_delay_ms",
];
const IOT_720P: &str = "hdmi_force_hotplug=1\nhdmi_group=1\nhdmi_mode=4\n";
const IOT_1080P: &str = "hdmi_force_hotplug=1\nhdmi_group=1\nhdmi_mode=16\n";

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("{0}")]
pub struct ConfigError(String);

fn error(message: impl Into<String>) -> ConfigError {
    ConfigError(message.into())
}

#[derive(Clone, Debug)]
struct Line {
    raw: String,
    number: usize,
    block: usize,
    unconditional: bool,
    key: String,
    value: String,
}

impl Line {
    fn plain(&self, raw: String) -> Self {
        Self {
            raw,
            key: String::new(),
            value: String::new(),
            ..self.clone()
        }
    }
}

fn base_key(key: &str) -> &str {
    key.split(':').next().unwrap_or("")
}

fn is_boolean(key: &str) -> bool {
    BOOLEANS.iter().any(|(item, _)| *item == key)
}

fn is_known_boolean(key: &str) -> bool {
    is_boolean(key)
        || [
            "arm_64bit",
            "enable_gic",
            "hdmi_ignore_hotplug",
            "hdmi_safe",
            "disable_splash",
        ]
        .contains(&key)
}

fn is_scalar(key: &str) -> bool {
    PROTECTED.contains(&key)
        || is_known_boolean(key)
        || HDMI_KEYS.contains(&key)
        || NUMERIC.contains(&key)
        || key == "avoid_warnings"
}

fn split_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<_> = text
        .split(['\n', '\u{0085}', '\u{2028}', '\u{2029}'])
        .collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

fn parse_value(text: &Value, warnings: &mut Vec<String>) -> Result<Vec<Line>, ConfigError> {
    parse(
        text.as_str()
            .ok_or_else(|| error("config.txt must be text of at most 64 KiB."))?,
        warnings,
    )
}

fn assignment(code: &str) -> Option<(&str, &str)> {
    let (key, value) = code.split_once('=')?;
    let key = key.trim_end();
    let name = match key.split_once(':') {
        Some((name, "0" | "1")) => name,
        Some(_) => return None,
        None => key,
    };
    let mut chars = name.chars();
    if !chars.next()?.is_ascii_alphabetic()
        || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return None;
    }
    Some((key, value.trim()))
}

fn parse(text: &str, warnings: &mut Vec<String>) -> Result<Vec<Line>, ConfigError> {
    if text.len() > MAX_CONFIG_BYTES {
        return Err(error("config.txt must be text of at most 64 KiB."));
    }
    if text
        .chars()
        .any(|ch| ch < ' ' && !['\r', '\n', '\t'].contains(&ch))
    {
        return Err(error(
            "config.txt contains a NUL or another invalid control character.",
        ));
    }
    let text = if let Some(text) = text.strip_prefix('\u{feff}') {
        warnings.push("Removed a UTF-8 byte-order mark from the start of config.txt.".into());
        text
    } else {
        text
    };
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines = Vec::new();
    let mut block = 0;
    let mut unconditional = true;
    let mut markers = Vec::new();
    for (index, raw) in split_lines(&text).into_iter().enumerate() {
        let number = index + 1;
        let stripped = raw.trim();
        if [MANAGED_START, MANAGED_END].contains(&stripped) {
            markers.push(stripped);
            continue;
        }
        if [LEGACY_HEADER, USER_HEADER, MANAGED_NOTE].contains(&stripped) {
            continue;
        }
        let code = raw.split('#').next().unwrap_or("").trim();
        let mut line = Line {
            raw: raw.into(),
            number,
            block,
            unconditional,
            key: String::new(),
            value: String::new(),
        };
        if code.is_empty() {
            lines.push(line);
            continue;
        }
        if raw.chars().count() > 98 {
            return Err(error(format!(
                "Line {number}: firmware ignores characters past the 98-character limit."
            )));
        }
        if !code.is_ascii() {
            return Err(error(format!(
                "Line {number}: firmware settings must use ASCII characters."
            )));
        }
        if code.starts_with('[') {
            if !code.ends_with(']')
                || code.len() < 3
                || code[1..code.len() - 1].contains(['[', ']'])
            {
                return Err(error(format!(
                    "Line {number}: malformed conditional section."
                )));
            }
            block += 1;
            unconditional = code == "[all]";
            line.block = block;
            line.unconditional = unconditional;
            lines.push(line);
            continue;
        }
        if code.starts_with("include ") {
            return Err(error(format!(
                "Line {number}: include files cannot be verified or copied by this editor. Keep settings in this file."
            )));
        }
        let Some((key, value)) = assignment(code) else {
            return Err(error(format!(
                "Line {number}: expected key=value, a comment or a conditional section."
            )));
        };
        if value.is_empty() {
            return Err(error(format!(
                "Line {number}: {key} has no value; remove the line to use firmware defaults."
            )));
        }
        line.key = key.into();
        line.value = value.into();
        lines.push(line);
    }
    if !markers.is_empty() && markers != [MANAGED_START, MANAGED_END] {
        return Err(error(
            "The managed boot-settings markers are incomplete, duplicated or out of order.",
        ));
    }
    let mut last = HashMap::new();
    for (index, line) in lines.iter().enumerate() {
        if !line.key.is_empty() && is_scalar(base_key(&line.key)) {
            last.insert((line.block, line.key.as_str()), index);
        }
    }
    let mut normalized = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !line.key.is_empty()
            && is_scalar(base_key(&line.key))
            && last[&(line.block, line.key.as_str())] != index
        {
            warnings.push(format!(
                "Removed duplicate {} on line {}; kept the last value in that section.",
                line.key, line.number
            ));
            if let Some((_, comment)) = line.raw.split_once('#') {
                normalized.push(line.plain(format!("#{comment}")));
            }
        } else {
            normalized.push(line.clone());
        }
    }
    Ok(normalized)
}

fn scalar_values(lines: &[Line]) -> Map<String, Value> {
    let mut values = Map::new();
    for line in lines {
        if !line.key.is_empty() && line.unconditional && !REPEATABLE.contains(&line.key.as_str()) {
            values.insert(line.key.clone(), json!(line.value));
        }
    }
    values
}

fn text_value<'a>(values: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    values.get(key).and_then(Value::as_str)
}

fn integer(value: &str, key: &str) -> Result<String, ConfigError> {
    if !value.is_empty() && value.bytes().all(|ch| ch.is_ascii_digit()) {
        let stripped = value.trim_start_matches('0');
        return Ok(if stripped.is_empty() { "0" } else { stripped }.into());
    }
    if let Some(hex) = value.strip_prefix("0x")
        && !hex.is_empty()
        && hex.bytes().all(|ch| ch.is_ascii_hexdigit())
    {
        //Firmware values are bounded text, but Python integers are not machine-sized.
        let mut decimal = vec![0_u8];
        for ch in hex.bytes() {
            let mut carry = u16::from(if ch.is_ascii_digit() {
                ch - b'0'
            } else {
                ch.to_ascii_lowercase() - b'a' + 10
            });
            for digit in &mut decimal {
                let number = u16::from(*digit) * 16 + carry;
                *digit = (number % 10) as u8;
                carry = number / 10;
            }
            while carry != 0 {
                decimal.push((carry % 10) as u8);
                carry /= 10;
            }
        }
        return Ok(decimal
            .iter()
            .rev()
            .map(|digit| char::from(b'0' + digit))
            .collect());
    }
    Err(error(format!(
        "{key} must be an integer, not {}.",
        python_repr(value)
    )))
}

fn at_most(value: &str, maximum: u32) -> bool {
    let maximum = maximum.to_string();
    value.len() < maximum.len() || value.len() == maximum.len() && value <= maximum.as_str()
}

fn same_value(key: &str, value: &str, expected: &str) -> Result<bool, ConfigError> {
    if ["armstub", "pciex4_reset"].contains(&key) {
        return Ok(value == expected);
    }
    Ok(integer(value, key)? == integer(expected, key)?)
}

fn required_values_match(lines: &[Line], key: &str, expected: &str) -> Result<bool, ConfigError> {
    for line in lines.iter().filter(|line| base_key(&line.key) == key) {
        if !same_value(key, &line.value, expected)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn check_values(
    lines: &[Line],
    model: &str,
    warnings: &mut Vec<String>,
    required_overlays: &[String],
) -> Result<(), ConfigError> {
    for line in lines {
        if line.key.is_empty() {
            continue;
        }
        let key = base_key(&line.key);
        let value = &line.value;
        if is_known_boolean(key) && !["0", "1"].contains(&integer(value, key)?.as_str()) {
            return Err(error(format!(
                "Line {}: {key} must be 0 (off) or 1 (on).",
                line.number
            )));
        }
        if key == "hdmi_group" && !["0", "1", "2"].contains(&integer(value, key)?.as_str()) {
            return Err(error(format!(
                "Line {}: hdmi_group must be 0 (auto), 1 (CEA) or 2 (DMT).",
                line.number
            )));
        }
        if key == "hdmi_drive" && !["1", "2"].contains(&integer(value, key)?.as_str()) {
            return Err(error(format!(
                "Line {}: hdmi_drive must be 1 (DVI) or 2 (HDMI).",
                line.number
            )));
        }
        if key == "hdmi_mode" && !at_most(&integer(value, key)?, 107) {
            return Err(error(format!(
                "Line {}: hdmi_mode is outside the documented 0-107 range.",
                line.number
            )));
        }
        if key == "hdmi_cvt" {
            let parts: Vec<_> = value.split_whitespace().collect();
            if !(3..=7).contains(&parts.len()) {
                return Err(error(
                    "hdmi_cvt needs width, height and refresh, followed by up to four timing options.",
                ));
            }
            let values = parts
                .into_iter()
                .map(|part| integer(part, key))
                .collect::<Result<Vec<_>, _>>()?;
            if values[..3].iter().any(|number| number == "0") {
                return Err(error(
                    "hdmi_cvt width, height and refresh must be positive.",
                ));
            }
            if values.len() > 3 && !["1", "2", "3", "4", "5", "6"].contains(&values[3].as_str()) {
                return Err(error("hdmi_cvt aspect must be 1-6."));
            }
            if values[4.min(values.len())..]
                .iter()
                .any(|number| !["0", "1"].contains(&number.as_str()))
            {
                return Err(error(
                    "hdmi_cvt margins, interlace and reduced blanking must be 0 or 1.",
                ));
            }
        }
        if NUMERIC.contains(&key) {
            integer(value, key)?;
        }
        if !(is_scalar(key) || REPEATABLE.contains(&key)) {
            warnings.push(format!(
                "{} is preserved, but its value and firmware support are not validated.",
                line.key
            ));
        }
        if key == "dtoverlay" && !required_overlays.contains(value) {
            warnings.push("Additional overlays are preserved, but file availability and Windows compatibility are not validated.".into());
        }
        if ["dtparam", "gpio"].contains(&key) {
            warnings.push(format!(
                "{key} commands are preserved; their values and hardware effect are not validated."
            ));
        }
        if !line.unconditional {
            warnings.push("Conditional sections are preserved; their hardware-dependent effect cannot be verified here.".into());
        }
        if model == "5" && key.starts_with("hdmi_") {
            warnings.push("Legacy HDMI timing overrides are not verified for Pi 5 UEFI; firmware or Windows may ignore them.".into());
        }
    }
    let blocks: BTreeSet<_> = lines.iter().map(|line| line.block).collect();
    for block in blocks {
        let mut values = Map::new();
        for line in lines {
            if line.block == block && HDMI_KEYS.contains(&line.key.as_str()) {
                values.insert(line.key.clone(), json!(line.value));
            }
        }
        let group = text_value(&values, "hdmi_group")
            .map(|value| integer(value, "hdmi_group"))
            .transpose()?;
        if group.as_deref() == Some("2")
            && let Some(mode) = text_value(&values, "hdmi_mode")
            && !at_most(&integer(mode, "hdmi_mode")?, 87)
        {
            return Err(error("DMT hdmi_mode must be in the documented 0-87 range."));
        }
        if group.as_deref() == Some("2")
            && matches!(text_value(&values, "hdmi_mode"), Some("87" | "0x57"))
            && !lines
                .iter()
                .any(|line| line.key == "hdmi_cvt" && (line.block == block || line.unconditional))
        {
            warnings.push("DMT mode 87 needs a custom timing definition; display compatibility is not verified.".into());
        }
    }
    Ok(())
}

fn choice(key: &str, label: &str, value: Value, options: &[(&str, &str)], disabled: bool) -> Value {
    let mut choices = Vec::new();
    if !options.iter().any(|(item, _)| value == *item) {
        choices.push(json!({
            "value": value,
            "label": format!("Current custom value ({})", python_str(&value))
        }));
    }
    choices.extend(
        options
            .iter()
            .map(|(item, title)| json!({"value": item, "label": title})),
    );
    json!({
        "key": key, "label": label, "type": "choice", "value": value,
        "disabled": disabled, "choices": choices
    })
}

fn conditional_keys(lines: &[Line]) -> BTreeSet<String> {
    lines
        .iter()
        .filter(|line| !line.key.is_empty() && (!line.unconditional || line.key.contains(':')))
        .map(|line| base_key(&line.key).to_owned())
        .collect()
}

fn hdmi_values(values: &Map<String, Value>) -> Map<String, Value> {
    values
        .iter()
        .filter(|(key, _)| HDMI_KEYS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn unique_warnings(warnings: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    warnings
        .into_iter()
        .filter(|warning| seen.insert(warning.clone()))
        .collect()
}

fn desktop(request: &Value) -> Result<Value, ConfigError> {
    let model = request["model"].as_str().unwrap_or("");
    if !["3", "4", "5"].contains(&model) {
        return Err(error("Choose a supported Raspberry Pi model (3, 4 or 5)."));
    }
    let (Some(official), Some(recommended)) = (
        request["official"].as_str(),
        request["recommended"].as_str(),
    ) else {
        return Err(error(
            "The shipped boot defaults are missing; no settings were changed.",
        ));
    };
    if official.trim().is_empty() {
        return Err(error(
            "The shipped boot defaults are missing; no settings were changed.",
        ));
    }
    let mut warnings = Vec::new();
    let baseline = parse(official, &mut Vec::new())?;
    let baseline_values = scalar_values(&baseline);
    let mut text = request
        .get("config")
        .cloned()
        .unwrap_or_else(|| json!(official));
    if let Some(previous_defaults) = request
        .get("previousDefaults")
        .filter(|value| !value.is_null())
    {
        if !previous_defaults.is_object() {
            return Err(error("Previous model defaults are missing."));
        }
        let previous = desktop(&json!({
            "model": previous_defaults["model"], "official": previous_defaults["official"],
            "recommended": previous_defaults["recommended"],
            "hdmiChoices": previous_defaults["hdmiChoices"], "config": text
        }))?;
        warnings.extend(
            previous["warnings"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned),
        );
        let old_overlays: HashSet<_> = previous["protected"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter_map(|item| item.strip_prefix("dtoverlay="))
            .collect();
        let mut removed_overlays = HashSet::new();
        let mut retained = Vec::new();
        for line in parse_value(&previous["config"], &mut Vec::new())? {
            if PROTECTED.contains(&base_key(&line.key)) {
                continue;
            }
            if line.key == "dtoverlay"
                && line.unconditional
                && old_overlays.contains(line.value.as_str())
                && removed_overlays.insert(line.value.clone())
            {
                continue;
            }
            retained.push(line.raw);
        }
        text = json!(format!("{official}\n{}", retained.join("\n")));
    }
    let mut lines = parse_value(&text, &mut warnings)?;
    let mut original = scalar_values(&lines);
    let required: Map<_, _> = baseline_values
        .iter()
        .filter(|(key, _)| PROTECTED.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    for (key, expected) in &required {
        let expected = expected.as_str().unwrap_or("");
        if !lines
            .iter()
            .any(|line| base_key(&line.key) == key && line.unconditional && line.key == *key)
        {
            return Err(error(format!(
                "Required boot setting {key}={expected} is missing. Restore this model's shipped setting."
            )));
        }
        if !required_values_match(&lines, key, expected)? {
            return Err(error(format!(
                "{key} is required for this UEFI boot route and must remain {expected}."
            )));
        }
    }
    let required_overlays: Vec<_> = baseline
        .iter()
        .filter(|line| line.key == "dtoverlay")
        .map(|line| line.value.clone())
        .collect();
    for value in &required_overlays {
        if !lines
            .iter()
            .any(|line| line.key == "dtoverlay" && line.unconditional && line.value == *value)
        {
            return Err(error(format!(
                "Required boot overlay dtoverlay={value} is missing."
            )));
        }
    }
    let mut supported: BTreeSet<String> = BOOLEANS
        .iter()
        .map(|(key, _)| *key)
        .filter(|key| baseline_values.contains_key(*key) || original.contains_key(*key))
        .map(str::to_owned)
        .collect();
    for key in ["hdmi_group", "hdmi_mode", "hdmi_drive"] {
        if original.contains_key(key) {
            supported.insert(key.into());
        }
    }
    if model == "3" {
        supported.insert("core_freq".into());
    }
    for key in &supported {
        if let Some(value) = text_value(&original, key) {
            let value = integer(value, key)?;
            original.insert(key.clone(), json!(value));
        }
    }
    let mut conditional = conditional_keys(&lines);
    let mut mode = request
        .get("hdmiMode")
        .filter(|value| !value.is_null())
        .cloned();
    if request["reset"] == true {
        for key in &supported {
            if !conditional.contains(key) {
                original.insert(
                    key.clone(),
                    baseline_values
                        .get(key)
                        .cloned()
                        .unwrap_or_else(|| json!("")),
                );
            }
        }
        let default_hdmi = hdmi_values(&scalar_values(&parse(recommended, &mut Vec::new())?));
        mode = Some(json!(if default_hdmi.is_empty() {
            "official"
        } else {
            "recommended"
        }));
    }
    let empty = Map::new();
    let changes = match request.get("values") {
        None => &empty,
        Some(value) => value
            .as_object()
            .ok_or_else(|| error("The editor contains an unsupported control."))?,
    };
    if changes.keys().any(|key| !supported.contains(key)) {
        return Err(error("The editor contains an unsupported control."));
    }
    for (key, value) in changes {
        if conditional.contains(key) {
            if python_str(value) != text_value(&original, key).unwrap_or("") {
                return Err(error(format!(
                    "{key} is controlled by advanced conditional settings; edit those first."
                )));
            }
            continue;
        }
        if !value.is_string()
            || value
                .as_str()
                .is_some_and(|value| value.contains(['\n', '\r']))
        {
            return Err(error(format!("Invalid value for {key}.")));
        }
        original.insert(key.clone(), value.clone());
    }
    let mode_name = mode.as_ref().and_then(Value::as_str);
    if mode.is_some() && !matches!(mode_name, Some("official" | "recommended" | "custom")) {
        return Err(error(
            "Choose official, recommended or custom HDMI settings.",
        ));
    }
    let preset_selected = matches!(mode_name, Some("official" | "recommended"));
    if preset_selected {
        if conditional
            .iter()
            .any(|key| HDMI_KEYS.contains(&key.as_str()))
        {
            return Err(error(
                "HDMI has advanced conditional or per-port settings. Edit those before choosing a preset.",
            ));
        }
        if lines
            .iter()
            .any(|line| line.key.starts_with("hdmi_") && !HDMI_KEYS.contains(&line.key.as_str()))
        {
            return Err(error(
                "Additional advanced HDMI overrides are present. Edit those before choosing a preset.",
            ));
        }
        let preset = parse(
            if mode_name == Some("official") {
                official
            } else {
                recommended
            },
            &mut Vec::new(),
        )?;
        for key in HDMI_KEYS {
            original.shift_remove(*key);
        }
        original.extend(hdmi_values(&scalar_values(&preset)));
        lines.retain(|line| !HDMI_KEYS.contains(&line.key.as_str()));
        supported.retain(|key| !HDMI_KEYS.contains(&key.as_str()));
    }
    if let Some(user_text) = request.get("userText").filter(|value| !value.is_null()) {
        let user_text = user_text
            .as_str()
            .ok_or_else(|| error("User additions must be text."))?;
        let user_lines = parse(user_text, &mut warnings)?;
        if user_lines
            .iter()
            .any(|line| !line.key.is_empty() && required.contains_key(base_key(&line.key)))
        {
            return Err(error(
                "Required boot settings belong to the protected section, not user additions.",
            ));
        }
        lines.retain(|line| {
            required.contains_key(&line.key)
                || supported.contains(&line.key)
                    && line.unconditional
                    && !conditional.contains(&line.key)
                || line.key == "dtoverlay" && required_overlays.contains(&line.value)
        });
        for key in HDMI_KEYS {
            if !supported.contains(*key) {
                original.shift_remove(*key);
            }
        }
        let user_values = scalar_values(&user_lines);
        for key in user_values.keys() {
            if is_boolean(key) || ["hdmi_group", "hdmi_mode", "hdmi_drive"].contains(&key.as_str())
            {
                supported.insert(key.clone());
            }
        }
        original.extend(
            user_values
                .into_iter()
                .filter(|(key, _)| supported.contains(key) || HDMI_KEYS.contains(&key.as_str())),
        );
        lines.extend(user_lines);
        conditional = conditional_keys(&lines);
    }
    for key in &supported {
        if let Some(value) = text_value(&original, key).filter(|value| !value.is_empty()) {
            let value = integer(value, key)?;
            original.insert(key.clone(), json!(value));
        }
    }
    let recommended_hdmi = hdmi_values(&scalar_values(&parse(recommended, &mut Vec::new())?));
    let current_hdmi = hdmi_values(&original);
    let conditional_hdmi = conditional
        .iter()
        .any(|key| HDMI_KEYS.contains(&key.as_str()));
    let managed_hdmi = preset_selected
        || !recommended_hdmi.is_empty() && current_hdmi == recommended_hdmi && !conditional_hdmi;
    let mut managed_keys = supported.clone();
    if managed_hdmi {
        managed_keys.extend(HDMI_KEYS.iter().map(|key| (*key).to_owned()));
    }
    managed_keys.retain(|key| !conditional.contains(key));
    let mut managed = vec!["[all]".to_owned()];
    for line in &baseline {
        if let Some(value) = text_value(&required, &line.key) {
            managed.push(format!("{}={value}", line.key));
        } else if line.key == "dtoverlay" && required_overlays.contains(&line.value) {
            managed.push(format!("dtoverlay={}", line.value));
        }
    }
    for (key, value) in &original {
        if !required.contains_key(key)
            && managed_keys.contains(key)
            && let Some(value) = value.as_str().filter(|value| !value.is_empty())
        {
            managed.push(format!("{key}={value}"));
        }
    }
    let mut additions: Vec<String> = Vec::new();
    let mut consumed_overlays = HashSet::new();
    for line in &lines {
        if line.key == "dtoverlay"
            && line.unconditional
            && required_overlays.contains(&line.value)
            && consumed_overlays.insert(line.value.clone())
        {
            if let Some((_, comment)) = line.raw.split_once('#') {
                additions.push(format!("#{comment}"));
            }
            continue;
        }
        if line.unconditional
            && (required.contains_key(&line.key) || managed_keys.contains(&line.key))
        {
            if let Some((_, comment)) = line.raw.split_once('#') {
                additions.push(format!("#{comment}"));
            }
            continue;
        }
        if line.raw.trim() == "[all]"
            && !additions
                .iter()
                .any(|item| !item.trim().is_empty() && !item.trim_start().starts_with('#'))
        {
            continue;
        }
        additions.push(line.raw.clone());
    }
    let user = additions.join("\n").trim_matches('\n').to_owned();
    let mut output = vec![MANAGED_START.to_owned(), MANAGED_NOTE.to_owned()];
    output.extend(managed);
    output.extend([
        MANAGED_END.to_owned(),
        String::new(),
        USER_HEADER.to_owned(),
        "[all]".into(),
        user.clone(),
    ]);
    let config = format!("{}\n", output.join("\n").trim_end());
    let validated_lines = parse(&config, &mut Vec::new())?;
    check_values(&validated_lines, model, &mut warnings, &required_overlays)?;
    let mut fields = Vec::new();
    for (key, label) in BOOLEANS {
        if !supported.contains(*key) || text_value(&original, key) == Some("") {
            continue;
        }
        fields.push(json!({
            "key": key, "label": label, "type": "boolean",
            "value": original.get(*key).cloned().unwrap_or_else(|| json!("0")),
            "disabled": conditional.contains(*key)
        }));
    }
    for (key, label, options) in [
        (
            "hdmi_group",
            "HDMI timing standard",
            vec![
                ("", "Firmware default"),
                ("0", "Auto"),
                ("1", "CEA (TV)"),
                ("2", "DMT (monitor)"),
            ],
        ),
        (
            "hdmi_drive",
            "HDMI output",
            vec![
                ("", "Firmware default"),
                ("1", "DVI (no audio)"),
                ("2", "HDMI (with audio)"),
            ],
        ),
    ] {
        if supported.contains(key) {
            fields.push(choice(
                key,
                label,
                original.get(key).cloned().unwrap_or_else(|| json!("")),
                &options,
                conditional.contains(key),
            ));
        }
    }
    if supported.contains("hdmi_mode") {
        fields.push(json!({
            "key": "hdmi_mode", "label": "Custom HDMI mode number", "type": "number",
            "value": original.get("hdmi_mode").cloned().unwrap_or_else(|| json!("")),
            "min": 0, "max": if text_value(&original, "hdmi_group") == Some("2") { 87 } else { 107 },
            "disabled": conditional.contains("hdmi_mode")
        }));
    }
    if supported.contains("core_freq") {
        fields.push(choice(
            "core_freq",
            "UART core clock",
            original
                .get("core_freq")
                .cloned()
                .unwrap_or_else(|| json!("")),
            &[
                ("", "Firmware default"),
                ("250", "250 MHz (UART troubleshooting)"),
            ],
            conditional.contains("core_freq"),
        ));
    }
    if !conditional.is_disjoint(&supported) {
        warnings.push("Some controls are read-only because advanced conditional or per-port settings also set them.".into());
    }
    let hdmi = hdmi_values(&scalar_values(&validated_lines));
    let other_hdmi = validated_lines
        .iter()
        .any(|line| line.key.starts_with("hdmi_") && !HDMI_KEYS.contains(&line.key.as_str()));
    let effective_mode = if conditional_hdmi || other_hdmi {
        "custom"
    } else if hdmi.is_empty() {
        "official"
    } else if !recommended_hdmi.is_empty() && hdmi == recommended_hdmi {
        "recommended"
    } else {
        "custom"
    };
    let choices = request.get("hdmiChoices").and_then(Value::as_array);
    if choices.is_none_or(|choices| {
        choices.is_empty()
            || choices.iter().any(|item| {
                !item.is_object()
                    || !matches!(
                        item["value"].as_str(),
                        Some("official" | "recommended" | "custom")
                    )
                    || !item["label"].is_string()
            })
    }) {
        return Err(error(
            "The shared HDMI choices are missing from the runtime.",
        ));
    }
    let mut protected: Vec<_> = required
        .iter()
        .map(|(key, value)| format!("{key}={}", value.as_str().unwrap_or("")))
        .collect();
    protected.extend(
        required_overlays
            .iter()
            .map(|value| format!("dtoverlay={value}")),
    );
    Ok(json!({
        "config": config, "userText": user, "fields": fields, "hdmiMode": effective_mode,
        "hdmiChoices": request["hdmiChoices"], "warnings": unique_warnings(warnings),
        "protected": protected
    }))
}

fn iot_key(key: &str) -> bool {
    HDMI_KEYS.contains(&key) && key != "hdmi_cvt"
}

fn validate_iot_hdmi(mode: &Value, config: &str) -> Result<(), ConfigError> {
    let Some(mode) = mode
        .as_str()
        .filter(|mode| ["official", "720p60", "1080p60", "custom"].contains(mode))
    else {
        return Err(error(
            "unknown HDMI mode; choose official, 720p60, 1080p60 or custom",
        ));
    };
    if config.len() > 512
        || !config.is_ascii()
        || config
            .chars()
            .any(|ch| ch < ' ' && !['\t', '\r', '\n'].contains(&ch))
    {
        return Err(error(
            "custom HDMI settings must be at most 512 ASCII characters",
        ));
    }
    if mode != "custom" {
        return Ok(());
    }
    let config = config.replace("\r\n", "\n").replace('\r', "\n");
    let mut values = Map::new();
    for raw in split_lines(&config) {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let (key, value) = assignment(line)
            .filter(|(key, value)| iot_key(key) && !value.is_empty() && value.bytes().all(|ch| ch.is_ascii_digit()))
            .ok_or_else(|| error("custom HDMI settings accept only hdmi_force_hotplug, hdmi_group, hdmi_mode and hdmi_drive"))?;
        if values.contains_key(key) {
            return Err(error(format!("duplicate HDMI setting: {key}")));
        }
        values.insert(key.into(), json!(integer(value, key)?));
    }
    if !matches!(text_value(&values, "hdmi_group"), Some("1" | "2"))
        || !values.contains_key("hdmi_mode")
    {
        return Err(error(
            "custom HDMI settings require hdmi_group=1 or 2 and hdmi_mode",
        ));
    }
    let maximum = if text_value(&values, "hdmi_group") == Some("1") {
        64
    } else {
        86
    };
    if text_value(&values, "hdmi_mode").is_none_or(|value| value == "0" || !at_most(value, maximum))
    {
        return Err(error(
            "HDMI mode is outside the legacy Pi 2/3 CEA/DMT range; custom modelines are not supported",
        ));
    }
    if text_value(&values, "hdmi_force_hotplug").is_some_and(|value| !["0", "1"].contains(&value)) {
        return Err(error("hdmi_force_hotplug must be 0 or 1"));
    }
    if text_value(&values, "hdmi_drive").is_some_and(|value| !["1", "2"].contains(&value)) {
        return Err(error("hdmi_drive must be 1 (DVI) or 2 (HDMI)"));
    }
    Ok(())
}

fn iot(request: &Value) -> Result<Value, ConfigError> {
    let mut warnings = Vec::new();
    let empty_text = json!("");
    let text = request.get("config").unwrap_or(&empty_text);
    let lines = parse_value(text, &mut warnings)?;
    if lines
        .iter()
        .any(|line| !line.key.is_empty() && (!iot_key(&line.key) || !line.unconditional))
    {
        return Err(error(
            "IoT permits only hdmi_force_hotplug, hdmi_group, hdmi_mode and hdmi_drive. Boot entries stay protected.",
        ));
    }
    let mut values = scalar_values(&lines);
    let empty = Map::new();
    let changes = match request.get("values") {
        None => &empty,
        Some(value) => value
            .as_object()
            .ok_or_else(|| error("The IoT editor contains an unsupported control."))?,
    };
    if changes.keys().any(|key| !iot_key(key)) {
        return Err(error("The IoT editor contains an unsupported control."));
    }
    values.extend(changes.clone());
    if let Some(user_text) = request.get("userText") {
        let user_lines = parse_value(user_text, &mut warnings)?;
        if user_lines
            .iter()
            .any(|line| !line.key.is_empty() && !iot_key(&line.key))
        {
            return Err(error("IoT advanced editing remains HDMI-only."));
        }
        values = scalar_values(&user_lines);
    }
    let mut config: String = values
        .iter()
        .filter(|(_, value)| value.as_str() != Some(""))
        .map(|(key, value)| format!("{key}={}\n", python_str(value)))
        .collect();
    let mode = if request["reset"] == true {
        json!("720p60")
    } else {
        request
            .get("hdmiMode")
            .cloned()
            .unwrap_or_else(|| json!("custom"))
    };
    validate_iot_hdmi(&mode, &config)?;
    if mode != "custom" {
        config = if mode == "1080p60" {
            IOT_1080P
        } else {
            IOT_720P
        }
        .into();
        values = scalar_values(&parse(&config, &mut Vec::new())?);
    }
    let fields = vec![
        choice(
            "hdmi_force_hotplug",
            BOOLEANS[6].1,
            values
                .get("hdmi_force_hotplug")
                .cloned()
                .unwrap_or_else(|| json!("")),
            &[("", "Firmware default"), ("1", "On"), ("0", "Off")],
            false,
        ),
        choice(
            "hdmi_group",
            "HDMI timing standard",
            values
                .get("hdmi_group")
                .cloned()
                .unwrap_or_else(|| json!("1")),
            &[("1", "CEA (TV)"), ("2", "DMT (monitor)")],
            false,
        ),
        json!({
            "key": "hdmi_mode", "label": "HDMI mode number", "type": "number",
            "value": values.get("hdmi_mode").cloned().unwrap_or_else(|| json!("4")),
            "min": 1, "max": if values.get("hdmi_group").is_none_or(|value| value == "1") { 64 } else { 86 },
            "disabled": false
        }),
        choice(
            "hdmi_drive",
            "HDMI output",
            values
                .get("hdmi_drive")
                .cloned()
                .unwrap_or_else(|| json!("")),
            &[
                ("", "Firmware default"),
                ("1", "DVI (no audio)"),
                ("2", "HDMI (with audio)"),
            ],
            false,
        ),
    ];
    Ok(json!({
        "config": config, "userText": config, "fields": fields, "hdmiMode": mode,
        "hdmiChoices": [
            {"value": "official", "label": "Official image default"},
            {"value": "720p60", "label": "1280 x 720 / 60 Hz (compatibility) (Recommended)"},
            {"value": "1080p60", "label": "1920 x 1080 / 60 Hz"},
            {"value": "custom", "label": "Custom video settings"}
        ],
        "warnings": unique_warnings(warnings),
        "protected": ["All non-HDMI IoT boot settings (unchanged in the verified FFU)"]
    }))
}

fn python_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut output = String::from(quote);
    for ch in text.chars() {
        match ch {
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            ch if ch == quote => {
                output.push('\\');
                output.push(ch);
            }
            ch if ch.is_control() || ['\u{2028}', '\u{2029}'].contains(&ch) => {
                if (ch as u32) < 256 {
                    output.push_str(&format!("\\x{:02x}", ch as u32));
                } else {
                    output.push_str(&format!("\\u{:04x}", ch as u32));
                }
            }
            ch => output.push(ch),
        }
    }
    output.push(quote);
    output
}

fn python_str(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(number) => number.to_string(),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(python_literal)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| format!("{}: {}", python_repr(key), python_literal(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn python_literal(value: &Value) -> String {
    match value {
        Value::String(text) => python_repr(text),
        _ => python_str(value),
    }
}

/// Process the unchanged Python helper's settings object.
pub fn process(request: &Value) -> Result<Value, ConfigError> {
    if !request.is_object() {
        return Err(error("Expected a boot-editor settings object."));
    }
    match request.get("profile") {
        None => desktop(request),
        Some(Value::String(profile)) if profile == "desktop" => desktop(request),
        Some(Value::String(profile)) if profile == "iot-core" => iot(request),
        _ => Err(error("Unknown boot-configuration profile.")),
    }
}

/// Decode a bounded stdin document without accessing transports or devices.
pub fn process_json(input: &str) -> Result<Value, ConfigError> {
    if input.chars().count() > MAX_INPUT_CHARACTERS {
        return Err(error("Boot-editor input is too large."));
    }
    let request = serde_json::from_str(input).map_err(|failure| json_error(input, &failure))?;
    process(&request)
}

fn json_error(input: &str, failure: &serde_json::Error) -> ConfigError {
    if input.starts_with('\u{feff}') {
        return error("Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)");
    }
    //Serde owns decoding; this scan preserves Python's diagnostic wording and positions.
    let mut syntax = JsonSyntax {
        bytes: input.as_bytes(),
        position: 0,
    };
    let (reason, byte) = syntax.document().err().unwrap_or_else(|| {
        let line_start = if failure.line() > 1 {
            input
                .match_indices('\n')
                .nth(failure.line() - 2)
                .map_or(0, |(index, _)| index + 1)
        } else {
            0
        };
        (
            "Expecting value",
            (line_start + failure.column()).min(input.len()),
        )
    });
    let byte = (0..=byte)
        .rev()
        .find(|index| input.is_char_boundary(*index))
        .unwrap_or(0);
    let prefix = &input[..byte];
    let line = prefix.bytes().filter(|ch| *ch == b'\n').count() + 1;
    let column = prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    let position = prefix.chars().count();
    error(format!(
        "{reason}: line {line} column {column} (char {position})"
    ))
}

type JsonDiagnostic = (&'static str, usize);

struct JsonSyntax<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl JsonSyntax<'_> {
    fn whitespace(&mut self) {
        while self
            .bytes
            .get(self.position)
            .is_some_and(|byte| b" \t\r\n".contains(byte))
        {
            self.position += 1;
        }
    }

    fn document(&mut self) -> Result<(), JsonDiagnostic> {
        self.value(0)?;
        self.whitespace();
        if self.position != self.bytes.len() {
            return Err(("Extra data", self.position));
        }
        Ok(())
    }

    fn value(&mut self, depth: usize) -> Result<(), JsonDiagnostic> {
        self.whitespace();
        let start = self.position;
        if depth > 128 {
            return Err(("Expecting value", start));
        }
        match self.bytes.get(start) {
            Some(b'"') => self.string(),
            Some(b'{') => self.object(depth + 1),
            Some(b'[') => self.array(depth + 1),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => {
                for literal in [b"true".as_slice(), b"false", b"null"] {
                    if self.bytes[start..].starts_with(literal) {
                        self.position += literal.len();
                        return Ok(());
                    }
                }
                Err(("Expecting value", start))
            }
        }
    }

    fn string(&mut self) -> Result<(), JsonDiagnostic> {
        let start = self.position;
        self.position += 1;
        while let Some(byte) = self.bytes.get(self.position).copied() {
            match byte {
                b'"' => {
                    self.position += 1;
                    return Ok(());
                }
                b'\\' => {
                    let escape = self.position;
                    self.position += 1;
                    match self.bytes.get(self.position) {
                        Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => {
                            self.position += 1;
                        }
                        Some(b'u') => {
                            self.position += 1;
                            if self
                                .bytes
                                .get(self.position..self.position + 4)
                                .is_none_or(|digits| !digits.iter().all(u8::is_ascii_hexdigit))
                            {
                                return Err(("Invalid \\uXXXX escape", escape + 1));
                            }
                            self.position += 4;
                        }
                        None => return Err(("Unterminated string starting at", start)),
                        _ => return Err(("Invalid \\escape", escape)),
                    }
                }
                0..=31 => return Err(("Invalid control character at", self.position)),
                _ => self.position += 1,
            }
        }
        Err(("Unterminated string starting at", start))
    }

    fn object(&mut self, depth: usize) -> Result<(), JsonDiagnostic> {
        self.position += 1;
        self.whitespace();
        if self.bytes.get(self.position) == Some(&b'}') {
            self.position += 1;
            return Ok(());
        }
        loop {
            if self.bytes.get(self.position) != Some(&b'"') {
                return Err((
                    "Expecting property name enclosed in double quotes",
                    self.position,
                ));
            }
            self.string()?;
            self.whitespace();
            if self.bytes.get(self.position) != Some(&b':') {
                return Err(("Expecting ':' delimiter", self.position));
            }
            self.position += 1;
            self.value(depth)?;
            self.whitespace();
            match self.bytes.get(self.position) {
                Some(b'}') => {
                    self.position += 1;
                    return Ok(());
                }
                Some(b',') => {
                    self.position += 1;
                    self.whitespace();
                }
                _ => return Err(("Expecting ',' delimiter", self.position)),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<(), JsonDiagnostic> {
        self.position += 1;
        self.whitespace();
        if self.bytes.get(self.position) == Some(&b']') {
            self.position += 1;
            return Ok(());
        }
        loop {
            self.value(depth)?;
            self.whitespace();
            match self.bytes.get(self.position) {
                Some(b']') => {
                    self.position += 1;
                    return Ok(());
                }
                Some(b',') => self.position += 1,
                _ => return Err(("Expecting ',' delimiter", self.position)),
            }
        }
    }

    fn digits(&mut self) {
        while self
            .bytes
            .get(self.position)
            .is_some_and(u8::is_ascii_digit)
        {
            self.position += 1;
        }
    }

    fn number(&mut self) -> Result<(), JsonDiagnostic> {
        let start = self.position;
        if self.bytes.get(self.position) == Some(&b'-') {
            self.position += 1;
        }
        match self.bytes.get(self.position) {
            Some(b'0') => self.position += 1,
            Some(b'1'..=b'9') => self.digits(),
            _ => return Err(("Expecting value", start)),
        }
        if self.bytes.get(self.position) == Some(&b'.')
            && self
                .bytes
                .get(self.position + 1)
                .is_some_and(u8::is_ascii_digit)
        {
            self.position += 1;
            self.digits();
        }
        if matches!(self.bytes.get(self.position), Some(b'e' | b'E')) {
            let exponent = self.position;
            self.position += 1;
            if matches!(self.bytes.get(self.position), Some(b'+' | b'-')) {
                self.position += 1;
            }
            if self
                .bytes
                .get(self.position)
                .is_some_and(u8::is_ascii_digit)
            {
                self.digits();
            } else {
                self.position = exponent;
            }
        }
        Ok(())
    }
}
