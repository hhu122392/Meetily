# Read-only Windows endpoint identity, mute and volume probe.
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace MeetilyDefaultAudioQa {
 [ComImport, Guid("BCDE0395-E52F-467C-8E3D-C4579291692E")] class Enumerator {}
 [ComImport, Guid("A95664D2-9614-4F35-A746-DE8DB63617E6"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
 interface IEnum {
  [PreserveSig] int EnumAudioEndpoints(int flow, uint mask, out IntPtr devices);
  [PreserveSig] int GetDefaultAudioEndpoint(int flow, int role, out IDevice device);
 }
 [ComImport, Guid("D666063F-1587-4E43-81F1-B948E807363F"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
 interface IDevice {
  [PreserveSig] int Activate(ref Guid iid, uint context, IntPtr parameters, [MarshalAs(UnmanagedType.IUnknown)] out object result);
  [PreserveSig] int OpenPropertyStore(uint mode, out IntPtr store);
  [PreserveSig] int GetId([MarshalAs(UnmanagedType.LPWStr)] out string id);
 }
 [ComImport, Guid("5CDF2C82-841E-4546-9722-0CF74078229A"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
 interface IVolume {
  [PreserveSig] int RegisterControlChangeNotify(IntPtr notify);
  [PreserveSig] int UnregisterControlChangeNotify(IntPtr notify);
  [PreserveSig] int GetChannelCount(out uint count);
  [PreserveSig] int SetMasterVolumeLevel(float value, Guid context);
  [PreserveSig] int SetMasterVolumeLevelScalar(float value, Guid context);
  [PreserveSig] int GetMasterVolumeLevel(out float value);
  [PreserveSig] int GetMasterVolumeLevelScalar(out float value);
  [PreserveSig] int SetChannelVolumeLevel(uint channel, float value, Guid context);
  [PreserveSig] int SetChannelVolumeLevelScalar(uint channel, float value, Guid context);
  [PreserveSig] int GetChannelVolumeLevel(uint channel, out float value);
  [PreserveSig] int GetChannelVolumeLevelScalar(uint channel, out float value);
  [PreserveSig] int SetMute([MarshalAs(UnmanagedType.Bool)] bool mute, Guid context);
  [PreserveSig] int GetMute([MarshalAs(UnmanagedType.Bool)] out bool mute);
 }
 public class Snapshot {
  public int Flow; public int Role; public string Id; public bool Muted; public float Volume;
 }
 public static class Probe {
  static void Check(int hr) { Marshal.ThrowExceptionForHR(hr); }
  public static Snapshot Read(int flow, int role) {
   IEnum enumerator = (IEnum)new Enumerator(); IDevice device=null; object volumeObject=null;
   try {
    Check(enumerator.GetDefaultAudioEndpoint(flow,role,out device));
    string id; Check(device.GetId(out id));
    Guid iid=typeof(IVolume).GUID; Check(device.Activate(ref iid,23,IntPtr.Zero,out volumeObject));
    IVolume volume=(IVolume)volumeObject; bool muted; float level;
    Check(volume.GetMute(out muted)); Check(volume.GetMasterVolumeLevelScalar(out level));
    return new Snapshot { Flow=flow, Role=role, Id=id, Muted=muted, Volume=level };
   } finally {
    if(volumeObject!=null) Marshal.ReleaseComObject(volumeObject);
    if(device!=null) Marshal.ReleaseComObject(device);
    Marshal.ReleaseComObject(enumerator);
   }
  }
 }
}
'@
$records = foreach ($flow in 0,1) {
    foreach ($role in 0,1,2) {
        [MeetilyDefaultAudioQa.Probe]::Read($flow,$role)
    }
}
[pscustomobject]@{captured_at=(Get-Date).ToString('o'); endpoints=@($records)} | ConvertTo-Json -Depth 4
