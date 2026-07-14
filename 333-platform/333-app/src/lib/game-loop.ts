// KG: TASK_333_B_GameLoop, CONTRACT_333_B_GameLoop
// KG: SPAN_333_B_GameLoop — requestAnimationFrame + fixed timestep
// Architecture: 20ms fixed physics + variable render + CRDT poll_sync integration
// Frame budget: target <16.67ms (60fps)

export interface GameLoopCallbacks {
  /** Fixed physics update (called every 20ms) */
  update: (dt: number) => void;
  /** Variable render (called every frame, alpha = interpolation factor) */
  render: (alpha: number) => void;
  /** Sync callback (called every 20ms, aligned with physics) */
  sync: () => void;
}

export interface FrameStats {
  fps: number;
  frameTime: number;   // ms
  physicsTime: number; // ms
  renderTime: number;  // ms
  syncTime: number;    // ms
  overBudget: boolean; // >16.67ms
}

const FIXED_DT = 20; // 20ms fixed physics timestep (50Hz)
const FRAME_BUDGET = 16.67; // 60fps target

/**
 * Game Loop — fixed timestep + variable render
 * KG: CONTRACT_333_B_GameLoop
 *
 * Pattern:
 *   accumulator += elapsed
 *   while (accumulator >= FIXED_DT):
 *     update(FIXED_DT)  // physics
 *     sync()            // CRDT poll_sync
 *     accumulator -= FIXED_DT
 *   alpha = accumulator / FIXED_DT
 *   render(alpha)       // interpolated render
 */
export class GameLoop {
  private running = false;
  private rafId = 0;
  private lastTime = 0;
  private accumulator = 0;
  private callbacks: GameLoopCallbacks;

  // Stats
  private frameCount = 0;
  private fpsTimer = 0;
  private currentFps = 0;
  private lastFrameTime = 0;
  private lastPhysicsTime = 0;
  private lastRenderTime = 0;
  private lastSyncTime = 0;

  constructor(callbacks: GameLoopCallbacks) {
    this.callbacks = callbacks;
  }

  start(): void {
    if (this.running) return;
    this.running = true;
    this.lastTime = performance.now();
    this.fpsTimer = this.lastTime;
    this.tick(this.lastTime);
  }

  stop(): void {
    this.running = false;
    if (this.rafId) {
      cancelAnimationFrame(this.rafId);
      this.rafId = 0;
    }
  }

  get stats(): FrameStats {
    return {
      fps: this.currentFps,
      frameTime: this.lastFrameTime,
      physicsTime: this.lastPhysicsTime,
      renderTime: this.lastRenderTime,
      syncTime: this.lastSyncTime,
      overBudget: this.lastFrameTime > FRAME_BUDGET,
    };
  }

  get isRunning(): boolean {
    return this.running;
  }

  private tick = (now: number): void => {
    if (!this.running) return;
    this.rafId = requestAnimationFrame(this.tick);

    const frameStart = performance.now();
    const elapsed = Math.min(now - this.lastTime, 100); // cap at 100ms (prevent spiral of death)
    this.lastTime = now;
    this.accumulator += elapsed;

    // Fixed timestep physics + sync
    // Taliban Fix 3: cap at 3 steps/frame to prevent budget explosion
    const MAX_STEPS_PER_FRAME = 3;
    let steps = 0;
    let physicsStart = performance.now();
    while (this.accumulator >= FIXED_DT && steps < MAX_STEPS_PER_FRAME) {
      this.callbacks.update(FIXED_DT / 1000); // pass dt in seconds
      this.accumulator -= FIXED_DT;
      steps++;
    }
    // If still over budget, discard remaining accumulator (drop physics ticks)
    if (this.accumulator > FIXED_DT * 2) {
      this.accumulator = 0; // prevent spiral of death
    }
    this.lastPhysicsTime = performance.now() - physicsStart;

    // KG: TASK_333_INT_CrdtSync — poll_sync once per frame (not per physics step)
    const syncStart = performance.now();
    this.callbacks.sync();
    this.lastSyncTime = performance.now() - syncStart;

    // Variable render with interpolation
    const alpha = this.accumulator / FIXED_DT;
    const renderStart = performance.now();
    this.callbacks.render(alpha);
    this.lastRenderTime = performance.now() - renderStart;

    // Frame budget tracking
    this.lastFrameTime = performance.now() - frameStart;

    // FPS counter (update every second)
    this.frameCount++;
    if (now - this.fpsTimer >= 1000) {
      this.currentFps = this.frameCount;
      this.frameCount = 0;
      this.fpsTimer = now;
    }
  };
}
