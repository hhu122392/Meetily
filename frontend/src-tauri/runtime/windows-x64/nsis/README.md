# NSIS DirectML payload

Tauri 2.11.1 automatically includes the generated adjacent `DirectML.dll` in
WiX/MSI output but omits it from NSIS output. The NSIS installer hook packages
this frozen copy beside `meetily.exe` and removes it on uninstall.

- File version: `1.15.4+241025-1615.1.dml-1.15.fac7597`
- Architecture: x64
- Authenticode status at capture: `Valid`
- Signer: Microsoft Windows Publisher
- Bytes: `18527776`
- SHA-256: `9C9E6D822561C6C41B90E6994B3E8857CF1D66DBFB1E0C4C799C7C89B4E92DA1`

The copy must remain byte-identical to the `DirectML.dll` emitted with the
candidate executable. The clean-Sandbox harness checks the installed hash for
both MSI and NSIS packages.
