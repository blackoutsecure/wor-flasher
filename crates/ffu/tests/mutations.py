#!/usr/bin/env python3
"""Kill safety-guard mutations in isolated native source copies, never the live tree."""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
from pathlib import Path


CRATE = Path(__file__).resolve().parents[1]
CANONICAL_PROVENANCE = CRATE.parents[1] / "src/config/iot-core-images.json"
STATE = CRATE / ".test-state" / f"mutations-{os.getpid()}"
STANDALONE = """\
[package]
name = "wor-ffu"
version = "0.1.0"
edition = "2024"
rust-version = "1.99"
license = "GPL-3.0-or-later"
publish = false
[workspace]
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sha2 = "0.10"
crc32fast = "1"
thiserror = "2"
libc = "0.2"
"""

# Each pattern is anchored to a real production guard, not a test-only imitation.
MUTATIONS = (
    ("chunk-hash", "parser.rs",
     r"if Sha256::digest\(&data\)\[\.\.\] != hashes\[index \* 32\.\.\(index \+ 1\) \* 32\] \{",
     "if false {", "payload_hash_guard"),
    ("mandatory-readback", "writer.rs", r"if actual != expected \{",
     "if false {", "readback_guard"),
    ("approved-source", "writer.rs", r"approve\(&info\)\?;",
     "let _ = &info;", "complete_source_before_target_open_and_allowlist_guard"),
    ("old-gpt-refusal", "writer.rs", r"reject_existing_gpt\(target, info\)\?;",
     "let _ = (target, info);", "gpt_preflight_guard"),
    ("confirmed-target-identity", "writer.rs", r"identity\.to_string\(\) != \*expected",
     "false && identity.to_string() != *expected", "target_identity_guard"),
    ("gpt-header-crc", "writer.rs", r"if crc32fast::hash\(&checked\) != u32_at\(data, 16\) \{",
     "if false {", "gpt_crc_guard"),
    ("gpt-tail-readback", "writer.rs", r"verify_gpt_cleanup\(target, info, plan, &cleanup\)\?;",
     "let _ = &cleanup;", "gpt_cleanup_readback_guard"),
    ("whole-drive-wipe", "writer.rs", r"wipe_target\(source, target, progress\)\?;",
     "let _ = (&*source, target, &progress);", "full_wipe_guard_zeroes_every_sector_and_tail_before_image"),
    ("whole-drive-blank-readback", "writer.rs", r"else if target\.read\(size, offset\)\? != zeros\[\.\.size\] \{",
     "else if false {", "wipe_readback_guard_prevents_image_application"),
    ("source-payload-rehash", "source.rs",
     r"if Sha256::digest\(&data\)\[\.\.\] != info\.chunk_hashes\[index \* 32\.\.\(index \+ 1\) \* 32\] \{",
     "if false {", "payload_rehash_even_when_stamp_change_is_hidden"),
    ("source-stamp", "source.rs", r"if SourceStamp::from\(&unix::metadata\(&self\.file\)\?\) != self\.stamp \{",
     "if false {", "source_mutation_after_preflight_blocks_first_write"),
    ("native-geometry", "writer.rs", r"if actual != self\.size \|\| sector != self\.sector \{",
     "if false {", "actual_geometry_guard"),
    ("parent-no-follow", "unix.rs",
     r"let flags = libc::O_RDONLY \| libc::O_DIRECTORY \| libc::O_NOFOLLOW \| libc::O_CLOEXEC;",
     "let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC;",
     "source_target_and_parent_symlinks_refused"),
    ("final-component-no-follow", "unix.rs", r"(?m)^            \| libc::O_NOFOLLOW\n",
     "", "final_component_symlink_race_is_never_followed"),
    ("readback-cache-preflight", "writer.rs", r"if !target\.fixture \{\s*target\.sync\(true\)\?;\s*\}",
     "if !target.fixture { let _ = target; }", "cache_capability_failure_and_4kn_before_any_write"),
    ("hdmi-fat-copy", "hdmi.rs", r"if fat\s*!= image_bytes\([\s\S]*?\)\?\s*\{",
     "if false {", "hdmi_bad_filesystem_or_allocation_never_writes"),
    ("bounded-cancellation", "lib.rs", r"if self\.flag\.load\(Ordering::Relaxed\) \{",
     "if false {", "cancelled_write_never_verifies"),
    ("initial-gpt-cleanup-readback", "writer.rs",
     r"if target\s*\.read\(region\.original\.len\(\), region\.offset\)\?\s*\.iter\(\)\s*\.any\(\|byte\| \*byte != 0\)\s*\{",
     "if false {", "cleanup_readback_failure_and_interrupt_cannot_verify"),
    ("target-close-failure", "writer.rs", r"target_close\?;",
     "let _ = target_close;", "close_failure_never_returns_success"),
)


def command(project: Path, test: str) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ, CARGO_TARGET_DIR=str(STATE / "target"))
    return subprocess.run(
        ["cargo", "test", "--offline", "--manifest-path", str(project / "Cargo.toml"),
         "--lib", f"tests::{test}", "--", "--exact"],
        env=env, capture_output=True, text=True, timeout=180, check=False,
    )


results = []
STATE.mkdir(parents=True)
try:
    for name, filename, pattern, replacement, test in MUTATIONS:
        root = STATE / name
        project = root / "crates/ffu"
        project.mkdir(parents=True)
        shutil.copytree(CRATE / "src", project / "src")
        provenance = root / "src/config/iot-core-images.json"
        provenance.parent.mkdir(parents=True)
        shutil.copyfile(CANONICAL_PROVENANCE, provenance)
        # Unique package identities prevent Cargo from reusing a previous
        # mutant's fingerprint when copytree preserves source timestamps.
        (project / "Cargo.toml").write_text(
            STANDALONE.replace('name = "wor-ffu"', f'name = "ffu-mutation-{name}"')
        )
        baseline = command(project, test)
        if baseline.returncode != 0 or "1 passed; 0 failed" not in baseline.stdout:
            raise AssertionError(f"{name}: unchanged guard test failed:\n{baseline.stdout}\n{baseline.stderr}")
        source = project / "src" / filename
        code, changes = re.subn(pattern, replacement, source.read_text())
        if changes != 1:
            raise AssertionError(f"{name}: expected exactly one real guard mutation, got {changes}")
        source.write_text(code)
        mutant = command(project, test)
        if mutant.returncode != 101 or "0 passed; 1 failed" not in mutant.stdout or "error: could not compile" in mutant.stderr:
            raise AssertionError(f"{name}: mutation survived or failed to compile:\n{mutant.stdout}\n{mutant.stderr}")
        results.append({"guard": name, "test": f"tests::{test}", "baseline_exit": 0, "mutant_exit": 101, "killed": True})
        print(f"KILLED {name}: tests::{test}", flush=True)
    print(json.dumps({"total": len(results), "killed": len(results), "survived": 0, "results": results}, sort_keys=True))
finally:
    shutil.rmtree(STATE)
    try:
        STATE.parent.rmdir()
    except OSError:
        pass
