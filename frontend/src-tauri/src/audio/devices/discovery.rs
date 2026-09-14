use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use log::error;

use super::configuration::{AudioDevice, DeviceType};
use super::platform;

/// List all available audio devices on the system
pub async fn list_audio_devices() -> Result<Vec<AudioDevice>> {
    let host = cpal::default_host();

    // Platform-specific device enumeration
    let mut devices = {
        #[cfg(target_os = "windows")]
        {
            platform::configure_windows_audio(&host)?
        }

        #[cfg(target_os = "linux")]
        {
            platform::configure_linux_audio(&host)?
        }

        #[cfg(target_os = "macos")]
        {
            platform::configure_macos_audio(&host)?
        }
    };

    // Windows devices above are already the complete WASAPI list and carry a
    // native endpoint ID. Adding default-host names here would reintroduce
    // identity-less duplicates.
    #[cfg(not(target_os = "windows"))]
    if let Ok(other_devices) = host.devices() {
        for device in other_devices {
            if let Ok(name) = device.name() {
                if !devices.iter().any(|d| d.name == name) {
                    devices.push(AudioDevice::new(name, DeviceType::Output));
                }
            }
        }
    }

    Ok(devices)
}

/// Resolve a persisted/UI selection to the current device object. Native IDs
/// are authoritative; the display-name form remains only for old preferences
/// and is rejected when it is ambiguous.
pub async fn resolve_audio_device(selection: &str, device_type: DeviceType) -> Result<AudioDevice> {
    let devices = list_audio_devices().await?;
    select_audio_device(devices, selection, device_type)
}

fn select_audio_device(
    devices: Vec<AudioDevice>,
    selection: &str,
    device_type: DeviceType,
) -> Result<AudioDevice> {
    if let Some(device) = devices.iter().find(|device| {
        device.device_type == device_type && device.native_id.as_deref() == Some(selection)
    }) {
        return Ok(device.clone());
    }

    let parsed = AudioDevice::from_name(selection)?;
    if parsed.device_type != device_type {
        anyhow::bail!("Audio device selection has the wrong device type: {selection}");
    }
    let matching: Vec<_> = devices
        .into_iter()
        .filter(|device| device.device_type == device_type && device.name == parsed.name)
        .collect();
    match matching.as_slice() {
        [device] => Ok(device.clone()),
        [] => anyhow::bail!("Audio device is not active: {selection}"),
        _ => anyhow::bail!(
            "Audio device name is ambiguous; refresh settings and select the endpoint again: {selection}"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(name: &str, id: &str) -> AudioDevice {
        AudioDevice::with_native_id(name.to_string(), DeviceType::Output, id.to_string())
    }

    #[test]
    fn native_id_selects_the_exact_same_named_endpoint() {
        let selected = select_audio_device(
            vec![
                output("Headphones", "endpoint-a"),
                output("Headphones", "endpoint-b"),
            ],
            "endpoint-b",
            DeviceType::Output,
        )
        .expect("native ID must resolve");

        assert_eq!(selected.native_id.as_deref(), Some("endpoint-b"));
    }

    #[test]
    fn legacy_name_is_rejected_when_same_named_endpoints_exist() {
        let error = select_audio_device(
            vec![
                output("Headphones", "endpoint-a"),
                output("Headphones", "endpoint-b"),
            ],
            "Headphones (output)",
            DeviceType::Output,
        )
        .expect_err("same-name legacy selection must not guess");

        assert!(error.to_string().contains("ambiguous"));
    }

    #[test]
    fn endpoint_from_the_wrong_flow_is_not_accepted() {
        let error = select_audio_device(
            vec![output("Headphones", "endpoint-a")],
            "endpoint-a",
            DeviceType::Input,
        )
        .expect_err("output endpoint must not satisfy an input request");

        assert!(error.to_string().contains("not specified"));
    }
}

/// Trigger audio permission request on platforms that require it
/// Returns Ok(true) if permission is granted, Ok(false) if denied, Err if something went wrong
pub fn trigger_audio_permission() -> Result<bool> {
    use log::info;

    let host = cpal::default_host();
    let device = match host.default_input_device() {
        Some(d) => d,
        None => {
            info!("[trigger_audio_permission] No default input device found - permission likely denied");
            return Ok(false);
        }
    };

    let config = match device.default_input_config() {
        Ok(c) => c,
        Err(e) => {
            info!("[trigger_audio_permission] Failed to get input config: {} - permission likely denied", e);
            return Ok(false);
        }
    };

    // Build and start an input stream to trigger the permission request
    let stream = match device.build_input_stream(
        &config.into(),
        |_data: &[f32], _: &cpal::InputCallbackInfo| {
            // Do nothing, we just want to trigger the permission request
        },
        |err| error!("Error in audio stream: {}", err),
        None,
    ) {
        Ok(s) => s,
        Err(e) => {
            info!("[trigger_audio_permission] Failed to build input stream: {} - permission likely denied", e);
            return Ok(false);
        }
    };

    // Start the stream to actually trigger the permission dialog
    if let Err(e) = stream.play() {
        info!(
            "[trigger_audio_permission] Failed to play stream: {} - permission likely denied",
            e
        );
        return Ok(false);
    }

    // Sleep briefly to allow the permission dialog to appear and for stream to actually work
    std::thread::sleep(std::time::Duration::from_millis(500));

    // If we got here, permission was granted
    info!("[trigger_audio_permission] Stream played successfully - permission granted");

    // Stop the stream
    drop(stream);

    Ok(true)
}
