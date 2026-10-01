// Run with `pnpm test` (plain `node --test`; Node strips the types itself).

import assert from 'node:assert/strict'
import { test } from 'node:test'
import {
  FIND_LIMIT,
  FIND_NOTHING,
  GENRES,
  POOLS,
  clockNews,
  countOf,
  dayText,
  deezerLink,
  failedText,
  foundText,
  hitNote,
  isIdle,
  isSongsPage,
  moreText,
  offsetText,
  orderGenres,
  pickDetail,
  pickProblem,
  poolLabel,
  poolNotes,
  poolsText,
  refusal,
  removalText,
  rerollNews,
  rerollQuestion,
  sameGenres,
  savedText,
  sectionsPlaying,
  songCount,
  songName,
  songsQuery,
  toggleGenre,
  totalsText,
  withRow,
} from '../src/lib/admin/model.ts'
import type {
  AdminHit,
  AdminState,
  Find,
  Genre,
  PoolCounts,
  SectionPick,
  SongRow,
  SongsPage,
} from '../src/lib/admin/model.ts'

function song(trackId: number, title: string, artist: string, genres: Genre[] = [], failed: string | null = null): SongRow {
  return { trackId, title, artist, album: `${title} (album)`, genres, previewFailedOn: failed }
}

// The six seed songs, in the server's order.
const SEED: SongRow[] = [
  song(92720046, 'Back In Black', 'AC/DC', ['rock']),
  song(15391618, 'Toxic', 'Britney Spears', ['pop']),
  song(1109731, 'Lose Yourself', 'Eminem', ['hip-hop']),
  song(4603408, 'Billie Jean', 'Michael Jackson', ['pop']),
  song(4091937401, 'Bohemian Rhapsody', 'Queen', ['rock']),
  song(3616616, 'Juicy', 'The Notorious B.I.G.', ['hip-hop']),
]

const SEED_COUNTS: PoolCounts = { all: 6, pop: 2, rock: 2, hipHop: 2, previewFailed: 0 }

function counts(all: number, pop: number, rock: number, hipHop: number, previewFailed = 0): PoolCounts {
  return { all, pop, rock, hipHop, previewFailed }
}

function page(songs: SongRow[], total = songs.length, limit = FIND_LIMIT): SongsPage {
  return { total, offset: 0, limit, songs, counts: SEED_COUNTS }
}

function find(text = '', failed = false): Find {
  return { text, failed }
}

function hit(id: number, playable: boolean, inPool = false, genres: Genre[] | null = null, album = 'Thriller'): AdminHit {
  return { id, title: 'Billie Jean', artist: 'Michael Jackson', album, cover: '', playable, inPool, genres }
}

function picked(section: SectionPick['section'], trackId: number, title: string, artist: string): SectionPick {
  return { section, status: 'picked', pick: { trackId, title, artist } }
}

const STATE: AdminState = {
  realDay: '2026-10-01',
  offset: 0,
  day: '2026-10-01',
  number: 1,
  sections: [
    picked('general', 92720046, 'Back In Black', 'AC/DC'),
    picked('pop', 15391618, 'Toxic', 'Britney Spears'),
    picked('rock', 4091937401, 'Bohemian Rhapsody', 'Queen'),
    picked('hip-hop', 1109731, 'Lose Yourself', 'Eminem'),
  ],
}

function withEntry(state: AdminState, entry: SectionPick): AdminState {
  return { ...state, sections: state.sections.map((each) => (each.section === entry.section ? entry : each)) }
}

// --- names ----------------------------------------------------------------------

test('the genres and the pools are the slugs of the API, in its order', () => {
  assert.deepEqual([...GENRES], ['pop', 'rock', 'hip-hop'])
  assert.deepEqual([...POOLS], ['general', 'pop', 'rock', 'hip-hop'])
  assert.deepEqual(POOLS.map(poolLabel), ['General', 'Pop', 'Rock', 'Hip-hop'])
})

test('songCount knows one song from several', () => {
  assert.equal(songCount(0), '0 songs')
  assert.equal(songCount(1), '1 song')
  assert.equal(songCount(6), '6 songs')
})

test('songName quotes the title and names the artist', () => {
  assert.equal(songName({ title: 'Toxic', artist: 'Britney Spears' }), '“Toxic” by Britney Spears')
  assert.equal(songName({ title: ' Toxic ', artist: '' }), '“Toxic”')
})

test('songName has words for a pick whose song has left the pool', () => {
  assert.equal(songName({ title: '', artist: '' }), 'a song that is no longer in the pool')
  assert.equal(songName({ title: '', artist: 'Queen' }), 'a song by Queen')
})

test('deezerLink is the track page', () => {
  assert.equal(deezerLink(4603408), 'https://www.deezer.com/track/4603408')
})

// --- genres ---------------------------------------------------------------------

test('toggleGenre adds a missing tag and takes away one that is there', () => {
  assert.deepEqual(toggleGenre([], 'rock'), ['rock'])
  assert.deepEqual(toggleGenre(['rock'], 'rock'), [])
  assert.deepEqual(toggleGenre(['pop', 'hip-hop'], 'pop'), ['hip-hop'])
})

test('toggleGenre answers in the server order, whatever order it was given', () => {
  assert.deepEqual(toggleGenre(['hip-hop'], 'pop'), ['pop', 'hip-hop'])
  assert.deepEqual(toggleGenre(['hip-hop', 'pop'], 'rock'), ['pop', 'rock', 'hip-hop'])
})

test('toggleGenre leaves the list it was given alone', () => {
  const genres: Genre[] = ['pop']
  toggleGenre(genres, 'rock')
  assert.deepEqual(genres, ['pop'])
})

test('orderGenres drops repeats and sorts', () => {
  assert.deepEqual(orderGenres(['hip-hop', 'pop', 'hip-hop']), ['pop', 'hip-hop'])
  assert.deepEqual(orderGenres([]), [])
})

test('sameGenres ignores order and repeats', () => {
  assert.equal(sameGenres(['pop', 'rock'], ['rock', 'pop']), true)
  assert.equal(sameGenres(['pop', 'pop'], ['pop']), true)
  assert.equal(sameGenres([], []), true)
  assert.equal(sameGenres(['pop'], []), false)
  assert.equal(sameGenres(['pop'], ['rock']), false)
})

test('poolsText always names General', () => {
  assert.equal(poolsText([]), 'General only')
  assert.equal(poolsText(['pop']), 'General and Pop')
  assert.equal(poolsText(['hip-hop', 'pop']), 'General, Pop and Hip-hop')
  assert.equal(poolsText(['pop', 'rock', 'hip-hop']), 'General, Pop, Rock and Hip-hop')
})

// --- the counts -----------------------------------------------------------------

test('countOf: General is every song, a genre its tagged ones', () => {
  const some = counts(312, 80, 120, 60, 3)
  assert.deepEqual(POOLS.map((pool) => countOf(some, pool)), [312, 80, 120, 60])
})

test('poolNotes has nothing to say about the seed', () => {
  assert.deepEqual(poolNotes(SEED_COUNTS), [])
})

test('poolNotes warns of a genre with one song or none', () => {
  assert.deepEqual(poolNotes(counts(5, 2, 2, 1)), ['Hip-hop has one song, so it plays that song every day.'])
  assert.deepEqual(poolNotes(counts(4, 2, 0, 2)), ['Rock has no song: it shows “No song today”.'])
})

test('poolNotes warns when the genre sections can take every song', () => {
  const notes = poolNotes(counts(3, 1, 1, 1))
  assert.equal(notes.length, 4)
  assert.equal(
    notes[3],
    'General may have no song: the pool has 3 songs and the genre sections take up to 3 of them each day.',
  )
  // One song more, and General has one left whatever the genres draw.
  assert.equal(poolNotes(counts(4, 1, 1, 1)).length, 3)
  // A pool of one pop song: Pop takes it.
  assert.equal(poolNotes(counts(1, 1, 0, 0)).at(-1), 'General may have no song: the pool has 1 song and the genre sections take up to 1 of them each day.')
})

test('poolNotes says one thing about an empty pool', () => {
  assert.deepEqual(poolNotes(counts(0, 0, 0, 0)), ['The pool is empty: no section has a song.'])
})

// --- finding a song in the pool -------------------------------------------------

test('the totals are one line to read', () => {
  assert.equal(totalsText(SEED_COUNTS), '6 songs · Pop 2 · Rock 2 · Hip-hop 2')
  assert.equal(totalsText(counts(1, 0, 1, 0)), '1 song · Pop 0 · Rock 1 · Hip-hop 0')
  assert.equal(totalsText(counts(312, 80, 120, 60, 3)), '312 songs · Pop 80 · Rock 120 · Hip-hop 60')
})

test('failedText is there only while a preview check has failed', () => {
  assert.equal(failedText(SEED_COUNTS), '')
  assert.equal(failedText(counts(312, 80, 120, 60, 1)), '1 failed the preview check')
  assert.equal(failedText(counts(312, 80, 120, 60, 3)), '3 failed the preview check')
})

test('a search with nothing typed is idle', () => {
  assert.equal(isIdle(FIND_NOTHING), true)
  assert.equal(isIdle(find('   ')), true)
  assert.equal(isIdle(find('q')), false)
  assert.equal(isIdle(find('', true)), false)
})

test('an idle search asks for the counts alone, and for no song', () => {
  assert.equal(songsQuery(FIND_NOTHING), 'limit=0')
  assert.equal(songsQuery(find('  ')), 'limit=0')
})

test('songsQuery sends the text and asks for one screenful', () => {
  assert.equal(FIND_LIMIT, 25)
  assert.equal(songsQuery(find('queen')), 'q=queen&limit=25')
  assert.equal(songsQuery(find('under pressure')), 'q=under%20pressure&limit=25')
  assert.equal(songsQuery(find('', true)), 'failed=true&limit=25')
  assert.equal(songsQuery(find('live', true)), 'q=live&failed=true&limit=25')
})

test('songsQuery never sends an offset or a genre: there is no second page and no filter row', () => {
  for (const each of [FIND_NOTHING, find('a'), find('a', true), find('', true)]) {
    assert.equal(songsQuery(each).includes('offset'), false)
    assert.equal(songsQuery(each).includes('genre'), false)
  }
})

test('songsQuery trims the text and escapes it', () => {
  assert.equal(songsQuery(find('  AC/DC & more ')), 'q=AC%2FDC%20%26%20more&limit=25')
  assert.equal(songsQuery(find('Beyoncé')), 'q=Beyonc%C3%A9&limit=25')
  assert.equal(songsQuery(find('a=b#c?d')), 'q=a%3Db%23c%3Fd&limit=25')
})

test('isSongsPage tells the answer of the searchable route from the old plain list', () => {
  assert.equal(isSongsPage(page(SEED)), true)
  assert.equal(isSongsPage({ total: 0, offset: 0, limit: 0, songs: [], counts: SEED_COUNTS }), true)
  assert.equal(isSongsPage(SEED), false)
  assert.equal(isSongsPage([]), false)
  assert.equal(isSongsPage(null), false)
  assert.equal(isSongsPage('songs'), false)
  assert.equal(isSongsPage({ total: 6, offset: 0, limit: 25, songs: SEED }), false)
  assert.equal(isSongsPage({ error: 'internal', message: 'x' }), false)
})

test('withRow replaces the row of a song that is on screen, where it is', () => {
  const row = { ...SEED[3], genres: ['pop', 'rock'] as Genre[] }
  const before = page(SEED, 40)
  const after = withRow(before, row)
  assert.deepEqual(after.songs[3].genres, ['pop', 'rock'])
  assert.deepEqual(after.songs.map((each) => each.trackId), SEED.map((each) => each.trackId))
  assert.equal(after.total, 40)
  assert.deepEqual(before.songs[3].genres, ['pop'])
})

test('withRow leaves the answer alone when the song is not on screen', () => {
  const before = page(SEED)
  assert.equal(withRow(before, song(1, 'Other', 'Other')), before)
})

test('foundText says nothing for an idle search', () => {
  assert.equal(foundText(FIND_NOTHING, 312), '')
  assert.equal(foundText(find('  '), 0), '')
})

test('foundText counts what matches a text', () => {
  assert.equal(foundText(find('queen'), 3), '3 songs match “queen”.')
  assert.equal(foundText(find(' queen '), 1), '1 song matches “queen”.')
  assert.equal(foundText(find('zzz'), 0), 'No song matches “zzz”.')
})

test('foundText says what the list of failed checks is', () => {
  assert.equal(foundText(find('', true), 3), '3 songs had no preview at the last daily check.')
  assert.equal(foundText(find('', true), 1), '1 song had no preview at the last daily check.')
  assert.equal(foundText(find('', true), 0), 'No song failed its last daily preview check.')
  assert.equal(foundText(find('live', true), 2), '2 songs without a preview at the last daily check match “live”.')
})

test('moreText speaks only when songs were left out', () => {
  assert.equal(moreText(25, 312), 'Showing the first 25 of 312. Refine the search to see the rest.')
  assert.equal(moreText(25, 26), 'Showing the first 25 of 26. Refine the search to see the rest.')
  assert.equal(moreText(25, 25), '')
  assert.equal(moreText(3, 3), '')
  assert.equal(moreText(0, 0), '')
  // An idle search has no rows; the count of the pool is not "more".
  assert.equal(moreText(0, 312), '')
})

// --- adding a song --------------------------------------------------------------

test('hitNote marks a release without a preview, and one that is in the pool', () => {
  assert.equal(hitNote(hit(1, false)), 'No preview')
  assert.equal(hitNote(hit(4603408, true, true, ['pop'])), 'In the pool')
  assert.equal(hitNote(hit(1, true)), null)
  // Not playable matters more than being there.
  assert.equal(hitNote(hit(4603408, false, true, ['pop'])), 'No preview')
})

test('refusal lets a playable release through, in the pool or not', () => {
  assert.equal(refusal(hit(1, true)), null)
  assert.equal(refusal(hit(4603408, true, true, [])), null)
})

test('refusal says why a release without a preview is not added', () => {
  assert.equal(
    refusal(hit(1, false)),
    '“Billie Jean” by Michael Jackson (Thriller) has no preview on Deezer, so it cannot be played. Nothing was added. Choose another release.',
  )
  assert.equal(
    refusal(hit(1, false, false, null, '')),
    '“Billie Jean” by Michael Jackson has no preview on Deezer, so it cannot be played. Nothing was added. Choose another release.',
  )
})

test('refusal points at the pool for a song that is in it and lost its preview', () => {
  assert.equal(
    refusal(hit(4603408, false, true, ['pop'])),
    '“Billie Jean” by Michael Jackson (Thriller) has no preview on Deezer, so it cannot be played. It is in the pool already; find it below to remove it.',
  )
})

test('savedText tells a new song from a retag and from a save that changed nothing', () => {
  const row = { ...SEED[3], genres: ['pop', 'rock'] as Genre[] }
  assert.equal(savedText(row, null), 'Added “Billie Jean” by Michael Jackson to the pool: General, Pop and Rock.')
  assert.equal(
    savedText(row, ['pop']),
    '“Billie Jean” by Michael Jackson was in the pool already; its genres are now: General, Pop and Rock.',
  )
  assert.equal(
    savedText(row, ['rock', 'pop']),
    '“Billie Jean” by Michael Jackson was in the pool already; its genres are unchanged: General, Pop and Rock.',
  )
  assert.equal(savedText(song(1, 'A', 'B'), null), 'Added “A” by B to the pool: General only.')
})

// --- the clock and the picks ----------------------------------------------------

test('offsetText words the offset', () => {
  assert.equal(offsetText(0), 'None')
  assert.equal(offsetText(1), '1 day ahead')
  assert.equal(offsetText(12), '12 days ahead')
  assert.equal(offsetText(-1), '1 day behind')
  assert.equal(offsetText(-30), '30 days behind')
})

test('dayText is the day in one line', () => {
  assert.equal(dayText(STATE), 'Day 1, 2026-10-01')
})

test('a picked section has no problem to report', () => {
  assert.equal(pickProblem(STATE.sections[1]), '')
  assert.equal(pickDetail(STATE.sections[1]), '')
})

test('a section without a song says so, and what to do', () => {
  const entry: SectionPick = { section: 'rock', status: 'none', pick: null }
  assert.equal(pickProblem(entry), 'No song today')
  assert.match(pickDetail(entry), /Add a song/)
})

test('an unavailable section names the pick that stands, if there is one', () => {
  const waiting: SectionPick = { section: 'pop', status: 'unavailable', pick: null }
  assert.equal(pickProblem(waiting), 'Deezer did not answer')
  assert.match(pickDetail(waiting), /No song could be picked yet/)
  const standing: SectionPick = { ...STATE.sections[1], status: 'unavailable' }
  assert.equal(
    pickDetail(standing),
    'The pick stands, but its song could not be loaded: “Toxic” by Britney Spears, track 15391618.',
  )
  const gone: SectionPick = { section: 'pop', status: 'unavailable', pick: { trackId: 7, title: '', artist: '' } }
  assert.match(pickDetail(gone), /a song that is no longer in the pool, track 7/)
})

test('sectionsPlaying finds where a track is today’s song', () => {
  assert.deepEqual(sectionsPlaying(STATE, 15391618), ['pop'])
  assert.deepEqual(sectionsPlaying(STATE, 4603408), [])
  assert.deepEqual(sectionsPlaying(null, 15391618), [])
})

test('removalText says what goes, and that a standing pick stays', () => {
  const quiet = removalText(SEED[3], [])
  assert.equal(quiet.length, 1)
  assert.match(quiet[0], /^Track 4603408, from Billie Jean \(album\), leaves the pool/)
  const playing = removalText(SEED[1], ['pop'])
  assert.equal(playing.length, 2)
  assert.match(playing[1], /^It is today’s song in Pop\./)
  assert.match(removalText({ ...SEED[1], album: '' }, [])[0], /^Track 15391618, leaves the pool/)
})

test('rerollQuestion names the section and what is deleted', () => {
  const one = rerollQuestion('hip-hop')
  assert.equal(one.title, 'Re-roll Hip-hop?')
  assert.equal(one.confirm, 'Re-roll Hip-hop')
  assert.match(one.body, /Every player’s game in Hip-hop today is deleted\./)
  const all = rerollQuestion(null)
  assert.equal(all.title, 'Re-roll all four sections?')
  assert.equal(all.confirm, 'Re-roll all')
  assert.match(all.body, /in all four sections/)
})

test('rerollNews names the new song of a section', () => {
  const after = withEntry(STATE, picked('pop', 4603408, 'Billie Jean', 'Michael Jackson'))
  assert.equal(rerollNews('pop', STATE, after), 'Pop now plays “Billie Jean” by Michael Jackson.')
  // Without a state from before there is nothing to compare with.
  assert.equal(rerollNews('pop', null, after), 'Pop now plays “Billie Jean” by Michael Jackson.')
})

test('rerollNews says when the same song came back', () => {
  assert.equal(
    rerollNews('pop', STATE, STATE),
    'Pop drew “Toxic” by Britney Spears again: its pool has no other song for today.',
  )
})

test('rerollNews says when the section is left without a song or without an answer', () => {
  const none = withEntry(STATE, { section: 'rock', status: 'none', pick: null })
  assert.equal(rerollNews('rock', STATE, none), 'Rock was re-rolled and has no song today.')
  const waiting = withEntry(STATE, { section: 'rock', status: 'unavailable', pick: null })
  assert.equal(rerollNews('rock', STATE, waiting), 'Rock was re-rolled, but Deezer did not answer: its song is not picked yet.')
})

test('rerollNews for all four counts the sections Deezer did not answer for', () => {
  assert.equal(rerollNews(null, STATE, STATE), 'All four sections were re-rolled.')
  const one = withEntry(STATE, { section: 'rock', status: 'unavailable', pick: null })
  assert.equal(rerollNews(null, STATE, one), 'All four sections were re-rolled, but Deezer did not answer for one of them.')
  const two = withEntry(one, { section: 'pop', status: 'unavailable', pick: null })
  assert.equal(rerollNews(null, STATE, two), 'All four sections were re-rolled, but Deezer did not answer for 2 of them.')
})

test('clockNews says where the clock is now', () => {
  const next = { ...STATE, day: '2026-10-02', number: 2, offset: 1 }
  assert.equal(clockNews('next-day', next), 'Now on day 2, 2026-10-02.')
  assert.equal(
    clockNews('reset', STATE),
    'Back on day 1, 2026-10-01. Every game and every pick was deleted; the pool is as it was.',
  )
})
