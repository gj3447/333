// KG: TASK_333_C_OPFS, SPAN_333_C_OPFS
// OPFS (Origin Private File System) for large CRDT snapshots
// 2-4x faster than IndexedDB for large binary data
// Worker-only sync access for maximum performance

const OPFS_DIR = '333-data';

/**
 * OPFS Storage — large binary persistence
 * KG: CONTRACT_333_C_OPFS — CRDT snapshots, world chunks
 */
export class OpfsStorage {
  private root: FileSystemDirectoryHandle | null = null;

  async init(): Promise<boolean> {
    try {
      const root = await navigator.storage.getDirectory();
      this.root = await root.getDirectoryHandle(OPFS_DIR, { create: true });
      // Request persistent storage
      if (navigator.storage.persist) {
        await navigator.storage.persist();
      }
      return true;
    } catch {
      return false; // OPFS not supported
    }
  }

  /**
   * Save CRDT snapshot (binary, large)
   */
  async saveSnapshot(name: string, data: Uint8Array): Promise<void> {
    if (!this.root) return;
    const file = await this.root.getFileHandle(name, { create: true });
    const writable = await file.createWritable();
    // KG: lesson-333-ts57-uint8-bufferlike-2026-04-19
    await writable.write(data as BufferSource);
    await writable.close();
  }

  /**
   * Load CRDT snapshot
   */
  async loadSnapshot(name: string): Promise<Uint8Array | null> {
    if (!this.root) return null;
    try {
      const file = await this.root.getFileHandle(name);
      const blob = await file.getFile();
      const buf = await blob.arrayBuffer();
      return new Uint8Array(buf);
    } catch {
      return null; // file doesn't exist
    }
  }

  /**
   * Delete a snapshot
   */
  async deleteSnapshot(name: string): Promise<void> {
    if (!this.root) return;
    try {
      await this.root.removeEntry(name);
    } catch { /* doesn't exist */ }
  }

  /**
   * List all snapshots
   */
  async listSnapshots(): Promise<string[]> {
    if (!this.root) return [];
    const names: string[] = [];
    // Taliban Fix 2: proper type iteration
    const iter = (this.root as FileSystemDirectoryHandle & AsyncIterable<[string, FileSystemHandle]>)[Symbol.asyncIterator]();
    for await (const [name, handle] of { [Symbol.asyncIterator]: () => iter }) {
      if (handle.kind === 'file') names.push(name);
    }
    return names;
  }

  /**
   * Get storage usage estimate
   */
  async getUsage(): Promise<{ used: number; quota: number }> {
    const est = await navigator.storage.estimate();
    return { used: est.usage || 0, quota: est.quota || 0 };
  }
}
