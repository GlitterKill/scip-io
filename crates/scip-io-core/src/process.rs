use std::ffi::OsStr;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn hidden_process_creation_flags() -> u32 {
    #[cfg(windows)]
    {
        CREATE_NO_WINDOW
    }
    #[cfg(not(windows))]
    {
        0
    }
}

pub fn hidden_tokio_command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program);
    apply_hidden_tokio_window(&mut command);
    command
}

pub fn hidden_std_command(program: impl AsRef<OsStr>) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    apply_hidden_std_window(&mut command);
    command
}

#[cfg(windows)]
fn apply_hidden_tokio_window(command: &mut tokio::process::Command) {
    // GUI builds have no parent console, so console-subsystem children flash
    // visible windows unless Windows is told to create them without a window.
    command.creation_flags(hidden_process_creation_flags());
}

#[cfg(not(windows))]
fn apply_hidden_tokio_window(_command: &mut tokio::process::Command) {}

#[cfg(windows)]
fn apply_hidden_std_window(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;

    // Keep explorer/open helpers and other GUI-launched children visually quiet.
    command.creation_flags(hidden_process_creation_flags());
}

#[cfg(not(windows))]
fn apply_hidden_std_window(_command: &mut std::process::Command) {}
