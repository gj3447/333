// KG: TASK_333_B_Render, CONTRACT_333_B_Render
// KG: SPAN_333_B_Render — WebGL2 voxel grid renderer
// Architecture: flat 8x8 grid, color from CRDT world state, top-down camera
// Render budget: <8ms per frame at 60fps
// Integration: GameLoop.render(alpha) calls VoxelRenderer.draw()

const GRID_SIZE = 8;

// Block color mapping (matches room/+page.svelte blockColors)
const BLOCK_COLORS: Record<string, [number, number, number]> = {
  stone:  [0.42, 0.45, 0.50],
  grass:  [0.20, 0.83, 0.60],
  water:  [0.40, 0.91, 0.98],
  fire:   [0.96, 0.45, 0.55],
  '':     [0.12, 0.12, 0.18], // empty
};

const VERTEX_SHADER = `#version 300 es
  in vec2 a_position;
  in vec3 a_color;
  out vec3 v_color;
  uniform vec2 u_resolution;

  void main() {
    vec2 clipSpace = (a_position / u_resolution) * 2.0 - 1.0;
    gl_Position = vec4(clipSpace * vec2(1, -1), 0, 1);
    v_color = a_color;
  }
`;

const FRAGMENT_SHADER = `#version 300 es
  precision mediump float;
  in vec3 v_color;
  out vec4 outColor;

  void main() {
    outColor = vec4(v_color, 1.0);
  }
`;

/**
 * WebGL2 Voxel Grid Renderer
 * KG: CONTRACT_333_B_Render — 8x8 colored grid, CRDT-driven, 60fps
 */
export class VoxelRenderer {
  private gl: WebGL2RenderingContext | null = null;
  private program: WebGLProgram | null = null;
  private vao: WebGLVertexArrayObject | null = null;
  private vbo: WebGLBuffer | null = null;
  private colorBuf: WebGLBuffer | null = null;
  private resolutionLoc: WebGLUniformLocation | null = null;
  private canvas: HTMLCanvasElement | null = null;
  private vertexCount = 0;

  /**
   * Initialize WebGL2 context on a canvas element
   */
  init(canvas: HTMLCanvasElement): boolean {
    this.canvas = canvas;
    const gl = canvas.getContext('webgl2', { antialias: false, alpha: false });
    if (!gl) {
      console.error('WebGL2 not supported');
      return false;
    }
    this.gl = gl;

    // Compile shaders
    const vs = this.compileShader(gl.VERTEX_SHADER, VERTEX_SHADER);
    const fs = this.compileShader(gl.FRAGMENT_SHADER, FRAGMENT_SHADER);
    if (!vs || !fs) return false;

    // Link program
    const program = gl.createProgram()!;
    gl.attachShader(program, vs);
    gl.attachShader(program, fs);
    gl.linkProgram(program);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
      console.error('Program link failed:', gl.getProgramInfoLog(program));
      return false;
    }
    this.program = program;

    // Get uniform location
    this.resolutionLoc = gl.getUniformLocation(program, 'u_resolution');

    // Create VAO
    this.vao = gl.createVertexArray()!;
    gl.bindVertexArray(this.vao);

    // Position buffer
    this.vbo = gl.createBuffer()!;
    gl.bindBuffer(gl.ARRAY_BUFFER, this.vbo);
    const posLoc = gl.getAttribLocation(program, 'a_position');
    gl.enableVertexAttribArray(posLoc);
    gl.vertexAttribPointer(posLoc, 2, gl.FLOAT, false, 0, 0);

    // Color buffer
    this.colorBuf = gl.createBuffer()!;
    gl.bindBuffer(gl.ARRAY_BUFFER, this.colorBuf);
    const colorLoc = gl.getAttribLocation(program, 'a_color');
    gl.enableVertexAttribArray(colorLoc);
    gl.vertexAttribPointer(colorLoc, 3, gl.FLOAT, false, 0, 0);

    gl.bindVertexArray(null);

    // Set clear color
    gl.clearColor(0.06, 0.06, 0.1, 1.0);

    return true;
  }

  /**
   * Update mesh from CRDT world state and draw
   * KG: CONTRACT_333_B_Render — render from Map<string, string> world state
   */
  draw(blocks: Map<string, string>): void {
    const gl = this.gl;
    if (!gl || !this.program || !this.canvas) return;

    const w = this.canvas.width;
    const h = this.canvas.height;
    const cellW = w / GRID_SIZE;
    const cellH = h / GRID_SIZE;

    // Build vertex data (2 triangles per cell = 6 vertices per cell)
    const positions: number[] = [];
    const colors: number[] = [];

    for (let y = 0; y < GRID_SIZE; y++) {
      for (let x = 0; x < GRID_SIZE; x++) {
        const key = `${x},${y}`;
        const blockType = blocks.get(key) || '';
        const [r, g, b] = BLOCK_COLORS[blockType] || BLOCK_COLORS[''];

        const x0 = x * cellW + 1;
        const y0 = y * cellH + 1;
        const x1 = (x + 1) * cellW - 1;
        const y1 = (y + 1) * cellH - 1;

        // Triangle 1
        positions.push(x0, y0, x1, y0, x0, y1);
        // Triangle 2
        positions.push(x1, y0, x1, y1, x0, y1);

        // 6 vertices, same color
        for (let i = 0; i < 6; i++) colors.push(r, g, b);
      }
    }

    this.vertexCount = positions.length / 2;

    // Upload data
    gl.bindBuffer(gl.ARRAY_BUFFER, this.vbo!);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(positions), gl.DYNAMIC_DRAW);

    gl.bindBuffer(gl.ARRAY_BUFFER, this.colorBuf!);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(colors), gl.DYNAMIC_DRAW);

    // Draw
    gl.viewport(0, 0, w, h);
    gl.clear(gl.COLOR_BUFFER_BIT);
    gl.useProgram(this.program);
    gl.uniform2f(this.resolutionLoc!, w, h);
    gl.bindVertexArray(this.vao!);
    gl.drawArrays(gl.TRIANGLES, 0, this.vertexCount);
    gl.bindVertexArray(null);
  }

  /**
   * Resize canvas to match container
   */
  resize(width: number, height: number): void {
    if (!this.canvas) return;
    this.canvas.width = width;
    this.canvas.height = height;
  }

  destroy(): void {
    const gl = this.gl;
    if (!gl) return;
    if (this.program) gl.deleteProgram(this.program);
    if (this.vao) gl.deleteVertexArray(this.vao);
    if (this.vbo) gl.deleteBuffer(this.vbo);
    if (this.colorBuf) gl.deleteBuffer(this.colorBuf);
    this.gl = null;
  }

  private compileShader(type: number, source: string): WebGLShader | null {
    const gl = this.gl!;
    const shader = gl.createShader(type)!;
    gl.shaderSource(shader, source);
    gl.compileShader(shader);
    if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
      console.error('Shader compile error:', gl.getShaderInfoLog(shader));
      gl.deleteShader(shader);
      return null;
    }
    return shader;
  }
}
