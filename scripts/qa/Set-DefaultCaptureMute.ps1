param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('get', 'mute', 'unmute')]
    [string]$Action
)

$ErrorActionPreference = 'Stop'

if (-not ('MeetilyQa.AudioEndpoint' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

namespace MeetilyQa {
    enum EDataFlow { eRender = 0, eCapture = 1, eAll = 2 }
    enum ERole { eConsole = 0, eMultimedia = 1, eCommunications = 2 }

    [ComImport, Guid("D666063F-1587-4E43-81F1-B948E807363F"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IMMDevice {
        [PreserveSig] int Activate(ref Guid iid, uint clsCtx, IntPtr activationParams,
            [MarshalAs(UnmanagedType.IUnknown)] out object interfacePointer);
    }

    [ComImport, Guid("A95664D2-9614-4F35-A746-DE8DB63617E6"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IMMDeviceEnumerator {
        [PreserveSig] int EnumAudioEndpoints(EDataFlow dataFlow, uint stateMask, out IntPtr devices);
        [PreserveSig] int GetDefaultAudioEndpoint(EDataFlow dataFlow, ERole role, out IMMDevice endpoint);
    }

    [ComImport, Guid("5CDF2C82-841E-4546-9722-0CF74078229A"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IAudioEndpointVolume {
        [PreserveSig] int RegisterControlChangeNotify(IntPtr notify);
        [PreserveSig] int UnregisterControlChangeNotify(IntPtr notify);
        [PreserveSig] int GetChannelCount(out uint count);
        [PreserveSig] int SetMasterVolumeLevel(float levelDb, Guid context);
        [PreserveSig] int SetMasterVolumeLevelScalar(float level, Guid context);
        [PreserveSig] int GetMasterVolumeLevel(out float levelDb);
        [PreserveSig] int GetMasterVolumeLevelScalar(out float level);
        [PreserveSig] int SetChannelVolumeLevel(uint channel, float levelDb, Guid context);
        [PreserveSig] int SetChannelVolumeLevelScalar(uint channel, float level, Guid context);
        [PreserveSig] int GetChannelVolumeLevel(uint channel, out float levelDb);
        [PreserveSig] int GetChannelVolumeLevelScalar(uint channel, out float level);
        [PreserveSig] int SetMute([MarshalAs(UnmanagedType.Bool)] bool mute, Guid context);
        [PreserveSig] int GetMute([MarshalAs(UnmanagedType.Bool)] out bool mute);
    }

    [ComImport, Guid("BCDE0395-E52F-467C-8E3D-C4579291692E")]
    class MMDeviceEnumerator { }

    public static class AudioEndpoint {
        static IAudioEndpointVolume GetDefaultCaptureVolume() {
            var enumerator = (IMMDeviceEnumerator)(new MMDeviceEnumerator());
            IMMDevice device;
            int result = enumerator.GetDefaultAudioEndpoint(EDataFlow.eCapture, ERole.eMultimedia, out device);
            if (result != 0) Marshal.ThrowExceptionForHR(result);
            Guid iid = typeof(IAudioEndpointVolume).GUID;
            object endpoint;
            result = device.Activate(ref iid, 23, IntPtr.Zero, out endpoint);
            if (result != 0) Marshal.ThrowExceptionForHR(result);
            return (IAudioEndpointVolume)endpoint;
        }

        public static bool GetMute() {
            bool muted;
            int result = GetDefaultCaptureVolume().GetMute(out muted);
            if (result != 0) Marshal.ThrowExceptionForHR(result);
            return muted;
        }

        public static void SetMute(bool muted) {
            int result = GetDefaultCaptureVolume().SetMute(muted, Guid.Empty);
            if (result != 0) Marshal.ThrowExceptionForHR(result);
        }
    }
}
'@
}

$before = [MeetilyQa.AudioEndpoint]::GetMute()
switch ($Action) {
    'mute' { [MeetilyQa.AudioEndpoint]::SetMute($true) }
    'unmute' { [MeetilyQa.AudioEndpoint]::SetMute($false) }
}
$after = [MeetilyQa.AudioEndpoint]::GetMute()

[pscustomobject]@{
    CapturedAt = (Get-Date).ToString('o')
    Action = $Action
    Before = $before
    After = $after
} | ConvertTo-Json
