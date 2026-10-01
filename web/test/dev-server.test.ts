// Run with `pnpm test` (plain `node --test`; Node strips the types itself).

import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { localConfigPath, publicServer, publicUrlFrom } from '../dev-server.ts'

const config = (line: string) => `bind = "127.0.0.1:4810"\n${line}\nlaunch_date = "2026-10-01"\n\n[store]\nkind = "sqlite"\n`

test('the public address is read from the config file', () => {
  assert.equal(publicUrlFrom([config('public_url = "https://needledrop.example"')], undefined), 'https://needledrop.example')
  assert.equal(publicUrlFrom([config("public_url='http://10.0.0.5:4811'")], undefined), 'http://10.0.0.5:4811')
  assert.equal(publicUrlFrom([config('  public_url   =   "https://needledrop.example"   # the game')], undefined), 'https://needledrop.example')
})

test('no key, a blank one or a commented one is no public address', () => {
  assert.equal(publicUrlFrom([config('')], undefined), undefined)
  assert.equal(publicUrlFrom([config('public_url = ""')], undefined), undefined)
  assert.equal(publicUrlFrom([config('public_url = "  "')], undefined), undefined)
  assert.equal(publicUrlFrom([config('# public_url = "https://needledrop.example"')], undefined), undefined)
  assert.equal(publicUrlFrom([undefined], undefined), undefined)
  assert.equal(publicUrlFrom([''], undefined), undefined)
})

test('a key of the same name inside a table is another setting', () => {
  const text = 'launch_date = "2026-10-01"\n\n[store]\npublic_url = "https://not-this.example"\n'
  assert.equal(publicUrlFrom([text], undefined), undefined)
})

test('the environment wins over the file, unless it is blank', () => {
  const text = config('public_url = "https://from-the-file.example"')
  assert.equal(publicUrlFrom([text], 'https://from-the-environment.example'), 'https://from-the-environment.example')
  assert.equal(publicUrlFrom([undefined], ' https://from-the-environment.example '), 'https://from-the-environment.example')
  assert.equal(publicUrlFrom([text], ''), 'https://from-the-file.example')
  assert.equal(publicUrlFrom([text], '   '), 'https://from-the-file.example')
})

test('without a public address the dev server is left as Vite makes it', () => {
  assert.deepEqual(publicServer(undefined), {})
})

test('an https address is answered and hot reload goes to it over wss', () => {
  assert.deepEqual(publicServer('https://needledrop.example'), {
    allowedHosts: ['needledrop.example'],
    hmr: { protocol: 'wss', host: 'needledrop.example', clientPort: 443 },
  })
  assert.deepEqual(publicServer('https://needledrop.example:8443/'), {
    allowedHosts: ['needledrop.example'],
    hmr: { protocol: 'wss', host: 'needledrop.example', clientPort: 8443 },
  })
})

test('an http address uses ws and its own port', () => {
  assert.deepEqual(publicServer('http://needledrop.example'), {
    allowedHosts: ['needledrop.example'],
    hmr: { protocol: 'ws', host: 'needledrop.example', clientPort: 80 },
  })
  assert.deepEqual(publicServer('http://192.168.1.20:8080'), {
    allowedHosts: ['192.168.1.20'],
    hmr: { protocol: 'ws', host: '192.168.1.20', clientPort: 8080 },
  })
})

test('an address that is not one is an error that names the setting', () => {
  for (const value of ['needledrop.example', 'ftp://needledrop.example', 'https://']) {
    assert.throws(
      () => publicServer(value),
      (error: Error) => error.message.includes('public_url') && error.message.includes(JSON.stringify(value)),
      value,
    )
  }
})

test('the local settings file wins over the committed one', () => {
  const committed = config('public_url = "https://committed.example"')
  const local = 'public_url = "https://this-machine.example"\n'
  assert.equal(publicUrlFrom([local, committed], undefined), 'https://this-machine.example')
  // A local file that says nothing about it leaves the committed file's.
  assert.equal(publicUrlFrom(['bind = "127.0.0.1:9999"\n', committed], undefined), 'https://committed.example')
  assert.equal(publicUrlFrom([undefined, committed], undefined), 'https://committed.example')
  // A blank key says "no public address" and is not looked past.
  assert.equal(publicUrlFrom(['public_url = ""\n', committed], undefined), undefined)
  // The environment still wins over both.
  assert.equal(publicUrlFrom([local, committed], 'https://from-the-environment.example'), 'https://from-the-environment.example')
  assert.equal(publicUrlFrom([], undefined), undefined)
})

test('the local settings file sits next to the config file', () => {
  assert.equal(localConfigPath('/repo/config.toml'), '/repo/config.local.toml')
  assert.equal(localConfigPath('config.toml'), 'config.local.toml')
  assert.equal(localConfigPath('/etc/needledrop/prod.toml'), '/etc/needledrop/prod.local.toml')
  // The last extension is replaced, as the server's `with_extension` does it.
  assert.equal(localConfigPath('/repo/settings.dev.toml'), '/repo/settings.dev.local.toml')
  assert.equal(localConfigPath('/repo/settings.conf'), '/repo/settings.local.toml')
  assert.equal(localConfigPath('/repo.d/settings'), '/repo.d/settings.local.toml')
})

test("the repo's config file names no host", () => {
  const text = readFileSync(new URL('../../config.toml', import.meta.url), 'utf8')
  // It is committed, and a hostname belongs in config.local.toml.
  assert.equal(publicUrlFrom([text], undefined), undefined)
  assert.deepEqual(publicServer(publicUrlFrom([text], undefined)), {})
})
