#define UNICODE
#define _UNICODE
#include <windows.h>

#include <iostream>
#include <string>
#include <vector>

namespace {

struct PipeSet {
    HANDLE child_stdin = nullptr;
    HANDLE parent_stdin = nullptr;
    HANDLE parent_stdout = nullptr;
    HANDLE child_stdout = nullptr;
    HANDLE parent_stderr = nullptr;
    HANDLE child_stderr = nullptr;
};

void CloseIfValid(HANDLE value) {
    if (value != nullptr && value != INVALID_HANDLE_VALUE) {
        CloseHandle(value);
    }
}

bool CreatePipes(PipeSet* pipes) {
    SECURITY_ATTRIBUTES attributes{};
    attributes.nLength = sizeof(attributes);
    attributes.bInheritHandle = TRUE;
    if (!CreatePipe(&pipes->child_stdin, &pipes->parent_stdin, &attributes, 0) ||
        !CreatePipe(&pipes->parent_stdout, &pipes->child_stdout, &attributes, 0) ||
        !CreatePipe(&pipes->parent_stderr, &pipes->child_stderr, &attributes, 0)) {
        return false;
    }
    return SetHandleInformation(pipes->parent_stdin, HANDLE_FLAG_INHERIT, 0) &&
           SetHandleInformation(pipes->parent_stdout, HANDLE_FLAG_INHERIT, 0) &&
           SetHandleInformation(pipes->parent_stderr, HANDLE_FLAG_INHERIT, 0);
}

void ClosePipes(PipeSet* pipes) {
    CloseIfValid(pipes->child_stdin);
    CloseIfValid(pipes->parent_stdin);
    CloseIfValid(pipes->parent_stdout);
    CloseIfValid(pipes->child_stdout);
    CloseIfValid(pipes->parent_stderr);
    CloseIfValid(pipes->child_stderr);
    *pipes = {};
}

void RunCase(const wchar_t* name, bool use_handle_list, bool use_extended_path) {
    PipeSet pipes{};
    if (!CreatePipes(&pipes)) {
        std::wcout << name << L" pipe_error=" << GetLastError() << L"\n";
        ClosePipes(&pipes);
        return;
    }

    STARTUPINFOEXW startup{};
    startup.StartupInfo.cb = use_handle_list ? sizeof(startup) : sizeof(startup.StartupInfo);
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = pipes.child_stdin;
    startup.StartupInfo.hStdOutput = pipes.child_stdout;
    startup.StartupInfo.hStdError = pipes.child_stderr;

    std::vector<unsigned char> attribute_storage;
    if (use_handle_list) {
        SIZE_T bytes = 0;
        InitializeProcThreadAttributeList(nullptr, 1, 0, &bytes);
        attribute_storage.resize(bytes);
        startup.lpAttributeList = reinterpret_cast<LPPROC_THREAD_ATTRIBUTE_LIST>(
            attribute_storage.data());
        if (!InitializeProcThreadAttributeList(startup.lpAttributeList, 1, 0, &bytes)) {
            std::wcout << name << L" attribute_init_error=" << GetLastError() << L"\n";
            ClosePipes(&pipes);
            return;
        }
        HANDLE inherited[] = {pipes.child_stdin, pipes.child_stdout, pipes.child_stderr};
        if (!UpdateProcThreadAttribute(
                startup.lpAttributeList,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
                inherited,
                sizeof(inherited),
                nullptr,
                nullptr)) {
            std::wcout << name << L" attribute_update_error=" << GetLastError() << L"\n";
            DeleteProcThreadAttributeList(startup.lpAttributeList);
            ClosePipes(&pipes);
            return;
        }
    }

    std::wstring application = L"C:\\Windows\\System32\\cmd.exe";
    if (use_extended_path) {
        application = L"\\\\?\\" + application;
    }
    std::wstring command_line = L"\"" + application + L"\" /d /c exit 0";
    PROCESS_INFORMATION process{};
    DWORD flags = CREATE_SUSPENDED | CREATE_NO_WINDOW;
    if (use_handle_list) {
        flags |= EXTENDED_STARTUPINFO_PRESENT;
    }
    SetLastError(ERROR_SUCCESS);
    const BOOL created = CreateProcessW(
        application.c_str(),
        command_line.data(),
        nullptr,
        nullptr,
        TRUE,
        flags,
        nullptr,
        nullptr,
        &startup.StartupInfo,
        &process);
    const DWORD error = created ? ERROR_SUCCESS : GetLastError();
    std::wcout << name << L" created=" << created << L" error=" << error << L"\n";

    if (startup.lpAttributeList != nullptr) {
        DeleteProcThreadAttributeList(startup.lpAttributeList);
    }
    CloseIfValid(pipes.child_stdin);
    pipes.child_stdin = nullptr;
    CloseIfValid(pipes.child_stdout);
    pipes.child_stdout = nullptr;
    CloseIfValid(pipes.child_stderr);
    pipes.child_stderr = nullptr;
    if (created) {
        ResumeThread(process.hThread);
        CloseHandle(pipes.parent_stdin);
        pipes.parent_stdin = nullptr;
        WaitForSingleObject(process.hProcess, 2'000);
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
    }
    ClosePipes(&pipes);
}

}  // namespace

int wmain() {
    std::wcout << L"startup_info=" << sizeof(STARTUPINFOW)
               << L" startup_info_ex=" << sizeof(STARTUPINFOEXW)
               << L" handle=" << sizeof(HANDLE) << L"\n";
    RunCase(L"plain", false, false);
    RunCase(L"handle_list", true, false);
    RunCase(L"handle_list_extended_path", true, true);
    return 0;
}
