#!/usr/bin/env python3
"""Loss-aware config.txt editing; no image, network, privilege or device access."""

from __future__ import annotations

import importlib.util
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

MANAGED_START = "# >>> WoR-Flasher managed boot settings >>>"
MANAGED_END = "# <<< WoR-Flasher managed boot settings <<<"
USER_HEADER = "# User additions - use Advanced Text Edit (Not Recommended)"
MANAGED_NOTE = "# Required boot entries are protected; use Boot Config Editor for supported settings."
LEGACY_HEADER = "# don't change anything below this point #"
MAX_CONFIG_BYTES = 64 * 1024
PROTECTED = {
    "arm_64bit", "armstub", "device_tree_address", "device_tree_end",
    "disable_commandline_tags", "enable_gic", "pciex4_reset", "framebuffer_depth",
}
REPEATABLE = {"dtoverlay", "dtparam", "gpio"}
HDMI_KEYS = {"hdmi_force_hotplug", "hdmi_group", "hdmi_mode", "hdmi_drive", "hdmi_cvt"}
BOOLEANS = {
    "disable_overscan": "Remove display borders (overscan)",
    "enable_uart": "Enable serial console",
    "uart_2ndstage": "Show early boot serial messages",
    "arm_boost": "Enable normal CPU boost",
    "usb_max_current_enable": "Allow higher USB power (requires a suitable supply)",
    "force_turbo": "Keep maximum clock speed (uses more power)",
    "hdmi_force_hotplug": "Use HDMI even when no display is detected",
}
KNOWN_BOOLEANS = set(BOOLEANS) | {
    "arm_64bit", "enable_gic", "hdmi_ignore_hotplug", "hdmi_safe",
    "disable_splash",
}
SCALARS = PROTECTED | KNOWN_BOOLEANS | HDMI_KEYS | {
    "core_freq", "arm_freq", "gpu_mem", "boot_delay", "boot_delay_ms", "avoid_warnings",
}
ASSIGNMENT = re.compile(r"([A-Za-z][A-Za-z0-9_]*(?::[01])?)\s*=\s*(.*)")


class ConfigError(ValueError):
    pass


@dataclass(frozen=True)
class Line:
    raw: str
    number: int
    block: int
    unconditional: bool
    key: str = ""
    value: str = ""


def parse(text: str, warnings: list[str]) -> list[Line]:
    if not isinstance(text, str) or len(text.encode("utf-8")) > MAX_CONFIG_BYTES:
        raise ConfigError("config.txt must be text of at most 64 KiB.")
    if any(ord(char) < 32 and char not in "\r\n\t" for char in text):
        raise ConfigError("config.txt contains a NUL or another invalid control character.")
    if text.startswith("\ufeff"):
        text = text[1:]
        warnings.append("Removed a UTF-8 byte-order mark from the start of config.txt.")
    lines: list[Line] = []
    block = 0
    unconditional = True
    markers: list[str] = []
    for number, raw in enumerate(text.replace("\r\n", "\n").replace("\r", "\n").splitlines(), 1):
        stripped = raw.strip()
        if stripped in (MANAGED_START, MANAGED_END):
            markers.append(stripped)
            continue
        if stripped in (LEGACY_HEADER, USER_HEADER, MANAGED_NOTE):
            continue
        code = raw.split("#", 1)[0].strip()
        if not code:
            lines.append(Line(raw, number, block, unconditional))
            continue
        if len(raw) > 98:
            raise ConfigError(f"Line {number}: firmware ignores characters past the 98-character limit.")
        if not code.isascii():
            raise ConfigError(f"Line {number}: firmware settings must use ASCII characters.")
        if code.startswith("["):
            if not re.fullmatch(r"\[[^\[\]]+\]", code):
                raise ConfigError(f"Line {number}: malformed conditional section.")
            block += 1
            unconditional = code == "[all]"
            lines.append(Line(raw, number, block, unconditional))
            continue
        if code.startswith("include "):
            raise ConfigError(f"Line {number}: include files cannot be verified or copied by this editor. Keep settings in this file.")
        match = ASSIGNMENT.fullmatch(code)
        if match is None:
            raise ConfigError(f"Line {number}: expected key=value, a comment or a conditional section.")
        key, value = match.groups()
        if not value.strip():
            raise ConfigError(f"Line {number}: {key} has no value; remove the line to use firmware defaults.")
        lines.append(Line(raw, number, block, unconditional, key, value.strip()))
    if markers and markers != [MANAGED_START, MANAGED_END]:
        raise ConfigError("The managed boot-settings markers are incomplete, duplicated or out of order.")

    #Only scalar assignments within the same contiguous filter block are redundant.
    #Overlays, dtparam and GPIO commands are sequences, not dictionary entries.
    last: dict[tuple[int, str], int] = {}
    for index, line in enumerate(lines):
        if line.key and line.key.split(":")[0] in SCALARS:
            last[line.block, line.key] = index
    normalized: list[Line] = []
    for index, line in enumerate(lines):
        if line.key and line.key.split(":")[0] in SCALARS and last[line.block, line.key] != index:
            warnings.append(f"Removed duplicate {line.key} on line {line.number}; kept the last value in that section.")
            if "#" in line.raw:
                normalized.append(Line("#" + line.raw.split("#", 1)[1], line.number, line.block, line.unconditional))
        else:
            normalized.append(line)
    return normalized


def scalar_values(lines: list[Line]) -> dict[str, str]:
    return {line.key: line.value for line in lines if line.key and line.unconditional and line.key not in REPEATABLE}


def integer(value: str, key: str) -> int:
    if not re.fullmatch(r"(?:0x[0-9a-fA-F]+|[0-9]+)", value):
        raise ConfigError(f"{key} must be an integer, not {value!r}.")
    return int(value, 16 if value.startswith("0x") else 10)


def same_value(key: str, value: str, expected: str) -> bool:
    if key == "armstub" or key == "pciex4_reset":
        return value == expected
    return integer(value, key) == integer(expected, key)


def check_values(lines: list[Line], model: str, warnings: list[str], required_overlays: list[str]) -> None:
    known = PROTECTED | REPEATABLE | set(BOOLEANS) | HDMI_KEYS | {
        "core_freq", "arm_freq", "gpu_mem", "boot_delay", "boot_delay_ms",
        "disable_splash", "avoid_warnings", "hdmi_ignore_hotplug", "hdmi_safe",
    }
    for line in lines:
        if not line.key:
            continue
        key = line.key.split(":")[0]
        value = line.value
        if key in KNOWN_BOOLEANS and integer(value, key) not in (0, 1):
            raise ConfigError(f"Line {line.number}: {key} must be 0 (off) or 1 (on).")
        if key == "hdmi_group" and integer(value, key) not in (0, 1, 2):
            raise ConfigError(f"Line {line.number}: hdmi_group must be 0 (auto), 1 (CEA) or 2 (DMT).")
        if key == "hdmi_drive" and integer(value, key) not in (1, 2):
            raise ConfigError(f"Line {line.number}: hdmi_drive must be 1 (DVI) or 2 (HDMI).")
        if key == "hdmi_mode" and not 0 <= integer(value, key) <= 107:
            raise ConfigError(f"Line {line.number}: hdmi_mode is outside the documented 0-107 range.")
        if key == "hdmi_cvt":
            parts = value.split()
            if not 3 <= len(parts) <= 7:
                raise ConfigError("hdmi_cvt needs width, height and refresh, followed by up to four timing options.")
            values = [integer(part, key) for part in parts]
            if any(number <= 0 for number in values[:3]):
                raise ConfigError("hdmi_cvt width, height and refresh must be positive.")
            if len(values) > 3 and values[3] not in range(1, 7):
                raise ConfigError("hdmi_cvt aspect must be 1-6.")
            if any(number not in (0, 1) for number in values[4:]):
                raise ConfigError("hdmi_cvt margins, interlace and reduced blanking must be 0 or 1.")
        if key in ("core_freq", "arm_freq", "gpu_mem", "boot_delay", "boot_delay_ms"):
            integer(value, key)
        if key not in known:
            warnings.append(f"{line.key} is preserved, but its value and firmware support are not validated.")
        if key == "dtoverlay" and value not in required_overlays:
            warnings.append("Additional overlays are preserved, but file availability and Windows compatibility are not validated.")
        if key in ("dtparam", "gpio"):
            warnings.append(f"{key} commands are preserved; their values and hardware effect are not validated.")
        if not line.unconditional:
            warnings.append("Conditional sections are preserved; their hardware-dependent effect cannot be verified here.")
        if model == "5" and key.startswith("hdmi_"):
            warnings.append("Legacy HDMI timing overrides are not verified for Pi 5 UEFI; firmware or Windows may ignore them.")
    #Mode 87 is custom DMT timing, not a second CEA mode table.
    for block in {line.block for line in lines}:
        values = {line.key: line.value for line in lines if line.block == block and line.key in HDMI_KEYS}
        group = integer(values["hdmi_group"], "hdmi_group") if "hdmi_group" in values else None
        if group == 2 and "hdmi_mode" in values and integer(values["hdmi_mode"], "hdmi_mode") > 87:
            raise ConfigError("DMT hdmi_mode must be in the documented 0-87 range.")
        if group == 2 and values.get("hdmi_mode") in ("87", "0x57") and not any(
            line.key == "hdmi_cvt" and (line.block == block or line.unconditional) for line in lines
        ):
            warnings.append("DMT mode 87 needs a custom timing definition; display compatibility is not verified.")


def choice(key: str, label: str, value: str, options: list[tuple[str, str]], disabled: bool = False) -> dict[str, Any]:
    if value not in {item[0] for item in options}:
        options = [(value, f"Current custom value ({value})")] + options
    return {
        "key": key, "label": label, "type": "choice", "value": value, "disabled": disabled,
        "choices": [{"value": item, "label": title} for item, title in options],
    }


def desktop(request: dict[str, Any]) -> dict[str, Any]:
    model = request.get("model")
    if model not in ("3", "4", "5"):
        raise ConfigError("Choose a supported Raspberry Pi model (3, 4 or 5).")
    official = request.get("official")
    recommended = request.get("recommended")
    if not isinstance(official, str) or not isinstance(recommended, str) or not official.strip():
        raise ConfigError("The shipped boot defaults are missing; no settings were changed.")
    warnings: list[str] = []
    baseline = parse(official, [])
    baseline_values = scalar_values(baseline)
    text = request.get("config", official)
    previous_defaults = request.get("previousDefaults")
    if previous_defaults is not None:
        if not isinstance(previous_defaults, dict):
            raise ConfigError("Previous model defaults are missing.")
        previous = desktop({
            "model": previous_defaults.get("model"), "official": previous_defaults.get("official"),
            "recommended": previous_defaults.get("recommended"), "hdmiChoices": previous_defaults.get("hdmiChoices"),
            "config": text,
        })
        warnings.extend(previous["warnings"])
        old_overlays = {item.split("=", 1)[1] for item in previous["protected"] if item.startswith("dtoverlay=")}
        removed_overlays: set[str] = set()
        retained: list[str] = []
        for line in parse(previous["config"], []):
            if line.key.split(":")[0] in PROTECTED:
                continue
            if line.key == "dtoverlay" and line.unconditional and line.value in old_overlays and line.value not in removed_overlays:
                removed_overlays.add(line.value)
                continue
            retained.append(line.raw)
        text = official + "\n" + "\n".join(retained)
    lines = parse(text, warnings)
    original = scalar_values(lines)
    required = {key: value for key, value in baseline_values.items() if key in PROTECTED}
    for key, expected in required.items():
        matches = [line for line in lines if line.key.split(":")[0] == key]
        if not any(line.unconditional and line.key == key for line in matches):
            raise ConfigError(f"Required boot setting {key}={expected} is missing. Restore this model's shipped setting.")
        if any(not same_value(key, line.value, expected) for line in matches):
            raise ConfigError(f"{key} is required for this UEFI boot route and must remain {expected}.")
    required_overlays = [line.value for line in baseline if line.key == "dtoverlay"]
    for value in required_overlays:
        if not any(line.key == "dtoverlay" and line.unconditional and line.value == value for line in lines):
            raise ConfigError(f"Required boot overlay dtoverlay={value} is missing.")

    supported = set(BOOLEANS) & (set(baseline_values) | set(original))
    supported |= {"hdmi_group", "hdmi_mode", "hdmi_drive"} & set(original)
    if model == "3":
        supported.add("core_freq")
    for key in supported:
        if key in original:
            original[key] = str(integer(original[key], key))
    conditional_keys = {line.key.split(":")[0] for line in lines if line.key and (not line.unconditional or ":" in line.key)}
    if request.get("reset") is True:
        for key in supported:
            if key not in conditional_keys:
                original[key] = baseline_values.get(key, "")
        default_hdmi = scalar_values(parse(recommended, []))
        request = {**request, "hdmiMode": "recommended" if HDMI_KEYS & default_hdmi.keys() else "official"}
    changes = request.get("values", {})
    if not isinstance(changes, dict) or any(key not in supported for key in changes):
        raise ConfigError("The editor contains an unsupported control.")
    for key, value in changes.items():
        if key in conditional_keys:
            if str(value) != original.get(key, ""):
                raise ConfigError(f"{key} is controlled by advanced conditional settings; edit those first.")
            continue
        if type(value) is not str or "\n" in value or "\r" in value:
            raise ConfigError(f"Invalid value for {key}.")
        original[key] = value

    mode = request.get("hdmiMode")
    if mode is not None and mode not in ("official", "recommended", "custom"):
        raise ConfigError("Choose official, recommended or custom HDMI settings.")
    if mode in ("official", "recommended"):
        if conditional_keys & HDMI_KEYS:
            raise ConfigError("HDMI has advanced conditional or per-port settings. Edit those before choosing a preset.")
        if any(line.key.startswith("hdmi_") and line.key not in HDMI_KEYS for line in lines):
            raise ConfigError("Additional advanced HDMI overrides are present. Edit those before choosing a preset.")
        preset = parse(official if mode == "official" else recommended, [])
        for key in HDMI_KEYS:
            original.pop(key, None)
        original.update({key: value for key, value in scalar_values(preset).items() if key in HDMI_KEYS})
        lines = [line for line in lines if line.key not in HDMI_KEYS]
        supported -= HDMI_KEYS

    user_text = request.get("userText")
    if user_text is not None:
        if not isinstance(user_text, str):
            raise ConfigError("User additions must be text.")
        user_lines = parse(user_text, warnings)
        if any(line.key.split(":")[0] in required for line in user_lines if line.key):
            raise ConfigError("Required boot settings belong to the protected section, not user additions.")
        lines = [line for line in lines if line.key in required or line.key in supported or (
            line.key == "dtoverlay" and line.value in required_overlays
        )]
        #User additions are last in the actual file, so an explicitly added supported scalar wins.
        for key in HDMI_KEYS - supported:
            original.pop(key, None)
        supported |= (set(BOOLEANS) | {"hdmi_group", "hdmi_mode", "hdmi_drive"}) & set(scalar_values(user_lines))
        original.update({key: value for key, value in scalar_values(user_lines).items() if key in supported or key in HDMI_KEYS})
        lines += user_lines
        conditional_keys = {line.key.split(":")[0] for line in lines if line.key and (not line.unconditional or ":" in line.key)}

    for key in supported:
        if original.get(key):
            original[key] = str(integer(original[key], key))
    recommended_hdmi = {key: value for key, value in scalar_values(parse(recommended, [])).items() if key in HDMI_KEYS}
    current_hdmi = {key: value for key, value in original.items() if key in HDMI_KEYS}
    managed_hdmi = mode in ("official", "recommended") or (
        bool(recommended_hdmi) and current_hdmi == recommended_hdmi and not conditional_keys & HDMI_KEYS
    )
    managed: list[str] = ["[all]"]
    for line in baseline:
        if line.key in required:
            managed.append(f"{line.key}={required[line.key]}")
        elif line.key == "dtoverlay" and line.value in required_overlays:
            managed.append(f"dtoverlay={line.value}")
    for key, value in original.items():
        if key in required:
            continue
        if key in supported or (key in HDMI_KEYS and managed_hdmi):
            if value:
                managed.append(f"{key}={value}")

    additions: list[str] = []
    consumed_overlays: set[str] = set()
    for line in lines:
        if line.key == "dtoverlay" and line.unconditional and line.value in required_overlays:
            if line.value not in consumed_overlays:
                consumed_overlays.add(line.value)
                if "#" in line.raw:
                    additions.append("#" + line.raw.split("#", 1)[1])
                continue
        if line.unconditional and (
            line.key in required or line.key in supported
            or (line.key in HDMI_KEYS and managed_hdmi)
        ):
            if "#" in line.raw:
                additions.append("#" + line.raw.split("#", 1)[1])
            continue
        if line.raw.strip() == "[all]" and not any(
            item.strip() and not item.lstrip().startswith("#") for item in additions
        ):
            continue
        additions.append(line.raw)
    user = "\n".join(additions).strip("\n")
    config = "\n".join([MANAGED_START, MANAGED_NOTE,
                        *managed, MANAGED_END, "", USER_HEADER, "[all]", user]).rstrip() + "\n"
    validated_lines = parse(config, [])
    check_values(validated_lines, model, warnings, required_overlays)
    fields: list[dict[str, Any]] = []
    for key in BOOLEANS:
        if key not in supported or original.get(key) == "":
            continue
        fields.append({
            "key": key, "label": BOOLEANS[key], "type": "boolean", "value": original.get(key, "0"),
            "disabled": key in conditional_keys,
        })
    for key, label, options in [
        ("hdmi_group", "HDMI timing standard", [("", "Firmware default"), ("0", "Auto"), ("1", "CEA (TV)"), ("2", "DMT (monitor)")]),
        ("hdmi_drive", "HDMI output", [("", "Firmware default"), ("1", "DVI (no audio)"), ("2", "HDMI (with audio)")]),
    ]:
        if key in supported:
            fields.append(choice(key, label, original.get(key, ""), options, key in conditional_keys))
    if "hdmi_mode" in supported:
        fields.append({"key": "hdmi_mode", "label": "Custom HDMI mode number", "type": "number",
                       "value": original["hdmi_mode"], "min": 0, "max": 87 if original.get("hdmi_group") == "2" else 107,
                       "disabled": "hdmi_mode" in conditional_keys})
    if "core_freq" in supported:
        fields.append(choice("core_freq", "UART core clock", original.get("core_freq", ""),
                             [("", "Firmware default"), ("250", "250 MHz (UART troubleshooting)")],
                             "core_freq" in conditional_keys))
    if conditional_keys & supported:
        warnings.append("Some controls are read-only because advanced conditional or per-port settings also set them.")
    hdmi = {key: value for key, value in scalar_values(validated_lines).items() if key in HDMI_KEYS}
    other_hdmi = any(line.key.startswith("hdmi_") and line.key not in HDMI_KEYS for line in validated_lines)
    effective_mode = "custom" if conditional_keys & HDMI_KEYS or other_hdmi else (
        "official" if not hdmi else "recommended" if recommended_hdmi and hdmi == recommended_hdmi else "custom"
    )
    choices = request.get("hdmiChoices")
    if not isinstance(choices, list) or not choices or any(
        not isinstance(item, dict) or item.get("value") not in ("official", "recommended", "custom")
        or not isinstance(item.get("label"), str) for item in choices
    ):
        raise ConfigError("The shared HDMI choices are missing from the runtime.")
    return {"config": config, "userText": user, "fields": fields, "hdmiMode": effective_mode,
            "hdmiChoices": choices, "warnings": list(dict.fromkeys(warnings)),
            "protected": [f"{key}={value}" for key, value in required.items()] + [f"dtoverlay={value}" for value in required_overlays]}


def iot(request: dict[str, Any]) -> dict[str, Any]:
    path = Path(__file__).with_name("iot-ffu.py")
    spec = importlib.util.spec_from_file_location("wor_boot_editor_iot_ffu", path)
    if spec is None or spec.loader is None:
        raise ConfigError("The IoT HDMI validator is missing.")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    #The packaged runtime is immutable, including when its FFU validator is imported.
    sys.dont_write_bytecode = True
    spec.loader.exec_module(module)
    warnings: list[str] = []
    text = request.get("config", "")
    lines = parse(text, warnings)
    if any(line.key and (line.key not in HDMI_KEYS - {"hdmi_cvt"} or not line.unconditional) for line in lines):
        raise ConfigError("IoT permits only hdmi_force_hotplug, hdmi_group, hdmi_mode and hdmi_drive. Boot entries stay protected.")
    values = scalar_values(lines)
    changes = request.get("values", {})
    if not isinstance(changes, dict) or any(key not in HDMI_KEYS - {"hdmi_cvt"} for key in changes):
        raise ConfigError("The IoT editor contains an unsupported control.")
    values.update(changes)
    if "userText" in request:
        user_lines = parse(request["userText"], warnings)
        if any(line.key and line.key not in HDMI_KEYS - {"hdmi_cvt"} for line in user_lines):
            raise ConfigError("IoT advanced editing remains HDMI-only.")
        values = scalar_values(user_lines)
    config = "".join(f"{key}={value}\n" for key, value in values.items() if value != "")
    mode = "720p60" if request.get("reset") is True else request.get("hdmiMode", "custom")
    try:
        options = module.hdmi_options(mode, config)
    except module.FfuError as exc:
        raise ConfigError(str(exc)) from exc
    if mode != "custom":
        config = options["settings"] or module.HDMI_PRESETS["720p60"][1]
        values = scalar_values(parse(config, []))
    fields = [
        choice("hdmi_force_hotplug", BOOLEANS["hdmi_force_hotplug"], values.get("hdmi_force_hotplug", ""),
               [("", "Firmware default"), ("1", "On"), ("0", "Off")]),
        choice("hdmi_group", "HDMI timing standard", values.get("hdmi_group", "1"), [("1", "CEA (TV)"), ("2", "DMT (monitor)")]),
        {"key": "hdmi_mode", "label": "HDMI mode number", "type": "number", "value": values.get("hdmi_mode", "4"),
         "min": 1, "max": 64 if values.get("hdmi_group", "1") == "1" else 86, "disabled": False},
        choice("hdmi_drive", "HDMI output", values.get("hdmi_drive", ""), [("", "Firmware default"), ("1", "DVI (no audio)"), ("2", "HDMI (with audio)")]),
    ]
    return {"config": config, "userText": config, "fields": fields, "hdmiMode": mode,
            "hdmiChoices": [{"value": item["value"], "label": item["menu_label"]} for item in options["choices"]],
            "warnings": list(dict.fromkeys(warnings)), "protected": ["All non-HDMI IoT boot settings (unchanged in the verified FFU)"]}


def process(request: dict[str, Any]) -> dict[str, Any]:
    if not isinstance(request, dict):
        raise ConfigError("Expected a boot-editor settings object.")
    if request.get("profile", "desktop") == "desktop":
        return desktop(request)
    if request.get("profile") == "iot-core":
        return iot(request)
    raise ConfigError("Unknown boot-configuration profile.")


def main() -> int:
    try:
        raw = sys.stdin.read(512 * 1024 + 1)
        if len(raw) > 512 * 1024:
            raise ConfigError("Boot-editor input is too large.")
        result = process(json.loads(raw))
    except (ConfigError, json.JSONDecodeError, UnicodeError, OSError) as exc:
        json.dump({"ok": False, "error": str(exc)}, sys.stdout)
        sys.stdout.write("\n")
        return 1
    json.dump({"ok": True, **result}, sys.stdout)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
