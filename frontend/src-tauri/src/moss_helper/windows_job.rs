#[cfg(windows)]
mod imp {
    use std::ffi::{c_void, OsStr, OsString};
    use std::fs::File;
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::MetadataExt;
    use std::os::windows::io::{FromRawHandle, RawHandle};
    use std::path::Path;
    use std::ptr::{null, null_mut};
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{
        CloseHandle, SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
        JobObjectBasicProcessIdList, JobObjectExtendedLimitInformation, QueryInformationJobObject,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
        JOBOBJECT_BASIC_PROCESS_ID_LIST, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Pipes::CreatePipe;
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
        InitializeProcThreadAttributeList, ResumeThread, TerminateProcess,
        UpdateProcThreadAttribute, WaitForSingleObject, CREATE_NO_WINDOW, CREATE_SUSPENDED,
        EXTENDED_STARTUPINFO_PRESENT, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
        STARTF_USESTDHANDLES, STARTUPINFOEXW,
    };

    const FORCED_EXIT_CODE: u32 = 0x4D4F_5353;

    #[derive(Debug, thiserror::Error)]
    pub enum JobError {
        #[error("helper executable is unavailable")]
        Executable,
        #[error("{operation} failed with Windows error {code}: {message}")]
        Windows {
            operation: &'static str,
            code: i32,
            message: String,
        },
        #[error("{operation} returned an invalid result")]
        InvalidResult { operation: &'static str },
        #[error("{operation} did not complete within {timeout_ms} ms")]
        Timeout {
            operation: &'static str,
            timeout_ms: u128,
        },
    }

    struct KernelHandle(HANDLE);

    impl KernelHandle {
        fn raw(&self) -> HANDLE {
            self.0
        }
    }

    unsafe impl Send for KernelHandle {}
    unsafe impl Sync for KernelHandle {}

    impl Drop for KernelHandle {
        fn drop(&mut self) {
            // SAFETY: this wrapper uniquely owns a valid kernel handle.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    struct PendingHandle(Option<HANDLE>);

    impl PendingHandle {
        fn new(value: HANDLE) -> Self {
            Self(Some(value))
        }

        fn raw(&self) -> HANDLE {
            self.0.expect("pending handle")
        }

        fn into_file(mut self) -> File {
            let value = self.0.take().expect("pending handle");
            // SAFETY: ownership of this valid pipe handle moves into File.
            unsafe { File::from_raw_handle(value as RawHandle) }
        }
    }

    impl Drop for PendingHandle {
        fn drop(&mut self) {
            if let Some(value) = self.0.take() {
                // SAFETY: this guard uniquely owns the pending handle.
                unsafe {
                    CloseHandle(value);
                }
            }
        }
    }

    pub struct JobControl {
        job: KernelHandle,
        process: KernelHandle,
        process_id: u32,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct JobAccounting {
        pub total_processes: u32,
        pub active_processes: u32,
        pub peak_job_memory_bytes: u64,
    }

    impl JobControl {
        pub fn process_id(&self) -> u32 {
            self.process_id
        }

        pub fn active_processes(&self) -> Result<u32, JobError> {
            Ok(self.accounting()?.active_processes)
        }

        pub fn accounting(&self) -> Result<JobAccounting, JobError> {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { zeroed() };
            // SAFETY: info is correctly sized caller-owned output storage.
            let ok = unsafe {
                QueryInformationJobObject(
                    self.job.raw(),
                    JobObjectBasicAccountingInformation,
                    (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                    size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                    null_mut(),
                )
            };
            if ok == 0 {
                return Err(last_windows_error("QueryInformationJobObject(accounting)"));
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
            // SAFETY: limits is correctly sized caller-owned output storage.
            let ok = unsafe {
                QueryInformationJobObject(
                    self.job.raw(),
                    JobObjectExtendedLimitInformation,
                    (&mut limits as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                    null_mut(),
                )
            };
            if ok == 0 {
                return Err(last_windows_error("QueryInformationJobObject(limits)"));
            }
            Ok(JobAccounting {
                total_processes: info.TotalProcesses,
                active_processes: info.ActiveProcesses,
                peak_job_memory_bytes: limits.PeakJobMemoryUsed as u64,
            })
        }

        /// Return the exact process IDs currently assigned to this Job.
        pub fn process_ids(&self) -> Result<Vec<u32>, JobError> {
            const ERROR_MORE_DATA: i32 = 234;
            let mut capacity = 8usize;
            loop {
                let bytes = size_of::<JOBOBJECT_BASIC_PROCESS_ID_LIST>()
                    + (capacity.saturating_sub(1) * size_of::<usize>());
                let words = bytes.div_ceil(size_of::<usize>());
                let mut storage = vec![0usize; words];
                let list = storage
                    .as_mut_ptr()
                    .cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>();
                // SAFETY: the aligned storage is large enough for the header and
                // `capacity` process IDs, and stays live until the values are copied.
                let ok = unsafe {
                    QueryInformationJobObject(
                        self.job.raw(),
                        JobObjectBasicProcessIdList,
                        list.cast(),
                        bytes as u32,
                        null_mut(),
                    )
                };
                if ok != 0 {
                    // SAFETY: a successful query initialized the header and the
                    // reported number of entries in the variable-length tail.
                    let count = unsafe { (*list).NumberOfProcessIdsInList as usize };
                    let first = unsafe { (*list).ProcessIdList.as_ptr() };
                    let values = unsafe { std::slice::from_raw_parts(first, count) };
                    return values
                        .iter()
                        .map(|value| {
                            u32::try_from(*value).map_err(|_| JobError::InvalidResult {
                                operation: "QueryInformationJobObject(process IDs)",
                            })
                        })
                        .collect();
                }

                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(ERROR_MORE_DATA) {
                    return Err(io_error("QueryInformationJobObject(process IDs)", error));
                }
                // SAFETY: on ERROR_MORE_DATA Windows still fills the header with
                // the assigned process count needed to size the retry buffer.
                let assigned = unsafe { (*list).NumberOfAssignedProcesses as usize };
                capacity = assigned.max(capacity * 2);
            }
        }

        pub fn wait(&self, timeout: Duration) -> Result<bool, JobError> {
            let millis = timeout.as_millis().min(u32::MAX as u128) as u32;
            // SAFETY: process handle remains live through the wait.
            match unsafe { WaitForSingleObject(self.process.raw(), millis) } {
                WAIT_OBJECT_0 => Ok(true),
                WAIT_TIMEOUT => Ok(false),
                u32::MAX => Err(last_windows_error("WaitForSingleObject(process)")),
                _ => Err(JobError::InvalidResult {
                    operation: "WaitForSingleObject(process)",
                }),
            }
        }

        pub fn exit_code(&self) -> Result<Option<u32>, JobError> {
            const STILL_ACTIVE: u32 = 259;
            let mut code = 0u32;
            // SAFETY: code is valid caller-owned output storage and the process handle is live.
            if unsafe { GetExitCodeProcess(self.process.raw(), &mut code) } == 0 {
                return Err(last_windows_error("GetExitCodeProcess"));
            }
            Ok((code != STILL_ACTIVE).then_some(code))
        }

        pub fn terminate(&self) -> Result<(), JobError> {
            // SAFETY: job is owned by this controller and termination is idempotent.
            if unsafe { TerminateJobObject(self.job.raw(), FORCED_EXIT_CODE) } == 0 {
                return Err(last_windows_error("TerminateJobObject"));
            }
            Ok(())
        }

        pub fn terminate_and_confirm(&self, timeout: Duration) -> Result<(), JobError> {
            self.terminate()?;
            self.confirm_zero(timeout)
        }

        pub fn confirm_zero(&self, timeout: Duration) -> Result<(), JobError> {
            let deadline = Instant::now() + timeout;
            loop {
                if self.active_processes()? == 0 {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err(JobError::Timeout {
                        operation: "confirm job process count is zero",
                        timeout_ms: timeout.as_millis(),
                    });
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    impl Drop for JobControl {
        fn drop(&mut self) {
            if self.active_processes().unwrap_or(0) != 0 {
                // SAFETY: closing the job would also kill the tree; terminating first
                // gives tests and shutdown code an observable zero-active confirmation.
                unsafe {
                    TerminateJobObject(self.job.raw(), FORCED_EXIT_CODE);
                }
            }
        }
    }

    pub struct SpawnedJob {
        pub control: std::sync::Arc<JobControl>,
        pub stdin: File,
        pub stdout: File,
        pub stderr: File,
    }

    pub fn spawn_suspended_assigned(
        executable: &Path,
        arguments: &[OsString],
    ) -> Result<SpawnedJob, JobError> {
        spawn_suspended_assigned_with_creation_flags(executable, arguments, 0)
    }

    /// Spawn a process suspended, put it in a kill-on-close Job Object, then
    /// resume it. `additional_creation_flags` lets another helper preserve
    /// harmless scheduling flags without duplicating the handle-safe startup
    /// implementation.
    pub fn spawn_suspended_assigned_with_creation_flags(
        executable: &Path,
        arguments: &[OsString],
        additional_creation_flags: u32,
    ) -> Result<SpawnedJob, JobError> {
        spawn_suspended_assigned_with(
            executable,
            arguments,
            additional_creation_flags,
            |job, process, _process_id| {
                // SAFETY: both handles are live and the process is still suspended.
                unsafe { AssignProcessToJobObject(job, process) }
            },
        )
    }

    fn spawn_suspended_assigned_with<F>(
        executable: &Path,
        arguments: &[OsString],
        additional_creation_flags: u32,
        assign_process: F,
    ) -> Result<SpawnedJob, JobError>
    where
        F: FnOnce(HANDLE, HANDLE, u32) -> i32,
    {
        let executable = canonicalize_executable(executable)?;

        let (child_stdin, parent_stdin) = create_pipe(true)?;
        let (parent_stdout, child_stdout) = create_pipe(false)?;
        let (parent_stderr, child_stderr) = create_pipe(false)?;

        let job = create_kill_on_close_job()?;
        let mut attribute_list = create_handle_attribute_list(&[
            child_stdin.raw(),
            child_stdout.raw(),
            child_stderr.raw(),
        ])?;

        let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = child_stdin.raw();
        startup.StartupInfo.hStdOutput = child_stdout.raw();
        startup.StartupInfo.hStdError = child_stderr.raw();
        startup.lpAttributeList = attribute_list.raw();

        let executable_wide = wide_null(executable.as_os_str());
        let mut command_line = build_command_line(executable.as_os_str(), arguments);
        let mut process_info: PROCESS_INFORMATION = unsafe { zeroed() };
        // SAFETY: all pointers reference initialized storage for the duration of the call;
        // the explicit handle list and its owned handle values remain live through the call.
        let created = unsafe {
            CreateProcessW(
                executable_wide.as_ptr(),
                command_line.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_SUSPENDED
                    | CREATE_NO_WINDOW
                    | EXTENDED_STARTUPINFO_PRESENT
                    | additional_creation_flags,
                null(),
                null(),
                &startup.StartupInfo,
                &mut process_info,
            )
        };
        // Capture the error before cleaning up the attribute list because the cleanup
        // call is allowed to overwrite the thread-local Windows last-error value.
        let spawn_error = (created == 0).then(|| last_windows_error("CreateProcessW"));
        drop(attribute_list);
        if let Some(error) = spawn_error {
            return Err(error);
        }

        let suspended = SuspendedProcessGuard::new(process_info)?;
        drop(child_stdin);
        drop(child_stdout);
        drop(child_stderr);

        // SAFETY: process is still suspended, so neither it nor descendants can escape the job.
        let assigned = assign_process(
            job.raw(),
            suspended.process().raw(),
            process_info.dwProcessId,
        );
        let assign_error = (assigned == 0).then(|| last_windows_error("AssignProcessToJobObject"));
        if let Some(error) = assign_error {
            return Err(error);
        }
        // SAFETY: the primary thread is the suspended thread returned by CreateProcessW.
        let resumed = unsafe { ResumeThread(suspended.thread().raw()) };
        let resume_error = (resumed == u32::MAX).then(|| last_windows_error("ResumeThread"));
        if let Some(error) = resume_error {
            // SAFETY: job assignment already succeeded, so this kills the whole tree.
            unsafe {
                TerminateJobObject(job.raw(), FORCED_EXIT_CODE);
                WaitForSingleObject(suspended.process().raw(), 2_000);
            }
            return Err(error);
        }
        let (process, thread) = suspended.disarm();
        drop(thread);

        Ok(SpawnedJob {
            control: std::sync::Arc::new(JobControl {
                job,
                process,
                process_id: process_info.dwProcessId,
            }),
            stdin: parent_stdin.into_file(),
            stdout: parent_stdout.into_file(),
            stderr: parent_stderr.into_file(),
        })
    }

    fn last_windows_error(operation: &'static str) -> JobError {
        let error = std::io::Error::last_os_error();
        io_error(operation, error)
    }

    fn io_error(operation: &'static str, error: std::io::Error) -> JobError {
        JobError::Windows {
            operation,
            code: error.raw_os_error().unwrap_or(0),
            message: error.to_string(),
        }
    }

    struct ProcessAttributeList {
        storage: Vec<usize>,
        // UpdateProcThreadAttribute stores this pointer instead of copying the
        // handles. Heap ownership keeps the value alive and at a stable address
        // until CreateProcessW has finished consuming the attribute list.
        inherited_handles: Box<[HANDLE]>,
        initialized: bool,
    }

    impl ProcessAttributeList {
        fn raw(&mut self) -> *mut c_void {
            self.storage.as_mut_ptr().cast()
        }
    }

    impl Drop for ProcessAttributeList {
        fn drop(&mut self) {
            if self.initialized {
                // SAFETY: storage contains an attribute list initialized by the
                // constructor and remains allocated until this drop completes.
                unsafe { DeleteProcThreadAttributeList(self.raw()) };
            }
        }
    }

    struct SuspendedProcessGuard {
        process: Option<KernelHandle>,
        thread: Option<KernelHandle>,
        armed: bool,
    }

    impl SuspendedProcessGuard {
        fn new(info: PROCESS_INFORMATION) -> Result<Self, JobError> {
            let process = (!info.hProcess.is_null()).then(|| KernelHandle(info.hProcess));
            let thread = (!info.hThread.is_null()).then(|| KernelHandle(info.hThread));
            let mut value = Self {
                process,
                thread,
                armed: true,
            };
            if value.process.is_none() || value.thread.is_none() {
                value.terminate_and_wait();
                return Err(JobError::InvalidResult {
                    operation: "CreateProcessW handles",
                });
            }
            Ok(value)
        }

        fn process(&self) -> &KernelHandle {
            self.process.as_ref().expect("validated process handle")
        }

        fn thread(&self) -> &KernelHandle {
            self.thread.as_ref().expect("validated thread handle")
        }

        fn terminate_and_wait(&mut self) {
            if let Some(process) = &self.process {
                // SAFETY: the process handle is live and the child is still
                // suspended on every guarded failure path.
                unsafe {
                    TerminateProcess(process.raw(), FORCED_EXIT_CODE);
                    WaitForSingleObject(process.raw(), 2_000);
                }
            }
        }

        fn disarm(mut self) -> (KernelHandle, KernelHandle) {
            self.armed = false;
            (
                self.process.take().expect("validated process handle"),
                self.thread.take().expect("validated thread handle"),
            )
        }
    }

    impl Drop for SuspendedProcessGuard {
        fn drop(&mut self) {
            if self.armed {
                self.terminate_and_wait();
            }
        }
    }

    fn create_pipe(child_reads: bool) -> Result<(PendingHandle, PendingHandle), JobError> {
        let mut read = null_mut();
        let mut write = null_mut();
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: null_mut(),
            bInheritHandle: 1,
        };
        // SAFETY: read/write point to caller-owned HANDLE storage and attributes is valid.
        if unsafe { CreatePipe(&mut read, &mut write, &attributes, 0) } == 0 {
            return Err(last_windows_error("CreatePipe"));
        }
        let read = PendingHandle::new(read);
        let write = PendingHandle::new(write);
        let parent = if child_reads { write.raw() } else { read.raw() };
        // SAFETY: parent is one of the newly created valid pipe handles.
        if unsafe { SetHandleInformation(parent, HANDLE_FLAG_INHERIT, 0) } == 0 {
            return Err(last_windows_error("SetHandleInformation"));
        }
        Ok((read, write))
    }

    fn create_kill_on_close_job() -> Result<KernelHandle, JobError> {
        // SAFETY: unnamed job with default security.
        let raw_job = unsafe { CreateJobObjectW(null(), null()) };
        if raw_job.is_null() {
            return Err(last_windows_error("CreateJobObjectW"));
        }
        let job = KernelHandle(raw_job);
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: limits is a correctly sized initialized input structure.
        if unsafe {
            SetInformationJobObject(
                job.raw(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err(last_windows_error("SetInformationJobObject"));
        }
        Ok(job)
    }

    fn create_handle_attribute_list(handles: &[HANDLE]) -> Result<ProcessAttributeList, JobError> {
        let mut bytes = 0usize;
        // SAFETY: the documented sizing call writes the required byte count.
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes);
        }
        if bytes == 0 {
            return Err(JobError::InvalidResult {
                operation: "InitializeProcThreadAttributeList(size)",
            });
        }
        let words = bytes.div_ceil(size_of::<usize>());
        let storage = vec![0usize; words];
        let inherited_handles = handles.to_vec().into_boxed_slice();
        let mut attribute_list = ProcessAttributeList {
            storage,
            inherited_handles,
            initialized: false,
        };
        let list = attribute_list.raw();
        // SAFETY: storage has the exact size returned by the sizing call.
        if unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut bytes) } == 0 {
            return Err(last_windows_error("InitializeProcThreadAttributeList"));
        }
        attribute_list.initialized = true;
        // SAFETY: list is initialized and the owned handle buffer stays at a stable
        // address until after CreateProcessW has consumed the list.
        let updated = unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                attribute_list.inherited_handles.as_ptr().cast::<c_void>(),
                std::mem::size_of_val(attribute_list.inherited_handles.as_ref()),
                null_mut(),
                null(),
            )
        };
        if updated == 0 {
            return Err(last_windows_error("UpdateProcThreadAttribute(handle list)"));
        }
        Ok(attribute_list)
    }

    fn canonicalize_executable(executable: &Path) -> Result<std::path::PathBuf, JobError> {
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if !executable.is_absolute() {
            return Err(JobError::Executable);
        }
        for component in executable.ancestors() {
            let metadata = std::fs::symlink_metadata(component)
                .map_err(|error| io_error("inspect helper executable", error))?;
            if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(JobError::Executable);
            }
        }
        let canonical = std::fs::canonicalize(executable)
            .map_err(|error| io_error("canonicalize helper executable", error))?;
        if !canonical.is_file() || canonical.to_str().is_none() {
            return Err(JobError::Executable);
        }
        for component in canonical.ancestors() {
            let metadata = std::fs::symlink_metadata(component)
                .map_err(|error| io_error("inspect canonical helper executable", error))?;
            if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(JobError::Executable);
            }
        }
        Ok(canonical)
    }

    fn wide_null(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(std::iter::once(0)).collect()
    }

    fn build_command_line(executable: &OsStr, arguments: &[OsString]) -> Vec<u16> {
        let mut value = quote_windows_arg(executable);
        for argument in arguments {
            value.push(b' ' as u16);
            value.extend(quote_windows_arg(argument));
        }
        value.push(0);
        value
    }

    fn quote_windows_arg(value: &OsStr) -> Vec<u16> {
        let mut output = vec![b'"' as u16];
        let mut slashes = 0usize;
        for unit in value.encode_wide() {
            if unit == b'\\' as u16 {
                slashes += 1;
            } else if unit == b'"' as u16 {
                output.resize(output.len() + slashes * 2 + 1, b'\\' as u16);
                output.push(b'"' as u16);
                slashes = 0;
            } else {
                output.resize(output.len() + slashes, b'\\' as u16);
                slashes = 0;
                output.push(unit);
            }
        }
        output.resize(output.len() + slashes * 2, b'\\' as u16);
        output.push(b'"' as u16);
        output
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::atomic::{AtomicU32, Ordering};
        use windows_sys::Win32::Foundation::{
            SetLastError, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER,
        };

        fn cmd_path() -> std::path::PathBuf {
            std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap())
                .join("System32")
                .join("cmd.exe")
        }

        fn long_ping_arguments() -> Vec<OsString> {
            vec![
                OsString::from("/d"),
                OsString::from("/c"),
                OsString::from("ping -n 30 127.0.0.1 >nul"),
            ]
        }

        #[test]
        fn spawn_error_preserves_win32_code_and_operation() {
            // SAFETY: Windows last-error storage is local to the current test thread.
            unsafe { SetLastError(ERROR_INVALID_PARAMETER) };
            match last_windows_error("CreateProcessW") {
                JobError::Windows {
                    operation,
                    code,
                    message,
                } => {
                    assert_eq!(operation, "CreateProcessW");
                    assert_eq!(code, ERROR_INVALID_PARAMETER as i32);
                    assert!(!message.is_empty());
                }
                error => panic!("unexpected error: {error:?}"),
            }
        }

        #[test]
        fn unicode_space_executable_is_assigned_resumed_and_has_no_residual_process() {
            let temporary = tempfile::tempdir().unwrap();
            let directory = temporary.path().join("中文 空格");
            std::fs::create_dir(&directory).unwrap();
            let executable = directory.join("测试 helper.exe");
            std::fs::copy(cmd_path(), &executable).unwrap();

            let process = spawn_suspended_assigned(&executable, &long_ping_arguments()).unwrap();
            assert!(process.control.process_id() != 0);
            assert!(process.control.active_processes().unwrap() >= 1);
            process
                .control
                .terminate_and_confirm(Duration::from_secs(2))
                .unwrap();
            assert_eq!(process.control.active_processes().unwrap(), 0);
        }

        #[test]
        fn explicit_handle_list_allows_stdin_eof_without_a_leaked_writer() {
            let process = spawn_suspended_assigned(
                &cmd_path(),
                &[
                    OsString::from("/d"),
                    OsString::from("/c"),
                    OsString::from("more >nul"),
                ],
            )
            .unwrap();
            drop(process.stdin);
            assert!(process.control.wait(Duration::from_secs(3)).unwrap());
            process
                .control
                .confirm_zero(Duration::from_secs(2))
                .unwrap();
            assert_eq!(process.control.active_processes().unwrap(), 0);
        }

        #[test]
        fn assignment_failure_terminates_suspended_process() {
            use windows_sys::Win32::System::Threading::OpenProcess;

            const SYNCHRONIZE: u32 = 0x0010_0000;
            let captured_pid = AtomicU32::new(0);
            let result = spawn_suspended_assigned_with(
                &cmd_path(),
                &long_ping_arguments(),
                0,
                |_job, _process, process_id| {
                    captured_pid.store(process_id, Ordering::SeqCst);
                    // SAFETY: the injected failure uses the calling thread's
                    // last-error storage exactly like the real Windows API.
                    unsafe { SetLastError(ERROR_ACCESS_DENIED) };
                    0
                },
            );
            let error = match result {
                Ok(process) => {
                    drop(process);
                    panic!("injected assignment failure unexpectedly succeeded")
                }
                Err(error) => error,
            };
            match error {
                JobError::Windows {
                    operation, code, ..
                } => {
                    assert_eq!(operation, "AssignProcessToJobObject");
                    assert_eq!(code, ERROR_ACCESS_DENIED as i32);
                }
                error => panic!("unexpected error: {error:?}"),
            }

            let process_id = captured_pid.load(Ordering::SeqCst);
            assert_ne!(process_id, 0);
            // If the process object still exists, it must already be signalled.
            let observed = unsafe { OpenProcess(SYNCHRONIZE, 0, process_id) };
            if !observed.is_null() {
                assert_eq!(unsafe { WaitForSingleObject(observed, 0) }, WAIT_OBJECT_0);
                unsafe { CloseHandle(observed) };
            }
        }

        #[test]
        fn immediate_grandchild_is_captured_and_terminated_with_the_job() {
            let temporary = tempfile::tempdir().unwrap();
            let script = temporary.path().join("立即 启动孙进程.cmd");
            std::fs::write(
                &script,
                concat!(
                    "@echo off\r\n",
                    "start \"\" /b \"%SystemRoot%\\System32\\ping.exe\" ",
                    "-n 30 127.0.0.1 >nul\r\n",
                    "\"%SystemRoot%\\System32\\ping.exe\" -n 30 127.0.0.1 >nul\r\n"
                ),
            )
            .unwrap();
            let process = spawn_suspended_assigned(
                &cmd_path(),
                &[
                    OsString::from("/d"),
                    OsString::from("/c"),
                    script.into_os_string(),
                ],
            )
            .unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut observed_descendant = false;
            while Instant::now() < deadline {
                if process.control.active_processes().unwrap() >= 2 {
                    observed_descendant = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(observed_descendant);
            process
                .control
                .terminate_and_confirm(Duration::from_secs(2))
                .unwrap();
            assert_eq!(process.control.active_processes().unwrap(), 0);
        }

        #[test]
        fn dropping_last_job_control_kills_the_parent_process() {
            use windows_sys::Win32::System::Threading::OpenProcess;

            const SYNCHRONIZE: u32 = 0x0010_0000;
            let process = spawn_suspended_assigned(&cmd_path(), &long_ping_arguments()).unwrap();
            let pid = process.control.process_id();
            let observed = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
            assert!(!observed.is_null());
            drop(process);
            assert_eq!(
                unsafe { WaitForSingleObject(observed, 3_000) },
                WAIT_OBJECT_0
            );
            unsafe { CloseHandle(observed) };
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::ffi::OsString;
    use std::fs::File;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    #[derive(Debug, thiserror::Error)]
    #[error("MOSS helper process isolation is only supported on Windows")]
    pub struct JobError;

    pub struct JobControl;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct JobAccounting {
        pub total_processes: u32,
        pub active_processes: u32,
        pub peak_job_memory_bytes: u64,
    }

    impl JobControl {
        pub fn process_id(&self) -> u32 {
            0
        }

        pub fn active_processes(&self) -> Result<u32, JobError> {
            Err(JobError)
        }

        pub fn accounting(&self) -> Result<JobAccounting, JobError> {
            Err(JobError)
        }

        pub fn process_ids(&self) -> Result<Vec<u32>, JobError> {
            Err(JobError)
        }

        pub fn wait(&self, _timeout: Duration) -> Result<bool, JobError> {
            Err(JobError)
        }

        pub fn exit_code(&self) -> Result<Option<u32>, JobError> {
            Err(JobError)
        }

        pub fn terminate(&self) -> Result<(), JobError> {
            Err(JobError)
        }

        pub fn terminate_and_confirm(&self, _timeout: Duration) -> Result<(), JobError> {
            Err(JobError)
        }

        pub fn confirm_zero(&self, _timeout: Duration) -> Result<(), JobError> {
            Err(JobError)
        }
    }

    pub struct SpawnedJob {
        pub control: Arc<JobControl>,
        pub stdin: File,
        pub stdout: File,
        pub stderr: File,
    }

    pub fn spawn_suspended_assigned(
        _executable: &Path,
        _arguments: &[OsString],
    ) -> Result<SpawnedJob, JobError> {
        Err(JobError)
    }

    pub fn spawn_suspended_assigned_with_creation_flags(
        _executable: &Path,
        _arguments: &[OsString],
        _additional_creation_flags: u32,
    ) -> Result<SpawnedJob, JobError> {
        Err(JobError)
    }
}

pub use imp::*;
