use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const UNREADABLE: &str = "The disk you attached was not readable by this computer.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alert {
    pub owner: String,
    pub messages: Vec<String>,
    pub buttons: Vec<String>,
}

pub fn ignore_candidate(alerts: &[Alert], writing: bool, permission: bool) -> Option<usize> {
    if !writing || !permission {
        return None;
    }
    let matches = alerts
        .iter()
        .enumerate()
        .filter(|(_, alert)| {
            matches!(
                alert.owner.as_str(),
                "UserNotificationCenter" | "DiskArbitrationAgent"
            ) && alert
                .messages
                .iter()
                .any(|message| message.trim() == UNREADABLE)
                && alert
                    .buttons
                    .iter()
                    .filter(|button| button.as_str() == "Ignore")
                    .count()
                    == 1
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    (matches.len() == 1).then(|| matches[0])
}

pub fn accessibility_trusted() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::trusted()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

pub struct Monitor {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Monitor {
    pub fn start(warning: Arc<dyn Fn(String) + Send + Sync>) -> Result<Self, String> {
        if !cfg!(target_os = "macos") {
            return Err("Automatic disk-alert handling is macOS-only".into());
        }
        if !accessibility_trusted() {
            return Err("Accessibility permission is unavailable; choose Ignore manually.".into());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let worker = thread::Builder::new()
            .name("native-disk-alerts".into())
            .spawn(move || {
                let mut last_warning = String::new();
                while !flag.load(Ordering::Acquire) {
                    #[cfg(target_os = "macos")]
                    let result = macos::handle(&flag);
                    #[cfg(not(target_os = "macos"))]
                    let result: Result<(), String> = Err("Unsupported alert provider".into());
                    if let Err(error) = result
                        && error != last_warning
                    {
                        warning(error.clone());
                        last_warning = error;
                    }
                    thread::sleep(Duration::from_millis(150));
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            stop,
            thread: Some(worker),
        })
    }
    pub fn stop(&mut self) -> Result<(), String> {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.thread.take() {
            worker
                .join()
                .map_err(|_| "Native alert monitor failed during cleanup".to_owned())?;
        }
        Ok(())
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            tracing::error!(error=%error,"Disk alert monitor cleanup failed");
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{Alert, ignore_candidate};
    use std::ffi::{CString, c_char, c_void};
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, Ordering};
    type Ref = *const c_void;
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> u8;
        fn AXUIElementCreateApplication(pid: i32) -> Ref;
        fn AXUIElementCopyAttributeValue(element: Ref, attribute: Ref, value: *mut Ref) -> i32;
        fn AXUIElementPerformAction(element: Ref, action: Ref) -> i32;
        fn AXUIElementSetMessagingTimeout(element: Ref, seconds: f32) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(value: Ref);
        fn CFRetain(value: Ref) -> Ref;
        fn CFGetTypeID(value: Ref) -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFArrayGetTypeID() -> usize;
        fn CFStringCreateWithCString(allocator: Ref, text: *const c_char, encoding: u32) -> Ref;
        fn CFStringGetLength(value: Ref) -> isize;
        fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
        fn CFStringGetCString(value: Ref, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
        fn CFArrayGetCount(value: Ref) -> isize;
        fn CFArrayGetValueAtIndex(value: Ref, index: isize) -> Ref;
    }
    struct Owned(Ref);
    impl Drop for Owned {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0) }
            }
        }
    }
    fn string(text: &str) -> Result<Owned, String> {
        let text = CString::new(text).map_err(|_| "Invalid Accessibility attribute".to_owned())?;
        let value =
            unsafe { CFStringCreateWithCString(std::ptr::null(), text.as_ptr(), 0x08000100) };
        if value.is_null() {
            Err("Cannot allocate Accessibility attribute".into())
        } else {
            Ok(Owned(value))
        }
    }
    fn attribute(element: Ref, name: &str) -> Result<Option<Owned>, String> {
        let key = string(name)?;
        let mut value = std::ptr::null();
        let status = unsafe { AXUIElementCopyAttributeValue(element, key.0, &mut value) };
        match status {
            0 if !value.is_null() => Ok(Some(Owned(value))),
            -25205 | -25212 => Ok(None),
            -25211 => Err(
                "Accessibility permission was denied or revoked; choose Ignore manually.".into(),
            ),
            _ => Err(format!(
                "Accessibility metadata is unavailable ({status}); handle the disk alert manually."
            )),
        }
    }
    fn text(value: Ref) -> Result<Option<String>, String> {
        if unsafe { CFGetTypeID(value) } != unsafe { CFStringGetTypeID() } {
            return Ok(None);
        }
        let length = unsafe { CFStringGetLength(value) };
        let size = unsafe { CFStringGetMaximumSizeForEncoding(length, 0x08000100) }
            .checked_add(1)
            .filter(|size| *size > 0 && *size < 65536)
            .ok_or_else(|| "Accessibility text exceeded its bound".to_owned())?;
        let mut buffer =
            vec![0_u8; usize::try_from(size).map_err(|_| "Invalid Accessibility string size")?];
        let ok = unsafe { CFStringGetCString(value, buffer.as_mut_ptr().cast(), size, 0x08000100) };
        if ok == 0 {
            return Err("Accessibility text decoding failed".into());
        }
        let end = buffer
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(buffer.len());
        Ok(Some(
            String::from_utf8(buffer[..end].into())
                .map_err(|_| "Accessibility text is not UTF-8")?,
        ))
    }
    fn attr_text(element: Ref, name: &str) -> Result<Option<String>, String> {
        match attribute(element, name)? {
            Some(value) => text(value.0),
            None => Ok(None),
        }
    }
    fn elements(value: Ref) -> Result<Vec<Owned>, String> {
        if unsafe { CFGetTypeID(value) } != unsafe { CFArrayGetTypeID() } {
            return Err("Unexpected Accessibility collection".into());
        }
        let count = unsafe { CFArrayGetCount(value) };
        if !(0..=512).contains(&count) {
            return Err("Accessibility collection exceeded its bound".into());
        }
        let mut values = Vec::new();
        for index in 0..count {
            let element = unsafe { CFArrayGetValueAtIndex(value, index) };
            if element.is_null() {
                return Err("Missing Accessibility element".into());
            }
            values.push(Owned(unsafe { CFRetain(element) }));
        }
        Ok(values)
    }
    fn scan(
        element: Ref,
        depth: u8,
        remaining: &mut usize,
        stop: &AtomicBool,
        alert: &mut Alert,
        buttons: &mut Vec<Owned>,
    ) -> Result<(), String> {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        if depth > 8 || *remaining == 0 {
            return Err(
                "The system alert exceeds safe inspection bounds; handle it manually.".into(),
            );
        }
        *remaining -= 1;
        let role = attr_text(element, "AXRole")?;
        for key in ["AXTitle", "AXValue", "AXDescription"] {
            if let Some(value) = attr_text(element, key)? {
                alert.messages.push(value);
            }
        }
        if role.as_deref() == Some("AXButton")
            && let Some(title) = attr_text(element, "AXTitle")?
        {
            alert.buttons.push(title.clone());
            if title == "Ignore" {
                buttons.push(Owned(unsafe { CFRetain(element) }));
            }
        }
        if let Some(children) = attribute(element, "AXChildren")? {
            for child in elements(children.0)? {
                scan(child.0, depth + 1, remaining, stop, alert, buttons)?;
            }
        }
        Ok(())
    }
    pub fn trusted() -> bool {
        unsafe { AXIsProcessTrusted() != 0 }
    }
    pub fn handle(stop: &AtomicBool) -> Result<(), String> {
        if !trusted() {
            return Err("Accessibility permission is unavailable; choose Ignore manually.".into());
        }
        let output = Command::new("/bin/ps")
            .args(["-axo", "pid=,comm="])
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err("Cannot identify system disk-alert owners".into());
        }
        let mut alerts = Vec::new();
        let mut buttons = Vec::<Vec<Owned>>::new();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if stop.load(Ordering::Acquire) {
                return Ok(());
            }
            let line = line.trim();
            let Some((pid, command)) = line.split_once(char::is_whitespace) else {
                continue;
            };
            let owner = std::path::Path::new(command.trim())
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            if !matches!(owner, "UserNotificationCenter" | "DiskArbitrationAgent") {
                continue;
            }
            let pid = pid
                .parse::<i32>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| "Invalid system alert owner identity".to_owned())?;
            let app = Owned(unsafe { AXUIElementCreateApplication(pid) });
            if app.0.is_null() {
                return Err("Cannot access the system disk-alert owner".into());
            }
            let status = unsafe { AXUIElementSetMessagingTimeout(app.0, 0.5) };
            if status != 0 {
                return Err("Cannot bound system Accessibility requests".into());
            }
            if let Some(windows) = attribute(app.0, "AXWindows")? {
                for window in elements(windows.0)? {
                    let mut alert = Alert {
                        owner: owner.into(),
                        messages: Vec::new(),
                        buttons: Vec::new(),
                    };
                    let mut ignore = Vec::new();
                    let mut remaining = 512;
                    scan(window.0, 0, &mut remaining, stop, &mut alert, &mut ignore)?;
                    alerts.push(alert);
                    buttons.push(ignore);
                }
            }
        }
        if let Some(index) = ignore_candidate(&alerts, true, trusted()) {
            if stop.load(Ordering::Acquire) {
                return Ok(());
            }
            let button = buttons
                .get(index)
                .and_then(|values| values.first())
                .ok_or_else(|| "Confirmed Ignore button disappeared".to_owned())?;
            let action = string("AXPress")?;
            if unsafe { AXUIElementPerformAction(button.0, action.0) } != 0 {
                return Err("The system Ignore action failed; handle the alert manually.".into());
            }
        } else if alerts
            .iter()
            .filter(|alert| {
                alert
                    .messages
                    .iter()
                    .any(|message| message.trim() == super::UNREADABLE)
            })
            .count()
            > 1
        {
            return Err(
                "Multiple unreadable-disk alerts are present; all were left for manual handling."
                    .into(),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn alert() -> Alert {
        Alert {
            owner: "UserNotificationCenter".into(),
            messages: vec![UNREADABLE.into()],
            buttons: vec!["Ignore".into(), "Initialize".into(), "Eject".into()],
        }
    }
    #[test]
    fn only_one_exact_system_alert_in_an_authorized_write_window_can_be_ignored() {
        assert_eq!(ignore_candidate(&[alert()], true, true), Some(0));
        assert_eq!(ignore_candidate(&[alert()], false, true), None);
        assert_eq!(ignore_candidate(&[alert()], true, false), None);
        assert_eq!(ignore_candidate(&[alert(), alert()], true, true), None);
        let mut other = alert();
        other.owner = "another application".into();
        assert_eq!(ignore_candidate(&[other], true, true), None);
        let mut other = alert();
        other.messages = vec!["A different warning".into()];
        assert_eq!(ignore_candidate(&[other], true, true), None);
        let mut other = alert();
        other.buttons = vec!["Initialize".into(), "Eject".into()];
        assert_eq!(ignore_candidate(&[other], true, true), None);
        let mut other = alert();
        other.buttons.push("Ignore".into());
        assert_eq!(ignore_candidate(&[other], true, true), None);
    }
}
