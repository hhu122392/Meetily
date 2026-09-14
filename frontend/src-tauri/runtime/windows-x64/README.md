# Windows x64 application-local runtime

These release-mode DLLs are copied from Microsoft Visual C++ Build Tools and are
bundled beside `meetily.exe`. They make both the main application and
`llama-helper.exe` runnable on clean Windows installations that do not already
have the Microsoft Visual C++ 2015–2022 Redistributable installed.

- Source toolset: Microsoft Visual C++ v143
- Source version: `14.44.35211.0`
- Architecture: x64
- Source directories:
  - `VC/Redist/MSVC/14.44.35112/x64/Microsoft.VC143.CRT`
  - `VC/Redist/MSVC/14.44.35112/x64/Microsoft.VC143.OpenMP`
- Deployment model: application-local; installers copy the DLLs into the
  application directory and remove them with the application.
- Debug and `debug_nonredist` binaries are intentionally excluded.

`manifest.json` is the authoritative byte-length, version, and SHA-256 inventory.
Redistribution remains subject to the Microsoft Visual Studio license applicable
to the Build Tools installation used to produce the release.
