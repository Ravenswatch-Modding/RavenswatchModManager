import { readFileSync, readdirSync } from 'node:fs';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import postcss from 'postcss';
import tailwindcss from 'tailwindcss';
import { describe, expect, it } from 'vitest';

const root = fileURLToPath(new URL('../../../../', import.meta.url));
const read = (path: string) => readFileSync(resolve(root, path), 'utf8');
const tokens = postcss.parse(read('packages/ui/src/tokens.css'));
const values = new Map<string, string>();
tokens.walkDecls((decl) => {
  if (decl.prop.startsWith('--')) values.set(decl.prop, decl.value);
});

function resolveToken(name: string): string {
  const value = values.get(name);
  if (!value) throw new Error(`Missing token ${name}`);
  return value.replace(/var\((--[\w-]+)\)/g, (_, alias) => resolveToken(alias));
}

type RGB = [number, number, number];
function color(name: string): RGB {
  const [h, s, l] = resolveToken(name).split(/\s+/).map(Number.parseFloat) as RGB;
  const lightness = l / 100;
  const a = (s / 100) * Math.min(lightness, 1 - lightness);
  return [0, 8, 4].map((n) => {
    const k = (n + h / 30) % 12;
    return lightness - a * Math.max(-1, Math.min(k - 3, 9 - k, 1));
  }) as RGB;
}

function composite(fg: RGB, bg: RGB, alpha: number): RGB {
  return ([0, 1, 2] as const).map((i) => fg[i] * alpha + bg[i] * (1 - alpha)) as RGB;
}

function luminance(rgb: RGB): number {
  const linear = rgb.map((c) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4)) as RGB;
  return linear[0] * 0.2126 + linear[1] * 0.7152 + linear[2] * 0.0722;
}

function contrast(fg: RGB, bg: RGB): number {
  const a = luminance(fg);
  const b = luminance(bg);
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

// Base surface plus the actual translucent web card, selected registry row,
// and destructive/10 feedback wash. Evaluate only supported role/surface pairs:
// selected rows use primary/secondary ink, not tertiary or danger ink.
const surfaces = {
  page: color('--surface-page'),
  panel: color('--surface-panel'),
  raised: color('--surface-raised'),
  webCard: composite(color('--card'), color('--surface-page'), 0.9),
  error: composite(color('--destructive'), color('--surface-raised'), 0.1),
  selected: composite(color('--char'), color('--surface-raised'), 0.55),
};

describe('shared theme contract', () => {
  it('is exported and imported by both hosts without a second token definition', async () => {
    // Next processes CSS imports separately; tokens must work without a host's
    // @tailwind base directive (unlike Vite's inlined-import pipeline).
    await postcss([tailwindcss({ content: [{ raw: 'text-sm' }] })]).process(tokens.clone(), {
      from: undefined,
    });
    const pkg = JSON.parse(read('packages/ui/package.json'));
    expect(pkg.exports['./tokens.css']).toBe('./src/tokens.css');
    const hosts = [
      ['packages/ui/src/styles.css', './tokens.css'],
      ['apps/www/src/app/globals.css', '@rsmm/ui/tokens.css'],
    ] as const;
    for (const [path, imported] of hosts) {
      const css = postcss.parse(read(path));
      const imports: string[] = [];
      css.walkAtRules('import', (rule) => {
        imports.push(rule.params.replace(/["']/g, ''));
      });
      expect(imports, path).toContain(imported);
      css.walkDecls((decl) => {
        expect(values.has(decl.prop), `${path} forks ${decl.prop}`).toBe(false);
      });
    }
    const schemes: string[] = [];
    tokens.walkDecls('color-scheme', (decl) => {
      schemes.push(decl.value);
    });
    expect(schemes).toEqual(['dark']);
    expect([...values.keys()].some((name) => name.startsWith('--font-'))).toBe(false);
  });

  it('keeps action fills separate from danger ink in generated Tailwind utilities', async () => {
    const require = createRequire(import.meta.url);
    const preset = require(resolve(root, 'packages/ui/tailwind.preset.cjs'));
    const result = await postcss([
      tailwindcss({
        presets: [preset],
        content: [{ raw: 'text-danger text-crimson bg-destructive text-destructive-foreground' }],
      }),
    ]).process('@tailwind utilities;', { from: undefined });
    const declarations = new Map<string, string>();
    result.root.walkRules((rule) => {
      rule.walkDecls((decl) => {
        declarations.set(`${rule.selector}:${decl.prop}`, decl.value);
      });
    });
    expect(declarations.get('.text-danger:color')).toContain('var(--text-danger)');
    expect(declarations.get('.text-crimson:color')).toContain('var(--text-danger)');
    expect(declarations.get('.bg-destructive:background-color')).toContain('var(--destructive)');
    expect(declarations.get('.text-destructive-foreground:color')).toContain(
      'var(--destructive-foreground)',
    );
    expect(resolveToken('--primary')).toBe('356 62% 33%');
    expect(resolveToken('--destructive')).toBe('356 70% 42%');
    expect(resolveToken('--text-danger')).not.toBe(resolveToken('--destructive'));
  });

  for (const [surface, background] of Object.entries(surfaces)) {
    const roles = ['--text-primary', '--text-secondary'];
    if (surface !== 'selected') roles.push('--text-tertiary', '--text-danger');
    for (const role of roles) {
      it(`${role} clears normal-text AA on ${surface}`, () => {
        expect(contrast(color(role), background)).toBeGreaterThanOrEqual(4.5);
      });
    }
    it(`focus remains visible on ${surface}`, () => {
      expect(contrast(color('--focus-ring'), background)).toBeGreaterThanOrEqual(3);
    });
  }

  it('does not reintroduce the destructive fill as web text', () => {
    const source = resolve(root, 'apps/www/src');
    const offenders = readdirSync(source, { recursive: true, encoding: 'utf8' })
      .filter((entry) => /\.tsx?$/.test(entry))
      .map((entry) => resolve(source, entry))
      .filter((path) => /\btext-destructive(?![-\w])/.test(readFileSync(path, 'utf8')));
    expect(offenders).toEqual([]);
  });
});
