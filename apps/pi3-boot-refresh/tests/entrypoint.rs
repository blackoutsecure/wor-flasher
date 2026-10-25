#[cfg(not(windows))]
#[test]
fn non_windows_binary_refuses_even_an_unrelated_board_without_simulating_success() {
    for board in ["RPi3-ARM64", "RPi4-ARM64", "RPi5-ARM64", ""] {
        let result = std::process::Command::new(env!("CARGO_BIN_EXE_wor-pi3-boot-refresh"))
            .env_clear()
            .env("WOR_DEVICE_TYPE", board)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(1));
        assert_eq!(
            String::from_utf8(result.stdout).unwrap(),
            "WoR-Event|LogFatal|Pi 3 boot finalization failed: Pi 3 boot finalization requires Windows PE; no disk accessed\n"
        );
        assert!(result.stderr.is_empty());
    }
}
