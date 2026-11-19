import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { app } from 'electron';
import { parseWorkerLine, type Win32RawPayload } from './normalize';

const REQUEST_TIMEOUT_MS = 3500;
const STARTUP_TIMEOUT_MS = 12_000;
const RESTART_COOLDOWN_MS = 3000;

interface PendingRequest {
  resolve: (value: unknown) => void;
  timer: NodeJS.Timeout;
}

function resolvePowerShellPath(): string {
  const systemRoot = process.env.SystemRoot || 'C:\\Windows';
  const classic = path.join(systemRoot, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
  if (fs.existsSync(classic)) return classic;
  return 'powershell.exe';
}

function resolveHelperScriptPath(): string {
  return app.isPackaged
    ? path.join(process.resourcesPath, 'win32', 'helper.ps1')
    : path.join(app.getAppPath(), 'resources', 'win32', 'helper.ps1');
}

/**
 * Manages a single persistent PowerShell helper process (`resources/win32/helper.ps1`)
 * that compiles its C# Win32/UIA/CoreAudio interop once on startup and answers
 * JSONL requests in a few milliseconds.
 */
export class Win32Worker {
  private proc: ChildProcessWithoutNullStreams | null = null;
  private buffer = '';
  private nextId = 1;
  private pending = new Map<number, PendingRequest>();
  private readyPromise: Promise<boolean> | null = null;
  private resolveReady: ((ok: boolean) => void) | null = null;
  private lastSpawnFailureAt = 0;

  uiaAvailable = true;
  audioAvailable = true;

  private ensureStarted(): Promise<boolean> {
    if (this.proc && !this.proc.killed && this.readyPromise) return this.readyPromise;
    if (Date.now() - this.lastSpawnFailureAt < RESTART_COOLDOWN_MS) return Promise.resolve(false);

    const scriptPath = resolveHelperScriptPath();
    if (!fs.existsSync(scriptPath)) {
      this.lastSpawnFailureAt = Date.now();
      return Promise.resolve(false);
    }

    const psPath = resolvePowerShellPath();
    this.buffer = '';

    this.readyPromise = new Promise<boolean>((resolve) => {
      let settled = false;
      const done = (ok: boolean) => {
        if (settled) return;
        settled = true;
        clearTimeout(startupTimer);
        this.resolveReady = null;
        if (!ok) this.lastSpawnFailureAt = Date.now();
        resolve(ok);
      };
      this.resolveReady = done;

      const startupTimer = setTimeout(() => {
        done(false);
        this.killProcess();
      }, STARTUP_TIMEOUT_MS);

      try {
        const child = spawn(
          psPath,
          ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-WindowStyle', 'Hidden', '-File', scriptPath],
          { windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] },
        );
        this.proc = child;

        child.stdout.setEncoding('utf8');
        child.stdout.on('data', (chunk: string) => this.onStdout(chunk));
        child.stderr.on('data', () => {
          /* ignore PowerShell stderr noise */
        });

        child.on('error', () => {
          this.cleanup(child);
          done(false);
        });
        child.on('exit', () => {
          this.cleanup(child);
          done(false);
        });
      } catch {
        done(false);
      }
    });

    return this.readyPromise;
  }

  private onStdout(chunk: string) {
    this.buffer += chunk;
    const lines = this.buffer.split(/\r?\n/);
    this.buffer = lines.pop() ?? '';

    for (const line of lines) {
      const msg = parseWorkerLine(line);
      if (!msg) continue;

      if (msg.ready) {
        if (typeof msg.uia === 'boolean') this.uiaAvailable = msg.uia;
        if (typeof msg.audio === 'boolean') this.audioAvailable = msg.audio;
        this.resolveReady?.(true);
        continue;
      }

      if (typeof msg.id === 'number') {
        const req = this.pending.get(msg.id);
        if (req) {
          this.pending.delete(msg.id);
          clearTimeout(req.timer);
          req.resolve(msg.ok ? msg.data : null);
        }
      }
    }
  }

  private cleanup(child: ChildProcessWithoutNullStreams) {
    if (this.proc === child) {
      this.proc = null;
      this.readyPromise = null;
    }
    for (const [id, req] of this.pending) {
      clearTimeout(req.timer);
      req.resolve(null);
      this.pending.delete(id);
    }
  }

  private killProcess() {
    const child = this.proc;
    if (!child) return;
    this.proc = null;
    this.readyPromise = null;
    try {
      child.stdin.write(`${JSON.stringify({ id: 0, cmd: 'quit' })}\n`);
      child.stdin.end();
    } catch {
      /* ignore broken pipe */
    }
    try {
      child.kill();
    } catch {
      /* ignore already exited */
    }
    this.cleanup(child);
  }

  private async request<T>(payload: Record<string, unknown>, timeoutMs = REQUEST_TIMEOUT_MS): Promise<T | null> {
    const ready = await this.ensureStarted();
    if (!ready || !this.proc || this.proc.killed) return null;

    const id = this.nextId++;
    return new Promise<T | null>((resolve) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        resolve(null);
      }, timeoutMs);

      this.pending.set(id, { resolve: (v) => resolve(v as T | null), timer });
      try {
        this.proc!.stdin.write(`${JSON.stringify({ id, ...payload })}\n`);
      } catch {
        clearTimeout(timer);
        this.pending.delete(id);
        this.killProcess();
        resolve(null);
      }
    });
  }

  async sample(browserTabs: boolean): Promise<Win32RawPayload | null> {
    const data = await this.request<Win32RawPayload | null>({ cmd: 'sample', browserTabs });
    if (!data || typeof data !== 'object') return null;
    return {
      hwnd: Number(data.hwnd) || 0,
      pid: Number(data.pid) || 0,
      exePath: String(data.exePath || ''),
      fileDescription: String(data.fileDescription || ''),
      productName: String(data.productName || ''),
      title: String(data.title || ''),
      url: String(data.url || ''),
    };
  }

  async isProcessPlayingAudio(pid: number, exePath: string): Promise<boolean> {
    if (!this.audioAvailable) return false;
    const data = await this.request<{ active?: boolean }>({ cmd: 'media', pid, exePath }, 2000);
    return Boolean(data?.active);
  }

  dispose() {
    this.killProcess();
  }
}
