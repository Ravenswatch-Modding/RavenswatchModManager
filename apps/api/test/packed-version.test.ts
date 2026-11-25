import { beforeEach, describe, expect, it, vi } from 'vitest';

process.env.DATABASE_URL ??= 'postgres://user:pass@localhost:5432/db';
process.env.BETTER_AUTH_SECRET ??= 'x'.repeat(32);

const state = vi.hoisted(() => ({
  sets: [] as Record<string, unknown>[],
  // What the conditional UPDATE matched: none when a sibling row holds the number.
  matched: true,
}));

vi.mock('@rsmm/db', () => ({
  getDb: () => ({
    update: () => ({
      set: (values: Record<string, unknown>) => {
        state.sets.push(values);
        return { where: () => ({ returning: async () => (state.matched ? [{ id: 'v1' }] : []) }) };
      },
    }),
  }),
  schema: new Proxy({}, { get: () => new Proxy({}, { get: () => 'col' }) }),
}));

const { syncPackedVersion } = await import('../src/scan-service.js');
const { assetKey } = await import('../src/storage.js');

/** A stored (uncompressed) zip holding one file. */
function zip(name: string, text: string): Buffer {
  const data = Buffer.from(text, 'utf8');
  const nameBuf = Buffer.from(name, 'utf8');
  const local = Buffer.alloc(30);
  local.writeUInt32LE(0x04034b50, 0);
  local.writeUInt32LE(data.length, 18);
  local.writeUInt32LE(data.length, 22);
  local.writeUInt16LE(nameBuf.length, 26);
  const central = Buffer.alloc(46);
  central.writeUInt32LE(0x02014b50, 0);
  central.writeUInt32LE(data.length, 20);
  central.writeUInt32LE(data.length, 24);
  central.writeUInt16LE(nameBuf.length, 28);
  const cd = Buffer.concat([central, nameBuf]);
  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(0x06054b50, 0);
  eocd.writeUInt16LE(1, 8);
  eocd.writeUInt16LE(1, 10);
  eocd.writeUInt32LE(cd.length, 12);
  eocd.writeUInt32LE(30 + nameBuf.length + data.length, 16);
  return Buffer.concat([local, nameBuf, data, cd, eocd]);
}

const target = { id: 'v1', version: '1.0.1', sha256: 'a'.repeat(64) };
const packed = (version: string) =>
  zip('demo/manifest.toml', `[mod]\nid = "demo"\nversion = "${version}"\n`);

beforeEach(() => {
  state.sets = [];
  state.matched = true;
});

describe('a version label follows its packed manifest', () => {
  it('relabels a version whose archive says otherwise', async () => {
    expect(await syncPackedVersion(target, packed('1.0.0'))).toBe('1.0.0');
    expect(state.sets[0]?.version).toBe('1.0.0');
  });

  it('writes nothing once label and archive agree, so a fixed version is left alone', async () => {
    expect(await syncPackedVersion(target, packed('1.0.1'))).toBeNull();
    expect(state.sets).toHaveLength(0);
  });

  it('leaves the label when the archive has no usable version', async () => {
    expect(
      await syncPackedVersion(target, zip('manifest.toml', '[mod]\nid = "demo"\n')),
    ).toBeNull();
    expect(await syncPackedVersion(target, packed('latest'))).toBeNull();
    expect(await syncPackedVersion(target, Buffer.from('not a zip'))).toBeNull();
    expect(state.sets).toHaveLength(0);
  });

  it('reports no change when another version of the mod already has that number', async () => {
    state.matched = false;
    expect(await syncPackedVersion(target, packed('1.0.0'))).toBeNull();
  });
});

describe('assetKey', () => {
  it('recovers the object key from either public URL form', () => {
    expect(assetKey('https://cdn.rsmm.me/mods/demo/1.0.1-aaaaaaaaaaaa.zip')).toBe(
      'mods/demo/1.0.1-aaaaaaaaaaaa.zip',
    );
    expect(assetKey('https://s3.example.com/bucket/mods/demo/1.0.1-aaaaaaaaaaaa.zip')).toBe(
      'mods/demo/1.0.1-aaaaaaaaaaaa.zip',
    );
  });
});
