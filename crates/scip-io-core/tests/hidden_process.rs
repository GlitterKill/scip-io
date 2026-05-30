#[cfg(windows)]
#[test]
fn hidden_child_processes_request_no_console_window() {
    assert_eq!(
        scip_io_core::process::hidden_process_creation_flags(),
        0x0800_0000
    );
}

#[cfg(not(windows))]
#[test]
fn hidden_child_processes_do_not_set_platform_flags_off_windows() {
    assert_eq!(scip_io_core::process::hidden_process_creation_flags(), 0);
}
