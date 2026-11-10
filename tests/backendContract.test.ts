import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { BACKEND_COMMANDS } from '../src/lib/backend';
import { SCHEDULING_MODES } from '../src/types/config';

const read = (relative: string) => readFileSync(fileURLToPath(new URL(relative, import.meta.url)), 'utf8');

/** Command names registered in `tauri::generate_handler![...]`. */
function registeredCommands(): string[] {
  const source = read('../src-tauri/src/lib.rs');
  const block = source.match(/generate_handler!\[([\s\S]*?)\]/);
  if (!block) throw new Error('generate_handler! block not found in lib.rs');
  return block[1]
    .split('\n')
    .map(line => line.replace(/\/\/.*$/, '').trim().replace(/,$/, ''))
    .filter(Boolean)
    .map(path => path.split('::').pop() as string);
}

/** Names of the `#[tauri::command]` functions defined in commands.rs. */
function definedCommands(): string[] {
  const source = read('../src-tauri/src/commands.rs');
  return [...source.matchAll(/#\[tauri::command\]\s*pub async fn (\w+)/g)].map(match => match[1]);
}

describe('frontend / backend command contract', () => {
  it('finds the commands in the Rust sources', () => {
    expect(registeredCommands().length).toBeGreaterThan(20);
    expect(definedCommands().length).toBeGreaterThan(20);
  });

  it('registers every command that is defined, and nothing else', () => {
    expect([...registeredCommands()].sort()).toEqual([...definedCommands()].sort());
  });

  // The names the UI may invoke must be exactly the ones the backend exposes.
  it('lists the same commands in the TypeScript contract', () => {
    expect([...BACKEND_COMMANDS].sort()).toEqual([...registeredCommands()].sort());
  });

  it('has no duplicate command names', () => {
    expect(new Set(BACKEND_COMMANDS).size).toBe(BACKEND_COMMANDS.length);
    expect(new Set(registeredCommands()).size).toBe(registeredCommands().length);
  });

  // Regression: the UI offered scheduling modes the backend does not have.
  it('offers exactly the routing strategies the backend implements', () => {
    const models = read('../src-tauri/src/models.rs');
    const body = models.match(/pub enum RoutingStrategy \{([\s\S]*?)\}/);
    if (!body) throw new Error('RoutingStrategy enum not found in models.rs');
    const variants = body[1]
      .split('\n')
      .map(line => line.trim().replace(/,$/, ''))
      .filter(line => /^[A-Z]\w*$/.test(line))
      .map(name => name.replace(/([a-z])([A-Z])/g, '$1_$2').toLowerCase());
    expect([...SCHEDULING_MODES].sort()).toEqual(variants.sort());
  });
});
