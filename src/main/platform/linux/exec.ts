import { execFile } from 'node:child_process';

export function execText(cmd: string, args: string[], timeout = 3000, env?: NodeJS.ProcessEnv): Promise<string | null> {
  return new Promise((resolve) => {
    execFile(cmd, args, { timeout, maxBuffer: 4 << 20, env }, (err, stdout) => {
      if (err) return resolve(null);
      resolve(String(stdout));
    });
  });
}

/**
 * Parses a flat GVariant tuple printed by `gdbus call`, such as:
 * `('org.gnome.Terminal', 'gnome-terminal-server', "Title with 'quotes'", uint32 1234)`
 */
export function parseGVariantTuple(raw: string): (string | number | boolean)[] | null {
  const s = raw.trim();
  if (!s.startsWith('(') || !s.endsWith(')')) return null;
  const inner = s.slice(1, -1).trim();
  if (!inner) return [];
  const out: (string | number | boolean)[] = [];
  let i = 0;

  while (i < inner.length) {
    while (i < inner.length && (inner[i] === ' ' || inner[i] === ',' || inner[i] === '\t' || inner[i] === '\n')) i++;
    if (i >= inner.length) break;

    const ch = inner[i];
    if (ch === "'" || ch === '"') {
      const quote = ch;
      i++;
      let str = '';
      while (i < inner.length) {
        const c = inner[i++];
        if (c === '\\' && i < inner.length) {
          const esc = inner[i++];
          if (esc === 'n') str += '\n';
          else if (esc === 't') str += '\t';
          else if (esc === 'r') str += '\r';
          else str += esc;
        } else if (c === quote) {
          break;
        } else {
          str += c;
        }
      }
      out.push(str);
    } else {
      let end = i;
      while (end < inner.length && inner[end] !== ',') end++;
      const tok = inner.slice(i, end).trim();
      i = end + 1;
      if (!tok) continue;
      if (tok === 'true') out.push(true);
      else if (tok === 'false') out.push(false);
      else {
        const numMatch = tok.match(/(?:^(?:u?int(?:16|32|64)|double|byte)\s+)?(-?\d+(?:\.\d+)?)$/);
        if (numMatch) out.push(Number(numMatch[1]));
        else out.push(tok);
      }
    }
  }
  return out;
}
