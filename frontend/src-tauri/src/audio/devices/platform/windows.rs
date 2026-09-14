use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait};
use log::{debug, info, warn};
use windows::core::Interface;
use windows::Win32::Media::Audio::{
    eAll, eCapture, eMultimedia, eRender, IMMDeviceEnumerator, IMMEndpoint, MMDeviceEnumerator,
    DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED,
};

use crate::audio::devices::configuration::{AudioDevice, DeviceType};

#[derive(Debug)]
struct NativeEndpoint {
    id: String,
    device_type: DeviceType,
}

fn native_endpoints() -> Result<Vec<NativeEndpoint>> {
    unsafe {
        // A process may already have initialized this thread in another COM
        // apartment. That is fine: the enumerator can still be created.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|error| anyhow!("Failed to create MMDeviceEnumerator: {error}"))?;
        let collection = enumerator
            .EnumAudioEndpoints(eAll, DEVICE_STATE_ACTIVE)
            .map_err(|error| {
                anyhow!("Failed to enumerate active Windows audio endpoints: {error}")
            })?;
        let count = collection
            .GetCount()
            .map_err(|error| anyhow!("Failed to count Windows audio endpoints: {error}"))?;
        let mut endpoints = Vec::with_capacity(count as usize);
        for index in 0..count {
            let endpoint = collection.Item(index).map_err(|error| {
                anyhow!("Failed to read Windows audio endpoint {index}: {error}")
            })?;
            let id = endpoint.GetId().map_err(|error| {
                anyhow!("Failed to read Windows audio endpoint ID {index}: {error}")
            })?;
            let value = id.to_string().map_err(|error| {
                anyhow!("Windows audio endpoint ID {index} is invalid: {error}")
            })?;
            CoTaskMemFree(Some(id.0.cast()));
            let flow = endpoint
                .cast::<IMMEndpoint>()
                .and_then(|endpoint| endpoint.GetDataFlow())
                .map_err(|error| {
                    anyhow!("Failed to read Windows audio endpoint flow {index}: {error}")
                })?;
            let device_type = if flow == eCapture {
                DeviceType::Input
            } else if flow == eRender {
                DeviceType::Output
            } else {
                return Err(anyhow!(
                    "Windows audio endpoint {value} has unsupported flow: {:?}",
                    flow
                ));
            };
            endpoints.push(NativeEndpoint {
                id: value,
                device_type,
            });
        }
        Ok(endpoints)
    }
}

fn default_native_endpoint_id(flow: windows::Win32::Media::Audio::EDataFlow) -> Result<String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|error| anyhow!("Failed to create MMDeviceEnumerator: {error}"))?;
        let endpoint = enumerator
            .GetDefaultAudioEndpoint(flow, eMultimedia)
            .map_err(|error| anyhow!("Windows default audio endpoint is unavailable: {error}"))?;
        let id = endpoint.GetId().map_err(|error| {
            anyhow!("Failed to read default Windows audio endpoint ID: {error}")
        })?;
        let value = id
            .to_string()
            .map_err(|error| anyhow!("Default Windows audio endpoint ID is invalid: {error}"))?;
        CoTaskMemFree(Some(id.0.cast()));
        Ok(value)
    }
}

fn enumerate_cpal_devices_with_ids(
    wasapi_host: &cpal::Host,
    device_type: DeviceType,
) -> Result<Vec<AudioDevice>> {
    let cpal_devices: Vec<_> = wasapi_host.devices()?.collect();
    let native_endpoints = native_endpoints()?;

    if cpal_devices.len() != native_endpoints.len() {
        return Err(anyhow!(
            "Windows audio endpoint mapping is unsafe: CPAL returned {} devices but MMDevice returned {} IDs",
            cpal_devices.len(),
            native_endpoints.len()
        ));
    }

    cpal_devices
        .into_iter()
        .zip(native_endpoints)
        .filter(|(_, endpoint)| endpoint.device_type == device_type)
        .map(|(device, endpoint)| {
            let name = device
                .name()
                .map_err(|error| anyhow!("Failed to read Windows audio device name: {error}"))?;
            Ok(AudioDevice::with_native_id(
                name,
                device_type.clone(),
                endpoint.id,
            ))
        })
        .collect()
}

fn find_cpal_device_by_native_id(
    wasapi_host: &cpal::Host,
    audio_device: &AudioDevice,
) -> Result<cpal::Device> {
    let native_id = audio_device.native_id.as_deref().ok_or_else(|| {
        anyhow!(
            "Windows audio selection '{}' has no native endpoint ID; refresh the device list and select it again",
            audio_device.name
        )
    })?;
    let endpoints = native_endpoints()?;
    let index = endpoints
        .iter()
        .position(|candidate| {
            candidate.id == native_id && candidate.device_type == audio_device.device_type
        })
        .ok_or_else(|| anyhow!("Windows audio endpoint is no longer active: {native_id}"))?;
    let device = wasapi_host.devices()?.nth(index);
    device.ok_or_else(|| {
        anyhow!("Windows audio endpoint mapping changed while binding endpoint: {native_id}")
    })
}

/// Configure Windows audio devices using WASAPI
pub fn configure_windows_audio(_host: &cpal::Host) -> Result<Vec<AudioDevice>> {
    let mut devices = Vec::new();

    // Get WASAPI devices
    if let Ok(wasapi_host) = cpal::host_from_id(cpal::HostId::Wasapi) {
        debug!("Using WASAPI host for Windows audio device enumeration");

        devices.extend(enumerate_cpal_devices_with_ids(
            &wasapi_host,
            DeviceType::Output,
        )?);
        devices.extend(enumerate_cpal_devices_with_ids(
            &wasapi_host,
            DeviceType::Input,
        )?);
    } else {
        return Err(anyhow!("Failed to create WASAPI host"));
    }

    if devices.is_empty() {
        return Err(anyhow!("WASAPI returned no active audio endpoints"));
    }

    debug!("Found {} Windows audio devices", devices.len());
    Ok(devices)
}

pub fn default_windows_audio_device(device_type: DeviceType) -> Result<AudioDevice> {
    let wasapi_host = cpal::host_from_id(cpal::HostId::Wasapi)
        .map_err(|error| anyhow!("Failed to create WASAPI host: {error}"))?;
    let native_id = match device_type {
        DeviceType::Input => default_native_endpoint_id(eCapture)?,
        DeviceType::Output => default_native_endpoint_id(eRender)?,
    };
    enumerate_cpal_devices_with_ids(&wasapi_host, device_type)?
        .into_iter()
        .find(|device| device.native_id.as_deref() == Some(native_id.as_str()))
        .map(|device| device.with_default_role("multimedia"))
        .ok_or_else(|| anyhow!("Default Windows audio endpoint is no longer active: {native_id}"))
}

pub fn default_windows_endpoint_id(device_type: DeviceType) -> Result<String> {
    match device_type {
        DeviceType::Input => default_native_endpoint_id(eCapture),
        DeviceType::Output => default_native_endpoint_id(eRender),
    }
}

/// Get Windows device and configuration using WASAPI
pub fn get_windows_device(
    audio_device: &AudioDevice,
) -> Result<(cpal::Device, cpal::SupportedStreamConfig)> {
    let wasapi_host = cpal::host_from_id(cpal::HostId::Wasapi)
        .map_err(|e| anyhow!("Failed to create WASAPI host: {}", e))?;

    let device = find_cpal_device_by_native_id(&wasapi_host, audio_device)?;
    info!(
        "Binding Windows {:?} endpoint by native ID: {} ({})",
        audio_device.device_type,
        audio_device.native_id.as_deref().unwrap_or("missing"),
        audio_device.name
    );

    match audio_device.device_type {
        DeviceType::Input => {
            if let Ok(name) = device.name() {
                // info!("Found matching input device: {}", name);

                // Try to get default input config with better error logging
                match device.default_input_config() {
                    Ok(default_config) => {
                        // info!("Using default input config: {:?}", default_config);
                        return Ok((device, default_config));
                    }
                    Err(e) => {
                        warn!(
                            "Failed to get default input config: {}. Trying supported configs...",
                            e
                        );

                        // Try to find a supported configuration
                        if let Ok(supported_configs) = device.supported_input_configs() {
                            let configs: Vec<_> = supported_configs.collect();
                            if configs.is_empty() {
                                warn!(
                                    "No supported input configurations found for device: {}",
                                    name
                                );
                            } else {
                                // info!("Found {} supported input configurations", configs.len());

                                // First try to find F32 format with 2 channels (stereo)
                                for config in &configs {
                                    if config.sample_format() == cpal::SampleFormat::F32
                                        && config.channels() == 2
                                    {
                                        let config = config.with_max_sample_rate();
                                        // info!("Using stereo F32 input config: {:?}", config);
                                        return Ok((device, config));
                                    }
                                }

                                // Then try any F32 format
                                for config in &configs {
                                    if config.sample_format() == cpal::SampleFormat::F32 {
                                        let config = config.with_max_sample_rate();
                                        // info!("Using F32 input config: {:?}", config);
                                        return Ok((device, config));
                                    }
                                }

                                // Finally, use the first available config
                                let config = configs[0].with_max_sample_rate();
                                info!("Using fallback input config: {:?}", config);
                                return Ok((device, config));
                            }
                        } else {
                            warn!(
                                "Could not enumerate supported configurations for device: {}",
                                name
                            );
                        }

                        return Err(anyhow!(
                            "No compatible input configuration found for device: {}",
                            name
                        ));
                    }
                }
            }
        }
        DeviceType::Output => {
            if let Ok(name) = device.name() {
                // info!("Found matching output device: {}", name);

                // For output devices, we want to use them in loopback mode
                if let Ok(supported_configs) = device.supported_output_configs() {
                    let configs: Vec<_> = supported_configs.collect();
                    if configs.is_empty() {
                        warn!(
                            "No supported output configurations found for device: {}",
                            name
                        );
                    } else {
                        // info!("Found {} supported output configurations", configs.len());

                        // Try to find a config that supports f32 format with 2 channels (stereo)
                        for config in &configs {
                            if config.sample_format() == cpal::SampleFormat::F32
                                && config.channels() == 2
                            {
                                let config = config.with_max_sample_rate();
                                info!("Using stereo F32 output config: {:?}", config);
                                return Ok((device, config));
                            }
                        }

                        // Then try any F32 format
                        for config in &configs {
                            if config.sample_format() == cpal::SampleFormat::F32 {
                                let config = config.with_max_sample_rate();
                                // info!("Using F32 output config: {:?}", config);
                                return Ok((device, config));
                            }
                        }

                        // Finally, use the first available config
                        let config = configs[0].with_max_sample_rate();
                        // info!("Using fallback output config: {:?}", config);
                        return Ok((device, config));
                    }
                } else {
                    warn!(
                        "Could not enumerate supported configurations for device: {}",
                        name
                    );
                }

                // If we couldn't get supported configs, try default
                if let Ok(default_config) = device.default_output_config() {
                    // info!("Using default output config: {:?}", default_config);
                    return Ok((device, default_config));
                }
            }
        }
    }

    Err(anyhow!(
        "Device not found or no compatible configuration available: {}",
        audio_device.name
    ))
}
