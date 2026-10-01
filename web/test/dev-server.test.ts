import assert from 'node:assert/strict'
import { test } from 'node:test'
import { publicServer, publicUrlFromEnv } from '../dev-server.ts'

test('ND_PUBLIC_URL is optional and trimmed', () => {
  assert.equal(publicUrlFromEnv(undefined), undefined)
  assert.equal(publicUrlFromEnv('  '), undefined)
  assert.equal(publicUrlFromEnv(' https://needledrop.example '), 'https://needledrop.example')
  assert.deepEqual(publicServer(undefined), {})
})

test('https public origin sets Vite host and secure hot reload', () => {
  assert.deepEqual(publicServer('https://needledrop.example'), {
    allowedHosts: ['needledrop.example'],
    hmr: { protocol: 'wss', host: 'needledrop.example', clientPort: 443 },
  })
  assert.deepEqual(publicServer('https://needledrop.example:8443/'), {
    allowedHosts: ['needledrop.example'],
    hmr: { protocol: 'wss', host: 'needledrop.example', clientPort: 8443 },
  })
})

test('http public origin sets its own websocket port', () => {
  assert.deepEqual(publicServer('http://192.168.1.20:8080'), {
    allowedHosts: ['192.168.1.20'],
    hmr: { protocol: 'ws', host: '192.168.1.20', clientPort: 8080 },
  })
})

test('invalid public origin names ND_PUBLIC_URL', () => {
  for (const value of [
    'needledrop.example', 'ftp://needledrop.example', 'https://',
    'https://user@needledrop.example', 'https://needledrop.example/app',
    'https://needledrop.example?q=x',
  ]) {
    assert.throws(
      () => publicServer(value),
      (error: Error) => error.message.includes('ND_PUBLIC_URL') && error.message.includes(JSON.stringify(value)),
      value,
    )
  }
})
