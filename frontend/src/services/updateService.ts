/** Manual releases for this independently distributed community build. */
import type { Update } from '@tauri-apps/plugin-updater';
import { getVersion } from '@tauri-apps/api/app';
import { invoke } from '@tauri-apps/api/core';

export interface UpdateInfo {
  available: boolean;
  currentVersion: string;
  manual?: boolean;
  version?: string;
  date?: string;
  body?: string;
  downloadUrl?: string;
}
export interface UpdateProgress { downloaded: number; total: number; percentage: number; }

export class UpdateService {
  async checkForUpdates(force = false): Promise<UpdateInfo> {
    const currentVersion = await getVersion();
    if (force) {
      await invoke('open_external_url', { url: 'https://github.com/hhu122392/Meetily/releases' });
    }
    return { available: false, currentVersion, manual: true };
  }
  async downloadAndInstall(_update: Update, _onProgress?: (progress: UpdateProgress) => void): Promise<void> {
    throw new Error('MANUAL_UPDATE_REQUIRED');
  }
  async getCurrentVersion(): Promise<string> { return getVersion(); }
  wasCheckedRecently(): boolean { return false; }
}
export const updateService = new UpdateService();
