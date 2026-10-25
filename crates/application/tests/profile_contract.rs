#[cfg(debug_assertions)]
#[inline(never)]
fn config_backtrace_marker() -> String {
    format!("{}", std::backtrace::Backtrace::force_capture())
}

#[cfg(debug_assertions)]
#[test]
fn development_test_profile_keeps_function_and_source_backtraces() {
    let trace = config_backtrace_marker();
    assert!(
        trace.contains("config_backtrace_marker"),
        "Development diagnostics must retain function names"
    );
    assert!(
        trace.contains("profile_contract.rs:"),
        "Development diagnostics must retain source file/line information"
    );
}
