// KG: TASK_333_C_Audio, SPAN_333_C_Audio
// Web Audio API: 3D positional audio + sound effects
// PannerNode for spatial sound, GainNode for volume

export type SoundId = string;

/**
 * 333 Audio Engine — Web Audio API with 3D positioning
 * KG: CONTRACT_333_C_Audio
 */
export class AudioEngine {
  private ctx: AudioContext | null = null;
  private masterGain: GainNode | null = null;
  private buffers = new Map<SoundId, AudioBuffer>();
  private listener: AudioListener | null = null;

  // Taliban Fix 4: lazy init — only create AudioContext on user gesture
  /** Initialize audio (MUST be called from user gesture: click, keydown) */
  async init(): Promise<boolean> {
    try {
      // AudioContext created inside user gesture → auto-resumes
      this.ctx = new AudioContext();
      this.masterGain = this.ctx.createGain();
      this.masterGain.connect(this.ctx.destination);
      this.masterGain.gain.value = 0.7;
      this.listener = this.ctx.listener;
      // Resume immediately (user gesture context)
      if (this.ctx.state === 'suspended') await this.ctx.resume();
      return true;
    } catch {
      return false;
    }
  }

  /** Resume if suspended (call on any user interaction) */
  async resume(): Promise<void> {
    if (this.ctx?.state === 'suspended') await this.ctx.resume();
  }

  /** Load a sound from URL */
  async loadSound(id: SoundId, url: string): Promise<void> {
    if (!this.ctx) return;
    const response = await fetch(url);
    const arrayBuffer = await response.arrayBuffer();
    const buffer = await this.ctx.decodeAudioData(arrayBuffer);
    this.buffers.set(id, buffer);
  }

  /** Play a sound at a 3D position */
  play3D(id: SoundId, x: number, y: number, z: number = 0): void {
    const buffer = this.buffers.get(id);
    if (!buffer || !this.ctx || !this.masterGain) return;

    const source = this.ctx.createBufferSource();
    source.buffer = buffer;

    // 3D positioning via PannerNode
    const panner = this.ctx.createPanner();
    panner.panningModel = 'HRTF';
    panner.distanceModel = 'inverse';
    panner.refDistance = 1;
    panner.maxDistance = 50;
    panner.positionX.value = x;
    panner.positionY.value = y;
    panner.positionZ.value = z;

    source.connect(panner);
    panner.connect(this.masterGain);
    // Taliban Fix 5: auto-disconnect on playback end (prevent GC pressure)
    source.onended = () => { source.disconnect(); panner.disconnect(); };
    source.start();
  }

  /** Play a 2D sound (no positioning) */
  play(id: SoundId, volume = 1.0): void {
    const buffer = this.buffers.get(id);
    if (!buffer || !this.ctx || !this.masterGain) return;

    const source = this.ctx.createBufferSource();
    source.buffer = buffer;

    const gain = this.ctx.createGain();
    gain.gain.value = volume;

    source.connect(gain);
    gain.connect(this.masterGain);
    source.onended = () => { source.disconnect(); gain.disconnect(); };
    source.start();
  }

  /** Update listener position (call every frame) */
  setListenerPosition(x: number, y: number, z: number = 0): void {
    if (!this.listener) return;
    this.listener.positionX.value = x;
    this.listener.positionY.value = y;
    this.listener.positionZ.value = z;
  }

  /** Set master volume (0-1) */
  setVolume(v: number): void {
    if (this.masterGain) this.masterGain.gain.value = Math.max(0, Math.min(1, v));
  }

  destroy(): void {
    this.ctx?.close();
    this.ctx = null;
    this.buffers.clear();
  }
}
