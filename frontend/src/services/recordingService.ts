/**
 * Recording Service
 *
 * Handles all recording lifecycle Tauri backend calls and events.
 * Pure 1-to-1 wrapper - no error handling changes, exact same behavior as direct invoke/listen calls.
 */

import { invoke } from '@tauri-apps/api/core';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import { RecordingStartGate } from '@/lib/recording-start-gate';
import type { PreparedRecordingMetadata } from '@/types/summary-template';
import type { RecordingMode } from '@/components/DeviceSelection';

export interface RecordingState {
  is_recording: boolean;
  is_paused: boolean;
  is_active: boolean;
  is_reconnecting: boolean;
  recording_duration: number | null;
  active_duration: number | null;
  recording_mode: RecordingMode | 'inactive';
  device_epoch: number;
  system_stream_started_qpc_ns: number | null;
  last_error: string | null;
  cutover: {
    watermark_qpc_ns: number | null;
    callback_drain_deadline_qpc_ns: number | null;
    in_flight_callback_count: number;
    old_epoch_last_capture_qpc_ns: number | null;
    new_epoch_first_capture_qpc_ns: number | null;
    late_callback_dropped_frames: number;
    attribution_error_frames: number;
  };
  microphone_route: RecordingRouteState;
  system_route: RecordingRouteState;
}

export interface RecordingRouteState {
  active: boolean;
  device_name: string | null;
  native_id: string | null;
  stream_started_count?: number;
  callback_count: number;
  sample_count: number;
  rms_level?: number;
  peak_level?: number;
  frame_count?: number;
  silent_flag_frame_count?: number;
  all_zero_frame_count?: number;
  last_capture_qpc_ns?: number | null;
  last_callback_observed_qpc_ns?: number | null;
  max_callback_gap_ns?: number;
  no_signal?: boolean;
  no_signal_count?: number;
  failed?: boolean;
  endpoint_muted?: boolean;
  silent?: boolean;
  driver_mute_behavior?: 'audible_frames' | 'silent_frames' | 'no_callback' | null;
  format?: {
    sample_rate: number;
    channels: number;
    bits_per_sample: number;
    block_align: number;
    sample_format: string;
  } | null;
}

export interface RecordingStoppedPayload {
  message: string;
  folder_path?: string;
  meeting_name?: string;
}

/**
 * Emitted when a saved recording device is unavailable and the session fell
 * back to the system default device instead of refusing to start.
 */
export interface RecordingDeviceFallbackPayload {
  route: 'microphone' | 'system';
  requested: string;
  used: string;
  reason: string;
}

export type DeviceEventResponse =
  | { type: 'DeviceDisconnected'; device_name: string; native_id: string | null; device_type: string }
  | { type: 'DeviceReconnected'; device_name: string; native_id: string | null; device_type: string }
  | { type: 'DeviceListChanged' };

/**
 * Recording Service
 * Singleton service for managing recording lifecycle operations
 */
export class RecordingService {
  private readonly startGate = new RecordingStartGate();

  /**
   * Check if recording is currently active
   * @returns Promise<boolean>
   */
  async isRecording(): Promise<boolean> {
    return invoke<boolean>('is_recording');
  }

  /**
   * Get comprehensive recording state (includes durations)
   * @returns Promise with full recording state
   */
  async getRecordingState(): Promise<RecordingState> {
    return invoke<RecordingState>('get_recording_state');
  }

  /** Drain one device-monitor event and execute its required route transition. */
  async pollAudioDeviceEvents(): Promise<DeviceEventResponse | null> {
    return invoke<DeviceEventResponse | null>('poll_audio_device_events');
  }

  /**
   * Get current meeting name
   * @returns Promise<string | null>
   */
  async getRecordingMeetingName(): Promise<string | null> {
    return invoke<string | null>('get_recording_meeting_name');
  }

  /**
   * Start recording (no device configuration)
   * @returns Promise<void>
   */
  async startRecording(): Promise<void> {
    return this.startGate.run(
      () => this.isRecording(),
      () => invoke<void>('start_recording'),
    );
  }

  /**
   * Start recording with device configuration and meeting name
   * @param micDeviceName - Microphone device name (null for default)
   * @param systemDeviceName - System audio device name (null for none)
   * @param meetingName - Meeting name/title
   * @returns Promise<void>
   */
  async startRecordingWithDevices(
    micDeviceName: string | null,
    systemDeviceName: string | null,
    meetingName: string,
    recordingMode: RecordingMode,
    metadata: PreparedRecordingMetadata = {
      templateSelection: null,
      meetingContextDraft: null,
    },
  ): Promise<void> {
    return this.startGate.run(
      () => this.isRecording(),
      () => invoke<void>('start_recording_with_devices_and_meeting', {
        micDeviceName: micDeviceName,
        systemDeviceName: systemDeviceName,
        meetingName: meetingName,
        recordingMode,
        templateSelection: metadata.templateSelection,
        meetingContextDraft: metadata.meetingContextDraft,
      }),
    );
  }

  /** Stop the active recording. Its save root was frozen at start. */
  async stopRecording(): Promise<void> {
    return invoke('stop_recording');
  }

  /**
   * Pause active recording
   * @returns Promise<void>
   */
  async pauseRecording(): Promise<void> {
    return invoke('pause_recording');
  }

  /**
   * Resume paused recording
   * @returns Promise<void>
   */
  async resumeRecording(): Promise<void> {
    return invoke('resume_recording');
  }

  // Event Listeners

  /**
   * Listen for recording-started event
   * @param callback - Function to call when recording starts
   * @returns Promise that resolves to unlisten function
   */
  async onRecordingStarted(callback: () => void): Promise<UnlistenFn> {
    return listen('recording-started', callback);
  }

  /**
   * Listen for recording-device-fallback event
   * A saved device was unavailable; the session used the system default instead.
   */
  async onRecordingDeviceFallback(
    callback: (payload: RecordingDeviceFallbackPayload) => void
  ): Promise<UnlistenFn> {
    return listen<RecordingDeviceFallbackPayload>('recording-device-fallback', (event) => {
      callback(event.payload);
    });
  }

  /**
   * Listen for recording-stopped event (with metadata)
   * @param callback - Function to call when recording stops
   * @returns Promise that resolves to unlisten function
   */
  async onRecordingStopped(callback: (payload: RecordingStoppedPayload) => void): Promise<UnlistenFn> {
    return listen<RecordingStoppedPayload>('recording-stopped', (event) => {
      callback(event.payload);
    });
  }

  /**
   * Listen for recording-paused event
   * @param callback - Function to call when recording is paused
   * @returns Promise that resolves to unlisten function
   */
  async onRecordingPaused(callback: () => void): Promise<UnlistenFn> {
    return listen('recording-paused', callback);
  }

  /**
   * Listen for recording-resumed event
   * @param callback - Function to call when recording resumes
   * @returns Promise that resolves to unlisten function
   */
  async onRecordingResumed(callback: () => void): Promise<UnlistenFn> {
    return listen('recording-resumed', callback);
  }

  /**
   * Listen for chunk-drop-warning event (audio buffer overflow)
   * @param callback - Function to call when chunks are dropped
   * @returns Promise that resolves to unlisten function
   */
  async onChunkDropWarning(callback: (warning: string) => void): Promise<UnlistenFn> {
    return listen<string>('chunk-drop-warning', (event) => {
      callback(event.payload);
    });
  }

  /**
   * Listen for speech-detected event (VAD)
   * @param callback - Function to call when speech is detected
   * @returns Promise that resolves to unlisten function
   */
  async onSpeechDetected(callback: () => void): Promise<UnlistenFn> {
    return listen('speech-detected', callback);
  }
}

// Export singleton instance
export const recordingService = new RecordingService();
