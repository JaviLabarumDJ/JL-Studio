//! Windows' consent-based fallback for a helper rejected with ERROR_ELEVATION_REQUIRED.
use std::{ffi::OsStr, os::windows::{ffi::OsStrExt, io::{FromRawHandle, AsRawHandle, OwnedHandle}, process::{ExitStatusExt,CommandExt}}, process::{ExitStatus, Command}};
use windows_sys::Win32::UI::{Shell::{ShellExecuteExW, SHELLEXECUTEINFOW, SEE_MASK_NOCLOSEPROCESS, SEE_MASK_NOASYNC, SEE_MASK_FLAG_NO_UI}, WindowsAndMessaging::SW_HIDE};
use windows_sys::Win32::{Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT}, System::Threading::{WaitForSingleObject, GetExitCodeProcess, TerminateProcess, GetProcessId, OpenProcess, PROCESS_SYNCHRONIZE}};

/// Older signed updaters put the new binary under its original filename.
/// Switch only in the registered installation and never overwrite another binary.
pub(super) fn migrate_legacy_launch(args: &[String]) -> std::io::Result<bool> {
    use std::{fs, path::Path};
    let current = std::env::current_exe()?;
    let Some(name) = current.file_name().and_then(|name| name.to_str()) else { return Ok(false); };
    if !name.eq_ignore_ascii_case(super::EXE) && !name.eq_ignore_ascii_case(super::WINDOWS_EXE) {
        return Ok(false);
    }
    let Some(install) = crate::file_assoc::registered_installation() else { return Ok(false); };
    let same_install = |path: &Path| -> std::io::Result<bool> {
        Ok(fs::canonicalize(path)?.as_os_str().to_string_lossy()
            .eq_ignore_ascii_case(&fs::canonicalize(&install)?.as_os_str().to_string_lossy()))
    };
    if !current.parent().is_some_and(|path| same_install(path).unwrap_or(false)) {
        return Ok(false);
    }
    let legacy = install.join(super::EXE);
    let branded = install.join(super::WINDOWS_EXE);
    if name.eq_ignore_ascii_case(super::WINDOWS_EXE) {
        // The helper came from the previous release. Run this build's repair
        // on its first normal launch as well.
        if crate::file_assoc::shortcut_repair_needed(&super::VERSION.to_string()) {
            let target = branded.clone();
            std::thread::spawn(move || {
                if let Err(error) = repair_start_menu(&target) { eprintln!("ML Studio shortcuts: {error}"); }
            });
        }
        if legacy.symlink_metadata().is_ok_and(|meta| meta.file_type().is_file()) {
            std::thread::spawn(move || {
                if !identical_executables(&legacy, &branded).unwrap_or(false) { return; }
                // The old image can still be releasing its executable after handing off.
                for _ in 0..20 {
                    if fs::remove_file(&legacy).is_ok() { break; }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            });
        }
        return Ok(false);
    }
    if !legacy.symlink_metadata()?.file_type().is_file() { return Ok(false); }
    create_branded_executable(&legacy, &branded)?;
    crate::file_assoc::retarget_installation(&legacy, &branded);
    if let Err(error) = repair_start_menu(&branded) { eprintln!("ML Studio shortcuts: {error}"); }
    std::process::Command::new(&branded).args(args).spawn()?;
    Ok(true)
}

fn identical_executables(left: &std::path::Path, right: &std::path::Path) -> std::io::Result<bool> {
    use std::{fs, io::{BufReader, Read}};
    let a = left.symlink_metadata()?;
    let b = right.symlink_metadata()?;
    if !a.file_type().is_file() || !b.file_type().is_file() || a.len() != b.len() { return Ok(false); }
    let mut left = BufReader::new(fs::File::open(left)?);
    let mut right = BufReader::new(fs::File::open(right)?);
    let (mut a, mut b) = ([0; 8192], [0; 8192]);
    let mut remaining = left.get_ref().metadata()?.len();
    while remaining > 0 {
        let size = remaining.min(a.len() as u64) as usize;
        left.read_exact(&mut a[..size])?;
        right.read_exact(&mut b[..size])?;
        if a[..size] != b[..size] { return Ok(false); }
        remaining -= size as u64;
    }
    Ok(true)
}

fn create_branded_executable(legacy: &std::path::Path, branded: &std::path::Path) -> std::io::Result<()> {
    use std::{fs, io};
    if branded.symlink_metadata().is_ok() {
        if identical_executables(legacy, branded)? { return Ok(()); }
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "another ML Studio executable is installed"));
    }
    let temporary = branded.with_file_name(format!(".ml-studio-name-{}.tmp", std::process::id()));
    let mut output = fs::OpenOptions::new().create_new(true).write(true).open(&temporary)?;
    let result = (|| {
        io::copy(&mut fs::File::open(legacy)?, &mut output)?;
        output.sync_all()?;
        drop(output);
        fs::hard_link(&temporary, branded)
    })();
    let _ = fs::remove_file(temporary);
    result
}

pub(super) struct ShellChild(OwnedHandle);
impl ShellChild {
    pub(super) fn id(&self) -> u32 { unsafe { GetProcessId(self.0.as_raw_handle()) } }
    pub(super) fn try_wait(&self) -> std::io::Result<Option<ExitStatus>> {
        let handle = self.0.as_raw_handle();
        match unsafe { WaitForSingleObject(handle, 0) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                let mut code = 0;
                if unsafe { GetExitCodeProcess(handle, &mut code) } == 0 { return Err(std::io::Error::last_os_error()); }
                Ok(Some(ExitStatus::from_raw(code)))
            }
            _ => Err(std::io::Error::last_os_error()),
        }
    }
    pub(super) fn kill(&self) -> std::io::Result<()> {
        if unsafe { TerminateProcess(self.0.as_raw_handle(), 1) } == 0 { return Err(std::io::Error::last_os_error()); }
        Ok(())
    }
}

// ShellExecuteEx takes a command line. Preserve Rust/CRT argument boundaries, including
// Unicode, embedded quotes and trailing backslashes. No command interpreter is involved.
fn parameters(cmd: &Command) -> Vec<u16> {
    let mut result = Vec::new();
    for arg in cmd.get_args() {
        if !result.is_empty() { result.push(b' ' as u16); }
        result.push(b'"' as u16);
        let mut slashes = 0;
        for unit in arg.encode_wide() {
            if unit == b'\\' as u16 {
                slashes += 1;
                continue;
            }
            let count = if unit == b'"' as u16 { slashes * 2 + 1 } else { slashes };
            result.extend(std::iter::repeat_n(b'\\' as u16, count));
            slashes = 0;
            result.push(unit);
        }
        result.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
        result.push(b'"' as u16);
    }
    result.push(0);
    result
}

pub(super) fn elevate(cmd: &Command) -> super::Result<ShellChild> {
    // A separate normal-permission process reopens the user's session.
    let mut elevated = Command::new(cmd.get_program());
    for (index, arg) in cmd.get_args().enumerate() {
        elevated.arg(if index == 4 { OsStr::new("close") } else { arg });
    }
    shell_launch(&elevated, "runas")
}

pub(super) fn relaunch(args: &[String]) -> super::Result<()> {
    use std::path::Path;
    if args.len() != 5 { return Err("Missing restart watcher arguments".into()); }
    let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, args[1].parse()?) };
    if process.is_null() { return Err(std::io::Error::last_os_error().into()); }
    let process = unsafe { OwnedHandle::from_raw_handle(process) };
    let handoff = Path::new(&args[3]);
    super::wait_parent(args[0].parse()?, handoff)?;
    if unsafe { WaitForSingleObject(process.as_raw_handle(), 120000) } != WAIT_OBJECT_0 {
        return Err("Updater did not finish; reopen ML Studio from its shortcut".into());
    }
    if handoff.join("helper-cancelled").exists() { return Ok(()); }
    if let Some(stage) = handoff.parent().and_then(Path::parent) {
        if let Ok(message) = std::fs::read_to_string(stage.join("result.txt")) { super::report_result(&message); }
    }
    super::restart_app(Path::new(&args[2]), Some(Path::new(&args[4])))
}

fn shell_launch(cmd: &Command, operation: &str) -> super::Result<ShellChild> {
    let file: Vec<u16> = cmd.get_program().encode_wide().chain(Some(0)).collect();
    let args = parameters(cmd);
    let verb: Vec<u16> = OsStr::new(operation).encode_wide().chain(Some(0)).collect();
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = args.as_ptr();
    info.nShow = SW_HIDE;
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(1223) {
            return Err("Windows permission was cancelled. Your project is still open. Click Update to try again.".into());
        }
        return Err(format!("Windows could not start the updater with permission ({error}). Your project is still open.").into());
    }
    if info.hProcess.is_null() {
        return Err("Windows did not return an updater process. Your project is still open.".into());
    }
    Ok(ShellChild(unsafe { OwnedHandle::from_raw_handle(info.hProcess) }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_copy_adopts_the_new_process_name_without_overwriting_other_binaries() {
        let root = tempfile::tempdir().unwrap();
        let legacy = root.path().join(super::super::EXE);
        let branded = root.path().join(super::super::WINDOWS_EXE);
        std::fs::write(&legacy, b"1.0 signed update fixture").unwrap();
        create_branded_executable(&legacy, &branded).unwrap();
        assert!(identical_executables(&legacy, &branded).unwrap());
        assert!(!same_file::is_same_file(&legacy, &branded).unwrap());
        create_branded_executable(&legacy, &branded).unwrap();
        std::fs::write(&branded, b"another installation").unwrap();
        assert!(create_branded_executable(&legacy, &branded).is_err());
        assert_eq!(std::fs::read(&branded).unwrap(), b"another installation");
    }
    #[test]
    fn updater_shell_returns_waitable_process() {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", "updates::windows_launch::tests::updater_shell_arguments_round_trip"]);
        let child = shell_launch(&command, "open").unwrap();
        assert_eq!(unsafe { WaitForSingleObject(child.0.as_raw_handle(), 5000) }, WAIT_OBJECT_0);
        assert_eq!(child.try_wait().unwrap().unwrap().code(), Some(0));
    }

    #[test]
    fn updater_shell_arguments_round_trip() {
        use windows_sys::Win32::{Foundation::LocalFree, UI::Shell::CommandLineToArgvW};
        let values = ["--apply-update", "1234", "C:\\Users\\Müller 音楽\\ML Studio\\lemonstudio.exe", "E:\\cache folder\\", "restart", "a\"b\\\"c", ""];
        let mut command = Command::new("unused.exe");
        command.args(values);
        // CommandLineToArgvW handles argv[0] differently, so supply a real executable token.
        let mut line: Vec<u16> = "helper.exe ".encode_utf16().collect();
        line.extend(parameters(&command));
        let mut count = 0;
        let args = unsafe { CommandLineToArgvW(line.as_ptr(), &mut count) };
        assert!(!args.is_null());
        let decoded: Vec<String> = unsafe { std::slice::from_raw_parts(args, count as usize) }.iter().skip(1).map(|ptr| {
            let mut len = 0;
            unsafe {
                while *ptr.add(len) != 0 { len += 1; }
                String::from_utf16(std::slice::from_raw_parts(*ptr, len)).unwrap()
            }
        }).collect();
        unsafe { LocalFree(args.cast()); }
        assert_eq!(decoded, values);
    }
}

/// Only repair an existing installer-managed copy. Portable and Development
/// executables never acquire public shortcuts through an update.
pub(super) fn repair_start_menu(target: &std::path::Path) -> std::io::Result<()> {
    let script = include_str!("windows-shortcuts.ps1");
    let command = format!("{script}\nRepair-RegisteredMLStudioShortcuts");
    let status=Command::new("powershell.exe")
        .args(["-NoProfile","-NonInteractive","-Command",&command])
        .env("ML_SHORTCUT_TARGET",target)
        .env("ML_SHORTCUT_BUILD",super::VERSION.to_string())
        .env("ML_SHORTCUT_VERSION", super::display_version())
        .creation_flags(0x08000000).status()?;
    if status.success(){Ok(())}else{Err(std::io::Error::other("Could not repair the Start menu shortcut"))}
}
