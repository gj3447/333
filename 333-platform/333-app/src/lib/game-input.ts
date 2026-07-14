// KG: TASK_333_C_GameInput, SPAN_333_C_GameInput
// Gamepad API + Pointer Lock + Fullscreen for game controls
// Progressive: works with keyboard/mouse, enhanced with gamepad

export interface InputState {
  // Keyboard/mouse
  keys: Set<string>;
  mouseX: number;
  mouseY: number;
  mouseDx: number;
  mouseDy: number;
  mouseDown: boolean;
  // Gamepad
  gamepadConnected: boolean;
  leftStick: { x: number; y: number };
  rightStick: { x: number; y: number };
  buttons: boolean[];
}

/**
 * Game Input Manager — keyboard, mouse, gamepad, pointer lock
 * KG: CONTRACT_333_C_GameInput
 */
export class GameInput {
  private state: InputState = {
    keys: new Set(), mouseX: 0, mouseY: 0, mouseDx: 0, mouseDy: 0, mouseDown: false,
    gamepadConnected: false, leftStick: { x: 0, y: 0 }, rightStick: { x: 0, y: 0 }, buttons: [],
  };
  private canvas: HTMLElement | null = null;
  private pointerLocked = false;

  // Taliban Fix 3: store handlers for cleanup
  private handlers: Array<[EventTarget, string, EventListener]> = [];

  private on(target: EventTarget, event: string, handler: EventListener): void {
    target.addEventListener(event, handler);
    this.handlers.push([target, event, handler]);
  }

  /** Remove all event listeners (call on component destroy) */
  detach(): void {
    for (const [target, event, handler] of this.handlers) {
      target.removeEventListener(event, handler);
    }
    this.handlers = [];
    this.canvas = null;
  }

  attach(element: HTMLElement): void {
    this.detach(); // prevent accumulation on re-attach
    this.canvas = element;

    // Keyboard
    // KG: lesson-333-ts57-uint8-bufferlike-2026-04-19 — same strictness wave hit EventListener narrowing
    this.on(window, 'keydown', ((e: KeyboardEvent) => this.state.keys.add(e.code)) as unknown as EventListener);
    this.on(window, 'keyup', ((e: KeyboardEvent) => this.state.keys.delete(e.code)) as unknown as EventListener);

    // Mouse
    this.on(element, 'mousedown', (() => { this.state.mouseDown = true; }) as EventListener);
    this.on(element, 'mouseup', (() => { this.state.mouseDown = false; }) as EventListener);
    this.on(element, 'mousemove', ((e: MouseEvent) => {
      if (this.pointerLocked) {
        this.state.mouseDx += e.movementX;
        this.state.mouseDy += e.movementY;
      } else {
        this.state.mouseX = e.offsetX;
        this.state.mouseY = e.offsetY;
      }
    }) as EventListener);

    // Pointer Lock change
    this.on(document, 'pointerlockchange', (() => {
      this.pointerLocked = document.pointerLockElement === element;
    }) as EventListener);

    // Gamepad
    this.on(window, 'gamepadconnected', (() => { this.state.gamepadConnected = true; }) as EventListener);
    this.on(window, 'gamepaddisconnected', (() => { this.state.gamepadConnected = false; }) as EventListener);
  }

  /** Request pointer lock (FPS camera control) */
  requestPointerLock(): void {
    this.canvas?.requestPointerLock();
  }

  /** Request fullscreen */
  async requestFullscreen(): Promise<void> {
    await this.canvas?.requestFullscreen();
  }

  /** Exit fullscreen */
  async exitFullscreen(): Promise<void> {
    if (document.fullscreenElement) await document.exitFullscreen();
  }

  /** Poll gamepad state (call every frame) */
  pollGamepad(): void {
    const gamepads = navigator.getGamepads();
    const gp = gamepads[0];
    if (!gp) return;

    this.state.leftStick = { x: gp.axes[0] || 0, y: gp.axes[1] || 0 };
    this.state.rightStick = { x: gp.axes[2] || 0, y: gp.axes[3] || 0 };
    this.state.buttons = gp.buttons.map(b => b.pressed);
  }

  /** Get current input state (read every frame) */
  get current(): Readonly<InputState> {
    return this.state;
  }

  /** Reset per-frame deltas (call after processing) */
  resetDeltas(): void {
    this.state.mouseDx = 0;
    this.state.mouseDy = 0;
  }

  /** Check if a key is held */
  isKeyDown(code: string): boolean {
    return this.state.keys.has(code);
  }

  get isPointerLocked(): boolean { return this.pointerLocked; }
  get isFullscreen(): boolean { return !!document.fullscreenElement; }
}
