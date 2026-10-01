// A stand-in for the Rust server's /api routes, for working on the UI alone.
//
//   GTS_MOCK=1 pnpm dev
//
// Loaded only when GTS_MOCK=1 (see vite.config.ts); without the flag this
// file is never imported. One game per process and no cookie: every browser
// shares it, and restarting Vite or `POST /api/mock/reset` starts over.
//
// Audio: the file named by GTS_MOCK_AUDIO, else the server's test fixture
// (git-ignored, may be missing), cut like the server cuts it. With neither, a
// generated tone. Never copy a preview into the repo: it is copyrighted.

import { readFileSync } from 'node:fs'
import type { IncomingMessage, ServerResponse } from 'node:http'
import { resolve } from 'node:path'
import type { Plugin } from 'vite'
import { parseMp3 } from './mp3.ts'
import type { Mp3 } from './mp3.ts'

const LADDER = [0.1, 0.3, 1, 3, 8, 16, 30]
const SEARCH_DELAY_MS = 150
const MAX_RESULTS = 8

type Attempt = { kind: 'skip' } | { kind: 'wrong'; title: string; artist: string }
type Status = 'playing' | 'won' | 'lost'

interface Track {
  id: number
  title: string
  artist: string
  album: string
  cover: string
}

function cover(text: string, colour: string): string {
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 80 80"><rect width="80" height="80" fill="${colour}"/>` +
    `<text x="40" y="52" font-family="sans-serif" font-size="34" text-anchor="middle" fill="#fff">${text}</text></svg>`
  return `data:image/svg+xml,${encodeURIComponent(svg)}`
}

// Invented songs: whatever audio is served, none of these claims to be it.
const SONGS: [title: string, artist: string, album: string, colour: string][] = [
  ['Paper Boats', 'The Lanterns', 'Harbour Lights', '#2340e0'],
  ['Paper Planes at Midnight', 'Ada Voss', 'Night Flights', '#b3261e'],
  ['Under the Overpass', 'The Lanterns', 'Harbour Lights', '#2340e0'],
  ['Señorita del Mar', 'Los Niños Perdidos', 'Costa Brava', '#0b6e4f'],
  ['Slow Train to Nowhere', 'Marlowe & the Night Shift', 'Second Class', '#5b3a8c'],
  ['Slow Dance', 'Ada Voss', 'Night Flights', '#b3261e'],
  ['Slow Burn', 'Kite Season', 'Embers', '#a65a00'],
  ['Slow Motion', 'Glass Animals of the North', 'Frames', '#205072'],
  ['Slow Down Sunday', 'The Lanterns', 'Low Tide', '#2340e0'],
  ['Slow Hands, Fast Heart', 'Ruby Okafor', 'Pulse', '#8a1c5a'],
  ['Slower', 'Kite Season', 'Embers', '#a65a00'],
  ['Slow', 'Marlowe & the Night Shift', 'Second Class', '#5b3a8c'],
  ['Slowly, Slowly', 'Ruby Okafor', 'Pulse', '#8a1c5a'],
  [
    'A Very Long Song Title That Keeps Going Well Past the Edge of a Narrow Phone Screen',
    'An Artist With an Equally Unreasonable Name and Several Featured Friends',
    'Overflow',
    '#444444',
  ],
]

const CATALOGUE: Track[] = SONGS.map(([title, artist, album, colour], i) => ({
  id: 1001 + i,
  title,
  artist,
  album,
  cover: cover(title[0], colour),
}))

// The answer is the first song: search "paper" and pick "Paper Boats" to win.
const ANSWER = CATALOGUE[0]

interface Game {
  attempts: Attempt[]
  status: Status
}

function today(): string {
  return new Date().toISOString().slice(0, 10)
}

function daily(game: Game) {
  const finished = game.status !== 'playing'
  return {
    day: today(),
    number: 1,
    ladder: LADDER,
    attempts: game.attempts,
    status: game.status,
    clipSeconds: finished ? LADDER[LADDER.length - 1] : LADDER[game.attempts.length],
    answer: finished
      ? {
          title: ANSWER.title,
          artist: ANSWER.artist,
          album: ANSWER.album,
          cover: ANSWER.cover,
          link: 'https://www.deezer.com/',
        }
      : null,
  }
}

function loadMp3(root: string): Mp3 | null {
  const candidates = [process.env.GTS_MOCK_AUDIO, resolve(root, '../server/tests/fixtures/preview.mp3')]
  for (const path of candidates) {
    if (!path) continue
    try {
      const mp3 = parseMp3(readFileSync(path))
      if (mp3) {
        console.log(`[mock] audio: ${path} (${mp3.frames} frames, ${mp3.seconds.toFixed(3)} s)`)
        return mp3
      }
      console.warn(`[mock] ${path} is not an MPEG-1 Layer III file`)
    } catch {
      console.warn(`[mock] cannot read ${path}`)
    }
  }
  console.log('[mock] audio: generated tone (set GTS_MOCK_AUDIO to an MP3 to use real audio)')
  return null
}

/**
 * A rising tone as a 16-bit mono WAV, with 30 ms of silence in front so the
 * player's leading-silence skip has something to skip.
 */
function toneWav(seconds: number): Buffer {
  const rate = 22050
  const lead = Math.round(0.03 * rate)
  const samples = Math.round(seconds * rate)
  const header = Buffer.alloc(44)
  header.write('RIFF', 0)
  header.writeUInt32LE(36 + samples * 2, 4)
  header.write('WAVEfmt ', 8)
  header.writeUInt32LE(16, 16)
  header.writeUInt16LE(1, 20) // PCM
  header.writeUInt16LE(1, 22) // mono
  header.writeUInt32LE(rate, 24)
  header.writeUInt32LE(rate * 2, 28)
  header.writeUInt16LE(2, 32)
  header.writeUInt16LE(16, 34)
  header.write('data', 36)
  header.writeUInt32LE(samples * 2, 40)
  const body = Buffer.alloc(samples * 2)
  let phase = 0
  for (let i = lead; i < samples; i++) {
    const t = (i - lead) / rate
    // One octave up every ten seconds, so the position in the clip is audible.
    phase += (2 * Math.PI * 220 * 2 ** (t / 10)) / rate
    body.writeInt16LE(Math.round(Math.sin(phase) * 0.4 * 32767), i * 2)
  }
  return Buffer.concat([header, body])
}

function sendJson(res: ServerResponse, status: number, body: unknown): void {
  res.statusCode = status
  res.setHeader('Content-Type', 'application/json')
  res.setHeader('Cache-Control', 'no-store')
  res.end(JSON.stringify(body))
}

function sendError(res: ServerResponse, status: number, error: string, message: string): void {
  sendJson(res, status, { error, message })
}

function readBody(req: IncomingMessage): Promise<string> {
  return new Promise((done, fail) => {
    const chunks: Buffer[] = []
    req.on('data', (chunk: Buffer) => chunks.push(chunk))
    req.on('end', () => done(Buffer.concat(chunks).toString('utf8')))
    req.on('error', fail)
  })
}

function search(query: string): Track[] {
  const q = query.trim().toLowerCase()
  if ([...q].length < 2) return []
  return CATALOGUE.filter((track) => `${track.title} ${track.artist}`.toLowerCase().includes(q)).slice(
    0,
    MAX_RESULTS,
  )
}

function applyMove(game: Game, body: unknown, res: ServerResponse): void {
  if (typeof body !== 'object' || body === null) {
    return sendError(res, 400, 'bad_request', 'Send either a track or a skip.')
  }
  const skip = 'skip' in body && body.skip === true
  const trackId = 'trackId' in body && typeof body.trackId === 'number' ? body.trackId : null
  if (skip === (trackId !== null)) {
    return sendError(res, 400, 'bad_request', 'Send either a track or a skip.')
  }
  if (game.status !== 'playing') {
    return sendError(res, 409, 'finished', "Today's game is already over.")
  }
  if (trackId === null) {
    game.attempts.push({ kind: 'skip' })
  } else {
    const track = CATALOGUE.find((candidate) => candidate.id === trackId)
    if (!track) return sendError(res, 404, 'unknown_track', "That song couldn't be found. Pick another one.")
    if (track.id === ANSWER.id) {
      game.status = 'won'
      return sendJson(res, 200, daily(game))
    }
    game.attempts.push({ kind: 'wrong', title: track.title, artist: track.artist })
  }
  if (game.attempts.length >= LADDER.length) game.status = 'lost'
  sendJson(res, 200, daily(game))
}

export function mockApi(): Plugin {
  let game: Game = { attempts: [], status: 'playing' }

  return {
    name: 'gts-mock-api',
    apply: 'serve',
    configureServer(server) {
      const mp3 = loadMp3(server.config.root)
      console.log('[mock] GTS_MOCK=1: /api is answered by web/mock, not by the Rust server')

      // Registered before Vite's own middlewares, so it wins over the /api proxy.
      server.middlewares.use((req, res, next) => {
        const url = new URL(req.url ?? '/', 'http://mock')
        if (!url.pathname.startsWith('/api/')) return next()
        const route = `${req.method} ${url.pathname}`

        switch (route) {
          case 'GET /api/health':
            return sendJson(res, 200, { ok: true })

          case 'GET /api/daily':
            return sendJson(res, 200, daily(game))

          case 'GET /api/daily/audio': {
            const seconds = daily(game).clipSeconds
            const clip = mp3 ? mp3.prefix(seconds * 1000) : toneWav(seconds + 0.1)
            res.statusCode = 200
            res.setHeader('Content-Type', mp3 ? 'audio/mpeg' : 'audio/wav')
            res.setHeader('Cache-Control', 'no-store')
            return res.end(clip)
          }

          case 'GET /api/search': {
            const found = search(url.searchParams.get('q') ?? '')
            // Slow enough to see the "Searching…" hint and to race two requests.
            setTimeout(() => sendJson(res, 200, found), SEARCH_DELAY_MS)
            return
          }

          case 'POST /api/daily/guess':
            readBody(req).then(
              (text) => {
                let body: unknown
                try {
                  body = JSON.parse(text)
                } catch {
                  return sendError(res, 400, 'bad_request', 'The request was not valid JSON.')
                }
                applyMove(game, body, res)
              },
              () => sendError(res, 400, 'bad_request', 'The request could not be read.'),
            )
            return

          // Mock only: start a new game without restarting Vite.
          case 'POST /api/mock/reset':
            game = { attempts: [], status: 'playing' }
            return sendJson(res, 200, daily(game))

          default:
            return sendError(res, 404, 'not_found', 'No such route.')
        }
      })
    },
  }
}
