use std::path::Path;

#[derive(Default)]
pub(crate) struct SaveHealth {
    pub failure: Option<String>,
    pub saved_bytes: u64,
}

#[derive(Debug, PartialEq)]
pub(crate) enum SpaceState { Healthy, Warning, Stop }

// Concat needs another copy of all committed checkpoints, plus finalization reserve.
pub(crate) fn space_state(available: u64, saved: u64) -> SpaceState {
    let reserve = saved.saturating_add(256 * 1024 * 1024);
    if available < reserve { SpaceState::Stop }
    else if available < reserve.saturating_add(1024 * 1024 * 1024) { SpaceState::Warning }
    else { SpaceState::Healthy }
}

#[cfg(windows)]
pub(crate) fn available_space(path: &Path) -> std::io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut available = 0;
    let success = unsafe { windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
        path.as_ptr(), &mut available, std::ptr::null_mut(), std::ptr::null_mut(),
    ) };
    if success == 0 { Err(std::io::Error::last_os_error()) } else { Ok(available) }
}

#[cfg(not(windows))]
pub(crate) fn available_space(path: &Path) -> std::io::Result<u64> {
    let canonical = path.canonicalize()?;
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks.list().iter().filter(|d| canonical.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().components().count()).map(|d| d.available_space())
        .ok_or_else(|| std::io::Error::other("无法查询录音所在磁盘的可用空间"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reserves_merge_space_as_recording_grows() {
        let mib = 1024 * 1024;
        assert_eq!(space_state(255 * mib, 0), SpaceState::Stop);
        assert_eq!(space_state(512 * mib, 0), SpaceState::Warning);
        assert_eq!(space_state(2_000 * mib, 0), SpaceState::Healthy);
        assert_eq!(space_state(2_000 * mib, 1_000 * mib), SpaceState::Warning);
        assert_eq!(space_state(2_000 * mib, 1_800 * mib), SpaceState::Stop);
        assert_eq!(space_state(0, u64::MAX), SpaceState::Stop);
    }
    #[test]
    fn reads_the_target_disk_and_rejects_missing_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert!(available_space(dir.path()).unwrap() > 0);
        assert!(available_space(&dir.path().join("missing")).is_err());
    }
}
