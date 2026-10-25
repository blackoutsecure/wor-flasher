#!/usr/bin/env python3
"""Kill safety-guard mutations in disposable, ignored native-helper copies."""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class Mutation:
    name: str
    file: str
    old: str
    new: str
    suite: str
    test: str


MUTATIONS = (
    Mutation(
        "unknown-board-is-a-skip", "context.rs",
        "_ => return Err(Error::Board),", "_ => return Ok(None),",
        "context", "context_never_guesses_a_board_disk_architecture_scheme_or_drive",
    ),
    Mutation(
        "missing-disk-defaults-to-zero", "context.rs",
        'let index = getenv("WOR_DISK_INDEX");',
        'let index = { let value = getenv("WOR_DISK_INDEX"); if value.is_empty() { "0".into() } else { value } };',
        "context", "context_never_guesses_a_board_disk_architecture_scheme_or_drive",
    ),
    Mutation(
        "discovery-ignores-physical-disk", "context.rs",
        "|| volume.disk_index != context.disk_index", "",
        "context", "discovery_binds_to_selected_physical_disk_partition_one_not_adjacent_letter",
    ),
    Mutation(
        "oversized-extent-response", "context.rs",
        "data.len() != 32", "data.len() < 32",
        "context", "windows_arm64_ioctl_layouts_require_exact_lengths_and_one_aligned_extent",
    ),
    Mutation(
        "oversized-device-number-response", "context.rs",
        "data.len() != 12", "data.len() < 12",
        "context", "raw_device_number_requires_disk_type_exact_index_and_whole_disk_sentinel",
    ),
    Mutation(
        "partition-handle-is-whole-disk", "context.rs",
        "if partition != 0 && partition != u32::MAX {", "if false {",
        "context", "raw_device_number_requires_disk_type_exact_index_and_whole_disk_sentinel",
    ),
    Mutation(
        "layout-ignores-extent-disk", "context.rs",
        "if extent.disk_index != self.disk_index", "if false",
        "guards", "invalid_layouts_fail_before_any_backup_write_or_unbounded_read",
    ),
    Mutation(
        "layout-allows-non512-sectors", "context.rs",
        "if self.bytes_per_sector != 512", "if false",
        "guards", "invalid_layouts_fail_before_any_backup_write_or_unbounded_read",
    ),
    Mutation(
        "gpt-header-crc-ignored", "bootstrap.rs",
        "if crc32fast::hash(&checksum_header[..header_size]) != le32(&header, 16) {",
        "if false {",
        "guards", "gpt_header_and_crc_bounds_cannot_be_bypassed",
    ),
    Mutation(
        "gpt-backup-not-at-physical-end", "bootstrap.rs",
        "|| backup_lba != layout.bytes / 512 - 1", "",
        "guards", "every_used_gpt_partition_and_both_volume_matches_are_checked",
    ),
    Mutation(
        "gpt-table-crc-ignored", "bootstrap.rs",
        "if checksum.finalize() != le32(&header, 88) {", "if false {",
        "guards", "every_used_gpt_partition_and_both_volume_matches_are_checked",
    ),
    Mutation(
        "gpt-used-partition-bounds-ignored", "bootstrap.rs",
        "if start < first_usable || start > end || end > last_usable {", "if false {",
        "guards", "every_used_gpt_partition_and_both_volume_matches_are_checked",
    ),
    Mutation(
        "gpt-boot-may-not-be-first", "bootstrap.rs",
        "if offset + within as u64 != 0", "if false",
        "guards", "every_used_gpt_partition_and_both_volume_matches_are_checked",
    ),
    Mutation(
        "gpt-windows-match-not-unique", "bootstrap.rs",
        "if boot_matches != 1 || windows_matches != 1 {", "if boot_matches != 1 {",
        "guards", "every_used_gpt_partition_and_both_volume_matches_are_checked",
    ),
    Mutation(
        "fat16-root-directory-allowed", "bootstrap.rs",
        "if le16(&boot, 17) != 0", "if false",
        "guards", "fat32_geometry_bounds_version_and_metadata_pointers_fail_closed",
    ),
    Mutation(
        "fat-active-index-ignored", "bootstrap.rs",
        "if fat_flags & 0x80 != 0 && u64::from(fat_flags & 0xf) >= fat_count {", "if false {",
        "guards", "fat32_geometry_bounds_version_and_metadata_pointers_fail_closed",
    ),
    Mutation(
        "fat32-lower-cluster-bound-removed", "bootstrap.rs",
        "!(65525..0x0fff_fff5).contains(&clusters)", "!(0..0x0fff_fff5).contains(&clusters)",
        "guards", "fat32_geometry_bounds_version_and_metadata_pointers_fail_closed",
    ),
    Mutation(
        "fat-root-below-two", "bootstrap.rs",
        "|| root_cluster < 2", "",
        "guards", "fat32_geometry_bounds_version_and_metadata_pointers_fail_closed",
    ),
    Mutation(
        "fat-reserved-metadata-pointer-ignored", "bootstrap.rs",
        "if relative >= reserved {", "if false {",
        "guards", "fat32_geometry_bounds_version_and_metadata_pointers_fail_closed",
    ),
    Mutation(
        "backup-failure-ignored", "bootstrap.rs",
        "backup(&plan.original).map_err(|source| Error::Backup(Box::new(source)))?;",
        "let _ = backup(&plan.original);",
        "transaction", "failed_backup_prevents_every_device_write",
    ),
    Mutation(
        "prewrite-metadata-fingerprint-ignored", "bootstrap.rs",
        "if current != plan {", "if false {",
        "transaction", "changes_to_original_or_valid_gpt_fat_metadata_during_backup_prevent_write",
    ),
    Mutation(
        "short-write-ignored", "bootstrap.rs",
        "if bytes != SECTOR_SIZE {\n        return Err(Error::ShortWrite(bytes));",
        "if false {\n        return Err(Error::ShortWrite(bytes));",
        "transaction", "short_failed_dropped_and_unflushed_writes_never_report_success_or_retry",
    ),
    Mutation(
        "flush-failure-ignored", "bootstrap.rs",
        "disk.sync().map_err(Error::Flush)?;", "let _ = disk.sync();",
        "transaction", "short_failed_dropped_and_unflushed_writes_never_report_success_or_retry",
    ),
    Mutation(
        "sector-zero-readback-ignored", "bootstrap.rs",
        "if verified.original != plan.replacement", "if false",
        "transaction", "short_failed_dropped_and_unflushed_writes_never_report_success_or_retry",
    ),
    Mutation(
        "postwrite-metadata-fingerprint-ignored", "bootstrap.rs",
        "|| verified.metadata != plan.metadata", "",
        "transaction", "postwrite_validation_checks_original_replacement_and_full_metadata_fingerprint",
    ),
    Mutation(
        "empty-installation-file-accepted", "files.rs",
        "if !info.is_file() || info.len() == 0 {", "if !info.is_file() {",
        "filesystem", "installed_kernel_and_firmware_checks_require_nonempty_regular_files",
    ),
    Mutation(
        "winpe-failure-ignored", "workflow.rs",
        '.map_err(|error| error.context("automatic boot refresh must run in Windows PE"))?;',
        '.map_err(|error| error.context("automatic boot refresh must run in Windows PE")).ok();',
        "workflow", "every_prewrite_os_failure_denies_write_and_closes_all_opened_devices",
    ),
    Mutation(
        "reopened-boot-identity-ignored", "workflow.rs",
        '.map_err(|error| error.context("boot-volume identity changed after discovery"))?;',
        '.map_err(|error| error.context("boot-volume identity changed after discovery")).ok();',
        "workflow", "boot_identity_extent_rebinding_and_raw_device_aliases_fail_closed",
    ),
    Mutation(
        "raw-disk-identity-ignored", "workflow.rs",
        "validate_disk_number(&disk.device_number()?, context.disk_index)?;",
        "let _ = validate_disk_number(&disk.device_number()?, context.disk_index);",
        "workflow", "boot_identity_extent_rebinding_and_raw_device_aliases_fail_closed",
    ),
    Mutation(
        "close-error-ignored", "workflow.rs",
        "(Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),",
        "(Err(error), Ok(())) => Err(error), (Ok(value), Err(_)) => Ok(value),",
        "workflow", "empty_or_failed_enumeration_is_fatal_and_close_errors_cannot_report_success",
    ),
    Mutation(
        "nonwindows-simulated-success", "main.rs",
        "Err(Error::UnsupportedPlatform)", 'Ok("simulated success")',
        "entrypoint", "non_windows_binary_refuses_even_an_unrelated_board_without_simulating_success",
    ),
    Mutation(
        "event-field-injection", "lib.rs",
        "'|' => '/',", "'|' => '|',",
        "lib", "tests::wor_events_cannot_inject_fields_or_additional_lines",
    ),
)


def run_tests(root: Path, manifest: Path, selectors: list[str]) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    # A previous run may end on a mutant. Do not let Cargo's mtime fingerprints
    # reuse that binary for a newly copied, older-mtime pristine baseline.
    env["CARGO_TARGET_DIR"] = str(manifest.parent / "build")
    return subprocess.run(
        [
            "cargo", "test", "--offline", "--manifest-path", str(manifest),
            "-p", "wor-pi3-boot-refresh", *selectors,
        ],
        cwd=root, env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        check=False, timeout=180,
    )


def main() -> int:
    root = Path(__file__).resolve().parents[3]
    source = root / "apps/pi3-boot-refresh"
    directory = root / "target/pi3-native-mutations" / f"{os.getpid()}-{time.time_ns()}"
    directory.mkdir(parents=True)
    try:
        manifest = directory / "Cargo.toml"
        inherited = (root / "Cargo.toml").read_text()
        inherited = inherited[inherited.index("[workspace.package]"):]
        manifest.write_text('[workspace]\nmembers = ["pi3"]\nresolver = "2"\n\n' + inherited)
        shutil.copytree(source, directory / "pi3", ignore=shutil.ignore_patterns("__pycache__"))
        baseline = run_tests(root, manifest, ["--lib", "--tests"])
        if baseline.returncode != 0:
            print(baseline.stdout, file=sys.stderr)
            print("Mutation baseline failed; no mutations counted.", file=sys.stderr)
            return 1
        print("BASELINE: PASS", flush=True)
        for mutation in MUTATIONS:
            path = directory / "pi3/src" / mutation.file
            original = path.read_text()
            if original.count(mutation.old) != 1:
                print(f"Mutation anchor is not unique: {mutation.name}", file=sys.stderr)
                return 1
            path.write_text(original.replace(mutation.old, mutation.new, 1))
            try:
                selectors = ["--lib"] if mutation.suite == "lib" else ["--test", mutation.suite]
                result = run_tests(root, manifest, [*selectors, mutation.test, "--", "--exact"])
            finally:
                path.write_text(original)
            killed = (
                result.returncode != 0
                and f"test {mutation.test} ... FAILED" in result.stdout
                and "0 passed; 1 failed" in result.stdout
            )
            if not killed:
                print(result.stdout, file=sys.stderr)
                print(f"Mutation survived or failed to execute: {mutation.name}", file=sys.stderr)
                return 1
            print(f"KILLED: {mutation.name}", flush=True)
        print(f"RESULT: {len(MUTATIONS)}/{len(MUTATIONS)} guard mutations detected; source files untouched.")
        return 0
    finally:
        shutil.rmtree(directory)


if __name__ == "__main__":
    raise SystemExit(main())
