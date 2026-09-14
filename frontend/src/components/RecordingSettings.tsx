import React, { useState, useEffect } from 'react';
import { HelpHint } from '@/components/ui/help-hint';
import { Switch } from '@/components/ui/switch';
import { FolderOpen } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { DeviceSelection } from '@/components/DeviceSelection';
import type { RecordingMode, SelectedDevices } from '@/components/DeviceSelection';
import { useConfig } from '@/contexts/ConfigContext';
import Analytics from '@/lib/analytics';
import { toast } from 'sonner';
import { useTranslation } from 'react-i18next';

export interface RecordingPreferences {
  save_folder: string;
  auto_save: boolean;
  file_format: string;
  preferred_mic_device: string | null;
  preferred_system_device: string | null;
  recording_mode: RecordingMode;
}

interface RecordingSettingsProps {
  onSave?: (preferences: RecordingPreferences) => void;
}

export function RecordingSettings({ onSave }: RecordingSettingsProps) {
  const { t } = useTranslation('settings');
  const { setSelectedDevices } = useConfig();
  const [preferences, setPreferences] = useState<RecordingPreferences>({
    save_folder: '',
    auto_save: true,
    file_format: 'mp4',
    preferred_mic_device: null,
    preferred_system_device: null,
    recording_mode: 'microphone_and_system',
  });
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [showRecordingNotification, setShowRecordingNotification] = useState(true);

  // Load recording preferences on component mount
  useEffect(() => {
    const loadPreferences = async () => {
      try {
        const prefs = await invoke<RecordingPreferences>('get_recording_preferences');
        setPreferences(prefs);
      } catch (error) {
        console.error('Failed to load recording preferences:', error);
        // If loading fails, get default folder path
        try {
          const defaultPath = await invoke<string>('get_default_recordings_folder_path');
          setPreferences(prev => ({ ...prev, save_folder: defaultPath }));
        } catch (defaultError) {
          console.error('Failed to get default folder path:', defaultError);
        }
      } finally {
        setLoading(false);
      }
    };

    loadPreferences();
  }, []);

  // Load recording notification preference
  useEffect(() => {
    const loadNotificationPref = async () => {
      try {
        const { Store } = await import('@tauri-apps/plugin-store');
        const store = await Store.load('preferences.json');
        const show = await store.get<boolean>('show_recording_notification') ?? true;
        setShowRecordingNotification(show);
      } catch (error) {
        console.error('Failed to load notification preference:', error);
      }
    };
    loadNotificationPref();
  }, []);

  const handleAutoSaveToggle = async (enabled: boolean) => {
    const previousPreferences = preferences;
    const newPreferences = { ...preferences, auto_save: enabled };
    setPreferences(newPreferences);
    const saved = await savePreferences(newPreferences);

    if (!saved) {
      setPreferences(previousPreferences);
      return;
    }

    // Track auto-save setting change
    await Analytics.track('auto_save_recording_toggled', {
      enabled: enabled.toString()
    });
  };

  const handleDeviceChange = async (devices: SelectedDevices) => {
    const previousPreferences = preferences;
    const newPreferences = {
      ...preferences,
      preferred_mic_device: devices.micDevice,
      preferred_system_device: devices.systemDevice,
      recording_mode: devices.recordingMode,
    };
    setPreferences(newPreferences);
    const saved = await savePreferences(newPreferences);

    if (!saved) {
      setPreferences(previousPreferences);
      return;
    }

    // Track default device preference changes
    // Note: Individual device selection analytics are tracked in DeviceSelection component
    await Analytics.track('default_devices_changed', {
      has_preferred_microphone: (!!devices.micDevice).toString(),
      has_preferred_system_audio: (!!devices.systemDevice).toString()
    });
  };

  const handleOpenFolder = async () => {
    try {
      await invoke('open_recordings_folder');
    } catch (error) {
      console.error('Failed to open recordings folder:', error);
    }
  };

  const handleNotificationToggle = async (enabled: boolean) => {
    try {
      setShowRecordingNotification(enabled);
      const { Store } = await import('@tauri-apps/plugin-store');
      const store = await Store.load('preferences.json');
      await store.set('show_recording_notification', enabled);
      await store.save();
      toast.success(t('messages.preferenceSaved'));
      await Analytics.track('recording_notification_preference_changed', {
        enabled: enabled.toString()
      });
    } catch (error) {
      console.error('Failed to save notification preference:', error);
      toast.error(t('errors.savePreference'));
    }
  };

  const savePreferences = async (prefs: RecordingPreferences): Promise<boolean> => {
    setSaving(true);
    try {
      await invoke('set_recording_preferences', { preferences: prefs });
      setSelectedDevices({
        micDevice: prefs.preferred_mic_device,
        systemDevice: prefs.preferred_system_device,
        recordingMode: prefs.recording_mode,
      });
      onSave?.(prefs);

      // Show success toast with device details
      const micDevice = prefs.preferred_mic_device || t('common.default');
      const systemDevice = prefs.preferred_system_device || t('common.default');
      toast.success(t('devices.saved'), {
        description: t('devices.selectionDescription', { micDevice, systemDevice })
      });
      return true;
    } catch (error) {
      console.error('Failed to save recording preferences:', error);
      toast.error(t('errors.saveDevices'));
      return false;
    } finally {
      setSaving(false);
    }
  };

  const handleChooseFolder = async () => {
    try {
      const selectedFolder = await invoke<string | null>('select_recording_folder');
      if (!selectedFolder) return;

      const previousPreferences = preferences;
      const nextPreferences = { ...preferences, save_folder: selectedFolder };
      setPreferences(nextPreferences);
      if (!(await savePreferences(nextPreferences))) {
        setPreferences(previousPreferences);
      }
    } catch (error) {
      console.error('Failed to select recording folder:', error);
      toast.error(t('errors.savePreference'));
    }
  };

  if (loading) {
    return (
      <div className="animate-pulse">
        <div className="h-4 bg-gray-200 rounded w-1/4 mb-4"></div>
        <div className="h-8 bg-gray-200 rounded mb-4"></div>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <div>
        <h3 className="flex items-center gap-1 text-base font-semibold">{t('recording.title')}<HelpHint>{t('recording.description')}</HelpHint></h3>
      </div>

      {/* Auto Save Toggle */}
      <div className="flex items-center justify-between p-4 border rounded-lg">
        <div className="flex-1">
          <div className="flex items-center gap-1 font-medium">{t('recording.saveAudio')}<HelpHint>{t('recording.saveAudioDescription')}</HelpHint></div>
        </div>
        <Switch
          checked={preferences.auto_save}
          onCheckedChange={handleAutoSaveToggle}
          disabled={saving}
          aria-label={t('recording.saveAudio')}
        />
      </div>

      {/* Folder Location - Only shown when auto_save is enabled */}
      {preferences.auto_save && (
        <div className="space-y-4">
          <div className="p-4 border rounded-lg bg-gray-50">
            <div className="font-medium mb-2">{t('recording.saveLocation')}</div>
            <div className="text-sm text-gray-600 mb-3 break-all">
              {preferences.save_folder || t('recording.defaultFolder')}
            </div>
            <div className="flex flex-wrap gap-2">
              <button
                onClick={handleChooseFolder}
                disabled={saving}
                className="flex items-center gap-2 px-3 py-2 text-sm border border-gray-300 rounded-md hover:bg-gray-50 transition-colors disabled:opacity-50"
              >
                <FolderOpen className="w-4 h-4" />
                {t('recording.chooseFolder')}
              </button>
              <button
                onClick={handleOpenFolder}
                className="flex items-center gap-2 px-3 py-2 text-sm border border-gray-300 rounded-md hover:bg-gray-50 transition-colors"
              >
                <FolderOpen className="w-4 h-4" />
                {t('recording.openFolder')}
              </button>
            </div>
          </div>

          <div className="flex flex-wrap items-center gap-1 text-xs text-gray-500">
            <div className="text-xs text-gray-500">
              <strong>{t('recording.fileFormat')}</strong> {t('recording.fileType', { format: preferences.file_format.toUpperCase() })}
            </div>
            <HelpHint>
              {t('recording.timestampDescription', { format: preferences.file_format })}
            </HelpHint>
          </div>
        </div>
      )}

      {/* Info when auto_save is disabled */}
      {!preferences.auto_save && (
        <div className="text-xs text-gray-600">
          <div className="text-xs text-gray-600">
            {t('recording.disabledDescription')}
          </div>
        </div>
      )}

      {/* Recording Notification Toggle */}
      <div className="flex items-center justify-between p-4 border rounded-lg">
        <div className="flex-1">
          <div className="flex items-center gap-1 font-medium">{t('recording.startNotification')}<HelpHint>{t('recording.startNotificationDescription')}</HelpHint></div>
        </div>
        <Switch
          checked={showRecordingNotification}
          onCheckedChange={handleNotificationToggle}
          aria-label={t('recording.startNotification')}
        />
      </div>

      {/* Device Preferences */}
      <div className="space-y-4">
        <div className="border-t pt-6">
          <h4 className="flex items-center gap-1 text-base font-medium text-gray-900 mb-4">{t('recording.defaultDevices')}<HelpHint>{t('recording.defaultDevicesDescription')}</HelpHint></h4>

          <div className="border rounded-lg p-4 bg-gray-50">
            <DeviceSelection
              selectedDevices={{
                micDevice: preferences.preferred_mic_device,
                systemDevice: preferences.preferred_system_device,
                recordingMode: preferences.recording_mode,
              }}
              onDeviceChange={handleDeviceChange}
              disabled={saving}
            />
          </div>
        </div>
      </div>
    </div>
  );
}
