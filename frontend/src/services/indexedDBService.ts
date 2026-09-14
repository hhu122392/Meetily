/**
 * IndexedDB Service for Transcript Recovery
 * Provides browser-based persistence for meeting transcripts and metadata
 * to enable recovery after app crashes or unexpected closures.
 */

import { isNewerTranscript } from '../lib/live-transcript';

// Database schema interfaces
export interface MeetingMetadata {
  meetingId: string;          // Primary key: "meeting-{timestamp}"
  title: string;              // Meeting title
  startTime: number;          // Unix timestamp (ms)
  lastUpdated: number;        // Unix timestamp (ms)
  transcriptCount: number;    // Number of transcript segments
  savedToSQLite: boolean;     // Flag: saved to backend DB
  folderPath?: string;        // Path to recording folder
}

export interface StoredTranscript {
  id?: number;                // Auto-increment primary key
  meetingId: string;          // Foreign key to meetings store
  text: string;               // Transcript text
  timestamp: string;          // ISO 8601 timestamp
  confidence: number;         // Whisper confidence score
  sequenceId: number;         // Sequence number for ordering
  storedAt: number;           // Unix timestamp when saved
  audio_start_time?: number;  // Recording-relative start time in seconds
  audio_end_time?: number;    // Recording-relative end time in seconds
  duration?: number;          // Duration in seconds
  revision?: number;
  is_partial?: boolean;
  [key: string]: any;         // Allow additional fields from TranscriptUpdate
}

function storedKey(row: StoredTranscript): string {
  const sequence = row.sequenceId ?? row.sequence_id;
  return Number.isSafeInteger(sequence) ? `seq:${sequence}` : `legacy:${row.id}`;
}

function latestStoredRows(rows: StoredTranscript[]): Map<string, StoredTranscript> {
  const latest = new Map<string, StoredTranscript>();
  for (const source of rows) {
    const row = { ...source, sequenceId: source.sequenceId ?? source.sequence_id };
    const old = latest.get(storedKey(row));
    if (!old || isNewerTranscript(old, row)
      || ((row.revision ?? 0) === 0 && (old.revision ?? 0) === 0 && row.storedAt > old.storedAt)) {
      latest.set(storedKey(row), row);
    }
  }
  return latest;
}

export class IndexedDBService {
  private db: IDBDatabase | null = null;
  constructor(private readonly DB_NAME = 'MeetilyRecoveryDB') {}
  private readonly DB_VERSION = 1;
  private initPromise: Promise<void> | null = null;
  private pendingTranscriptWrites = new Set<Promise<void>>();

  /**
   * Initialize database connection
   */
  async init(): Promise<void> {
    // Return existing promise if initialization is in progress
    if (this.initPromise) {
      return this.initPromise;
    }

    // Return immediately if already initialized
    if (this.db) {
      return Promise.resolve();
    }

    this.initPromise = new Promise((resolve, reject) => {
      try {
        const request = indexedDB.open(this.DB_NAME, this.DB_VERSION);

        request.onerror = () => {
          console.error('Failed to open IndexedDB:', request.error);
          reject(request.error);
        };

        request.onsuccess = () => {
          this.db = request.result;
          resolve();
        };

        request.onupgradeneeded = (event) => {
          const db = (event.target as IDBOpenDBRequest).result;

          // Create meetings store
          if (!db.objectStoreNames.contains('meetings')) {
            const meetingsStore = db.createObjectStore('meetings', { keyPath: 'meetingId' });
            meetingsStore.createIndex('lastUpdated', 'lastUpdated', { unique: false });
            meetingsStore.createIndex('savedToSQLite', 'savedToSQLite', { unique: false });
          }

          // Create transcripts store
          if (!db.objectStoreNames.contains('transcripts')) {
            const transcriptsStore = db.createObjectStore('transcripts', {
              keyPath: 'id',
              autoIncrement: true
            });
            transcriptsStore.createIndex('meetingId', 'meetingId', { unique: false });
            transcriptsStore.createIndex('storedAt', 'storedAt', { unique: false });
          }
        };
      } catch (error) {
        console.error('Exception during IndexedDB initialization:', error);
        reject(error);
      }
    });

    return this.initPromise;
  }

  // Meeting operations

  /**
   * Save or update meeting metadata
   */
  async saveMeetingMetadata(metadata: MeetingMetadata): Promise<void> {
    try {
      if (!this.db) await this.init();

      const transaction = this.db!.transaction(['meetings'], 'readwrite');
      const store = transaction.objectStore('meetings');

      await new Promise<void>((resolve, reject) => {
        const request = store.put(metadata);
        request.onsuccess = () => resolve();
        request.onerror = () => reject(request.error);
      });
    } catch (error) {
      console.warn('Failed to save meeting metadata to IndexedDB:', error);
      // Fail silently - don't interrupt recording
    }
  }

  /**
   * Get meeting metadata by ID
   */
  async getMeetingMetadata(meetingId: string): Promise<MeetingMetadata | null> {
    try {
      if (!this.db) await this.init();

      const transaction = this.db!.transaction(['meetings'], 'readonly');
      const store = transaction.objectStore('meetings');

      return new Promise((resolve, reject) => {
        const request = store.get(meetingId);
        request.onsuccess = () => resolve(request.result || null);
        request.onerror = () => reject(request.error);
      });
    } catch (error) {
      console.error('Failed to get meeting metadata from IndexedDB:', error);
      return null;
    }
  }

  /**
   * Get all unsaved meetings (savedToSQLite = false)
   */
  async getAllMeetings(): Promise<MeetingMetadata[]> {
    try {
      if (!this.db) await this.init();

      const transaction = this.db!.transaction(['meetings'], 'readonly');
      const store = transaction.objectStore('meetings');

      return new Promise((resolve, reject) => {
        const request = store.getAll();
        request.onsuccess = () => {
          const allMeetings = request.result as MeetingMetadata[];
          // Filter for unsaved meetings (savedToSQLite = false)
          const unsavedMeetings = allMeetings.filter(m => m.savedToSQLite === false);

          // Sort by most recent first
          unsavedMeetings.sort((a, b) => b.lastUpdated - a.lastUpdated);
          resolve(unsavedMeetings);
        };
        request.onerror = () => reject(request.error);
      });
    } catch (error) {
      console.error('Failed to get meetings from IndexedDB:', error);
      return [];
    }
  }

  /**
   * Mark meeting as saved to SQLite
   */
  async markMeetingSaved(meetingId: string): Promise<void> {
    try {
      if (!this.db) await this.init();

      const transaction = this.db!.transaction(['meetings'], 'readwrite');
      const store = transaction.objectStore('meetings');

      return new Promise((resolve, reject) => {
        const getRequest = store.get(meetingId);
        getRequest.onsuccess = () => {
          const meeting = getRequest.result;
          if (meeting) {
            meeting.savedToSQLite = true;
            meeting.lastUpdated = Date.now();
            const putRequest = store.put(meeting);
            putRequest.onsuccess = () => resolve();
            putRequest.onerror = () => reject(putRequest.error);
          } else {
            resolve();
          }
        };
        getRequest.onerror = () => reject(getRequest.error);
      });
    } catch (error) {
      console.warn('Failed to mark meeting as saved:', error);
    }
  }

  /**
   * Delete meeting and all its transcripts
   */
  async deleteMeeting(meetingId: string): Promise<void> {
    try {
      if (!this.db) await this.init();

      const transaction = this.db!.transaction(['meetings', 'transcripts'], 'readwrite');
      const meetingsStore = transaction.objectStore('meetings');
      const transcriptsStore = transaction.objectStore('transcripts');

      // Delete transcripts
      await this.deleteTranscriptsForMeetingInternal(transcriptsStore, meetingId);

      // Delete meeting
      await new Promise<void>((resolve, reject) => {
        const request = meetingsStore.delete(meetingId);
        request.onsuccess = () => resolve();
        request.onerror = () => reject(request.error);
      });
    } catch (error) {
      console.error('Failed to delete meeting from IndexedDB:', error);
      throw error;
    }
  }

  // Transcript operations

  /**
   * Save a transcript segment
   */
  saveTranscript(meetingId: string, transcript: any): Promise<void> {
    return this.saveTranscripts(meetingId, [transcript]);
  }

  saveTranscripts(meetingId: string, transcripts: any[]): Promise<void> {
    const write = this.persistTranscripts(meetingId, transcripts);
    this.pendingTranscriptWrites.add(write);
    void write.then(() => this.pendingTranscriptWrites.delete(write), () => this.pendingTranscriptWrites.delete(write));
    return write;
  }

  async flushTranscriptWrites(): Promise<void> {
    while (this.pendingTranscriptWrites.size) await Promise.all([...this.pendingTranscriptWrites]);
  }

  private async persistTranscripts(meetingId: string, transcripts: any[]): Promise<void> {
    if (transcripts.some(row => !Number.isSafeInteger(row.sequence_id ?? row.sequenceId)
      || (row.sequence_id ?? row.sequenceId) < 0)) throw new Error('Invalid transcript sequence');
    if (!transcripts.length) return;
    if (!this.db) await this.init();
    await new Promise<void>((resolve, reject) => {
      const transaction = this.db!.transaction(['transcripts', 'meetings'], 'readwrite');
      const store = transaction.objectStore('transcripts');
      const meetings = transaction.objectStore('meetings');
      let failure: unknown;
      transaction.oncomplete = () => resolve();
      transaction.onabort = () => reject(failure ?? transaction.error ?? new Error('Transcript transaction aborted'));
      transaction.onerror = () => { failure ??= transaction.error; };
      // Read and replace within one transaction. This also supports v1 caches
      // whose records used sequence_id, without a destructive schema migration.
      const request = store.index('meetingId').getAll(meetingId);
      request.onsuccess = () => {
        try {
          const stored = request.result as StoredTranscript[];
          const latest = latestStoredRows(stored);
          for (const source of transcripts) {
            const incoming: StoredTranscript = {
              ...source, meetingId, sequenceId: source.sequence_id ?? source.sequenceId, storedAt: Date.now(),
            };
            delete incoming.id;
            const key = storedKey(incoming);
            const old = latest.get(key);
            if (!old || isNewerTranscript(old, incoming)) {
              latest.set(key, { ...incoming, ...(old?.id !== undefined ? { id: old.id } : {}) });
            }
          }
          const retainedIds = new Set([...latest.values()].map(row => row.id));
          for (const old of stored) if (old.id !== undefined && !retainedIds.has(old.id)) store.delete(old.id);
          for (const row of latest.values()) store.put(row);
          const meetingRequest = meetings.get(meetingId);
          meetingRequest.onsuccess = () => {
            const meeting = meetingRequest.result as MeetingMetadata | undefined;
            if (meeting) meetings.put({ ...meeting, lastUpdated: Date.now(), transcriptCount: latest.size });
          };
        } catch (error) {
          failure = error;
          transaction.abort();
        }
      };
    });
  }

  /**
   * Get all transcripts for a meeting
   */
  async getTranscripts(meetingId: string): Promise<StoredTranscript[]> {
    try {
      if (!this.db) await this.init();

      const transaction = this.db!.transaction(['transcripts'], 'readonly');
      const store = transaction.objectStore('transcripts');
      const index = store.index('meetingId');

      return new Promise((resolve, reject) => {
        const request = index.getAll(meetingId);
        request.onsuccess = () => {
          const transcripts = [...latestStoredRows(request.result as StoredTranscript[]).values()];
          // Sort by sequence ID
          transcripts.sort((a, b) => (a.sequenceId ?? 0) - (b.sequenceId ?? 0));
          resolve(transcripts);
        };
        request.onerror = () => reject(request.error);
      });
    } catch (error) {
      console.error('Failed to get transcripts from IndexedDB:', error);
      return [];
    }
  }

  /**
   * Get transcript count for a meeting
   */
  async getTranscriptCount(meetingId: string): Promise<number> {
    return (await this.getTranscripts(meetingId)).length;
  }

  // Cleanup operations

  /**
   * Delete meetings older than specified days
   * @param daysOld Number of days threshold
   * @returns Number of meetings deleted
   */
  async deleteOldMeetings(daysOld: number): Promise<number> {
    try {
      if (!this.db) await this.init();

      const cutoffTime = Date.now() - (daysOld * 24 * 60 * 60 * 1000);
      const transaction = this.db!.transaction(['meetings', 'transcripts'], 'readwrite');
      const meetingsStore = transaction.objectStore('meetings');
      const transcriptsStore = transaction.objectStore('transcripts');

      // Get all meetings
      const allMeetings = await new Promise<MeetingMetadata[]>((resolve, reject) => {
        const request = meetingsStore.getAll();
        request.onsuccess = () => resolve(request.result);
        request.onerror = () => reject(request.error);
      });

      let deletedCount = 0;

      for (const meeting of allMeetings) {
        if (meeting.lastUpdated < cutoffTime) {
          // Delete transcripts
          await this.deleteTranscriptsForMeetingInternal(transcriptsStore, meeting.meetingId);

          // Delete meeting
          await new Promise<void>((resolve, reject) => {
            const request = meetingsStore.delete(meeting.meetingId);
            request.onsuccess = () => resolve();
            request.onerror = () => reject(request.error);
          });

          deletedCount++;
        }
      }

      console.log(`Cleaned up ${deletedCount} old meetings`);
      return deletedCount;
    } catch (error) {
      console.error('Failed to delete old meetings:', error);
      return 0;
    }
  }

  /**
   * Delete saved meetings older than specified hours
   * @param hoursOld Number of hours threshold after save
   * @returns Number of meetings deleted
   */
  async deleteSavedMeetings(hoursOld: number): Promise<number> {
    try {
      if (!this.db) await this.init();

      const cutoffTime = Date.now() - (hoursOld * 60 * 60 * 1000);
      const transaction = this.db!.transaction(['meetings', 'transcripts'], 'readwrite');
      const meetingsStore = transaction.objectStore('meetings');
      const transcriptsStore = transaction.objectStore('transcripts');

      // Get all meetings and filter for saved ones
      const allMeetings = await new Promise<MeetingMetadata[]>((resolve, reject) => {
        const request = meetingsStore.getAll();
        request.onsuccess = () => resolve(request.result);
        request.onerror = () => reject(request.error);
      });

      // Filter for saved meetings (savedToSQLite = true)
      const savedMeetings = allMeetings.filter(m => m.savedToSQLite === true);

      let deletedCount = 0;

      for (const meeting of savedMeetings) {
        if (meeting.lastUpdated < cutoffTime) {
          // Delete transcripts
          await this.deleteTranscriptsForMeetingInternal(transcriptsStore, meeting.meetingId);

          // Delete meeting
          await new Promise<void>((resolve, reject) => {
            const request = meetingsStore.delete(meeting.meetingId);
            request.onsuccess = () => resolve();
            request.onerror = () => reject(request.error);
          });

          deletedCount++;
        }
      }

      console.log(`Cleaned up ${deletedCount} saved meetings`);
      return deletedCount;
    } catch (error) {
      console.error('Failed to delete saved meetings:', error);
      return 0;
    }
  }

  /**
   * Helper to delete all transcripts for a meeting
   */
  private async deleteTranscriptsForMeetingInternal(
    transcriptsStore: IDBObjectStore,
    meetingId: string
  ): Promise<void> {
    const index = transcriptsStore.index('meetingId');

    return new Promise((resolve, reject) => {
      const request = index.openCursor(IDBKeyRange.only(meetingId));

      request.onsuccess = (event) => {
        const cursor = (event.target as IDBRequest<IDBCursorWithValue>).result;
        if (cursor) {
          cursor.delete();
          cursor.continue();
        } else {
          resolve();
        }
      };

      request.onerror = () => reject(request.error);
    });
  }
}

// Export singleton instance
export const indexedDBService = new IndexedDBService();
