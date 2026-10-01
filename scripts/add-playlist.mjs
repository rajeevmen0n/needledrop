#!/usr/bin/env node
// Adds the playable tracks of Deezer playlists to the song pool, through the
// admin API of a running server.
//
//   node scripts/add-playlist.mjs <playlist id or URL>… --genre pop [--genre rock]
//
// Options:
//   --genre <slug>   pop, rock or hip-hop; may be repeated. Without one the
//                    songs go into the General pool only.
//   --api <origin>   the server (default http://127.0.0.1:4810, or ND_API).
//   --min-rank <n>   leave out tracks whose Deezer rank is below n.
//   --dry-run        say what would happen and change nothing.
//
// The add route does not check that a track has a preview, so this does:
// a track that is not `readable` or has no `preview` is left out. The route
// answers 200 for a track that is in the pool already and replaces its
// genres, so duplicates are settled here too: a song that is in the pool
// (the same track, or another release of the same title and artist) is not
// added again, and only gains the genres it lacks.
//
// Deezer allows 50 requests per 5 seconds for this address, and the server
// keeps its own budget of 40, shared with the players. Each new song costs
// the server one request, so adds are made one at a time, 400 ms apart (12
// per 5 seconds), and this script's own Deezer requests 500 ms apart. When
// either side says the limit is reached (Deezer's quota error, the server's
// 502 `upstream`, a proxy's 429) the request waits 6 seconds and is made
// again, up to three times.

const GENRES = ['pop', 'rock', 'hip-hop'];
const DEEZER = 'https://api.deezer.com';
const ADD_PAUSE_MS = 400;
const DEEZER_PAUSE_MS = 500;
const RETRY_PAUSE_MS = 6000;
const RETRIES = 3;

const options = parseArgs(process.argv.slice(2));
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function usage(problem) {
	if (problem) console.error(`${problem}\n`);
	console.error(
		'Usage: node scripts/add-playlist.mjs <playlist id or URL>… [--genre pop|rock|hip-hop]… [--api origin] [--min-rank n] [--dry-run]',
	);
	process.exit(2);
}

function parseArgs(args) {
	const parsed = {
		playlists: [],
		genres: [],
		api: process.env.ND_API || 'http://127.0.0.1:4810',
		minRank: 0,
		dryRun: false,
	};
	for (let i = 0; i < args.length; i++) {
		const arg = args[i];
		const value = () => args[++i] ?? usage(`${arg} needs a value.`);
		if (arg === '--genre') {
			const genre = value();
			if (!GENRES.includes(genre)) usage(`"${genre}" is not one of ${GENRES.join(', ')}.`);
			if (!parsed.genres.includes(genre)) parsed.genres.push(genre);
		} else if (arg === '--api') {
			parsed.api = value().replace(/\/+$/, '');
		} else if (arg === '--min-rank') {
			parsed.minRank = Number(value());
			if (!Number.isFinite(parsed.minRank)) usage('--min-rank needs a number.');
		} else if (arg === '--dry-run') {
			parsed.dryRun = true;
		} else if (arg.startsWith('--')) {
			usage(`Unknown option ${arg}.`);
		} else {
			const id = /(?:^|playlist\/)(\d+)/.exec(arg)?.[1];
			if (!id) usage(`"${arg}" is not a playlist ID or a deezer.com playlist address.`);
			parsed.playlists.push(id);
		}
	}
	if (parsed.playlists.length === 0) usage('Name at least one playlist.');
	return parsed;
}

// Close to the server's `normalize_title` / `normalize_artist`: the same song
// in another release (a remaster, a best-of) gets the same key.
function fold(text) {
	return text.normalize('NFKD').replace(/[̀-ͯ]/g, '').toLowerCase();
}

function squash(text) {
	return text.replace(/[^\p{L}\p{N}]+/gu, '');
}

function songKey(title, artist) {
	const plain = fold(title)
		.replace(/\([^)]*\)|\[[^\]]*\]/g, ' ')
		.replace(/\s(feat\.?|ft\.?|featuring)\s.*$/, '')
		.replace(/\s-\s[^-]*(remaster|version|edit|mix|mono|stereo|live|single|deluxe|bonus)[^-]*$/, '');
	return `${squash(plain) || title.trim().toLowerCase()}|${squash(fold(artist).replace(/&/g, 'and'))}`;
}

// Deezer's "Quota limit exceeded", sent with status 200.
const DEEZER_QUOTA = 4;

async function deezer(path) {
	for (let attempt = 1; ; attempt++) {
		const response = await fetch(`${DEEZER}${path}`, {
			headers: { 'user-agent': 'needledrop-add-playlist' },
			signal: AbortSignal.timeout(15000),
		});
		const body = response.ok ? await response.json() : null;
		const limited = response.status === 429 || body?.error?.code === DEEZER_QUOTA;
		if (limited && attempt <= RETRIES) {
			await pause(RETRY_PAUSE_MS);
			continue;
		}
		if (!body) throw new Error(`Deezer answered ${response.status} for ${path}`);
		if (body.error) throw new Error(`Deezer: ${body.error.message ?? JSON.stringify(body.error)} (${path})`);
		return body;
	}
}

async function admin(method, path, body) {
	const response = await fetch(`${options.api}${path}`, {
		method,
		headers: body ? { 'content-type': 'application/json' } : {},
		body: body ? JSON.stringify(body) : undefined,
		signal: AbortSignal.timeout(30000),
	});
	const answer = await response.json().catch(() => null);
	return { status: response.status, body: answer };
}

/** The whole pool, a page at a time: by track ID, and by song key. */
async function loadPool() {
	const byId = new Map();
	for (let offset = 0; ; offset += 100) {
		const { status, body } = await admin('GET', `/api/admin/songs?limit=100&offset=${offset}`);
		if (status !== 200) throw new Error(`The pool could not be read (${status} from ${options.api}).`);
		for (const song of body.songs) byId.set(song.trackId, song);
		if (offset + 100 >= body.total) break;
	}
	const byKey = new Map();
	for (const song of byId.values()) byKey.set(songKey(song.title, song.artist), song);
	return { byId, byKey };
}

async function playlistTracks(id) {
	const tracks = [];
	for (let index = 0; ; index += 100) {
		const page = await deezer(`/playlist/${id}/tracks?limit=100&index=${index}`);
		tracks.push(...page.data);
		if (page.data.length === 0 || index + 100 >= page.total) break;
		await pause(DEEZER_PAUSE_MS);
	}
	return tracks;
}

/** Adds a song or replaces its genres; waits and tries again at a rate limit. */
async function save(trackId, genres) {
	for (let attempt = 1; ; attempt++) {
		const answer = await admin('POST', '/api/admin/songs', { trackId, genres });
		const limited = answer.status === 502 || answer.status === 429;
		if (!limited || attempt > RETRIES) return answer;
		await pause(RETRY_PAUSE_MS);
	}
}

function counts(pool) {
	const tally = { all: pool.byId.size, pop: 0, rock: 0, 'hip-hop': 0 };
	for (const song of pool.byId.values()) for (const genre of song.genres) tally[genre]++;
	return `${tally.all} songs: ${GENRES.map((genre) => `${tally[genre]} ${genre}`).join(', ')}`;
}

async function addPlaylist(id, pool) {
	const { title } = await deezer(`/playlist/${id}?limit=1`).catch(() => ({ title: '' }));
	await pause(DEEZER_PAUSE_MS);
	const tracks = await playlistTracks(id);
	console.log(`\nPlaylist ${id}${title ? ` "${title}"` : ''}: ${tracks.length} tracks`);

	const result = { added: 0, retagged: 0, inPool: 0, noPreview: 0, lowRank: 0, repeated: 0, failed: 0 };
	const seen = new Set();
	for (const track of tracks) {
		const name = `${track.title} — ${track.artist?.name ?? '?'} (${track.id})`;
		const key = songKey(track.title_short || track.title, track.artist?.name ?? '');
		if (seen.has(key)) {
			result.repeated++;
			continue;
		}
		seen.add(key);
		if (!track.readable || !track.preview) {
			result.noPreview++;
			console.log(`  no preview   ${name}`);
			continue;
		}
		if ((track.rank ?? 0) < options.minRank) {
			result.lowRank++;
			continue;
		}

		const existing = pool.byId.get(track.id) ?? pool.byKey.get(key);
		const missing = options.genres.filter((genre) => !existing?.genres.includes(genre));
		if (existing && missing.length === 0) {
			result.inPool++;
			continue;
		}
		// The route replaces the genres, so a song in the pool is sent the ones
		// it has together with the new ones.
		const trackId = existing?.trackId ?? track.id;
		const genres = GENRES.filter((genre) => existing?.genres.includes(genre) || missing.includes(genre));
		const what = existing ? 'retagged' : 'added';
		if (options.dryRun) {
			result[what]++;
			console.log(`  would be ${what}  ${name}`);
			const song = existing ?? { trackId, title: track.title, artist: track.artist?.name ?? '' };
			pool.byId.set(trackId, { ...song, genres });
			pool.byKey.set(key, pool.byId.get(trackId));
			continue;
		}

		const answer = await save(trackId, genres);
		if (answer.status === 200) {
			result[what]++;
			pool.byId.set(answer.body.trackId, answer.body);
			pool.byKey.set(key, answer.body);
			pool.byKey.set(songKey(answer.body.title, answer.body.artist), answer.body);
			console.log(`  ${what.padEnd(12)} ${name}${existing ? ` → ${genres.join(', ')}` : ''}`);
		} else {
			result.failed++;
			console.log(`  FAILED ${answer.status} ${answer.body?.error ?? ''}  ${name}`);
		}
		// Only a new song costs the server a Deezer request.
		if (!existing) await pause(ADD_PAUSE_MS);
	}

	console.log(
		`  ${result.added} added, ${result.retagged} given a genre, ${result.inPool} already in the pool, ` +
			`${result.noPreview} without a preview, ${result.repeated} twice in the playlist` +
			(options.minRank ? `, ${result.lowRank} below rank ${options.minRank}` : '') +
			`, ${result.failed} failed`,
	);
	return result;
}

const pool = await loadPool();
console.log(`${options.api} — the pool before: ${counts(pool)}`);
console.log(`Genres: ${options.genres.join(', ') || 'none (General only)'}${options.dryRun ? ' — dry run, nothing is changed' : ''}`);

const total = { added: 0, retagged: 0, failed: 0 };
for (const id of options.playlists) {
	try {
		const result = await addPlaylist(id, pool);
		for (const field of Object.keys(total)) total[field] += result[field];
	} catch (err) {
		total.failed++;
		console.log(`\nPlaylist ${id} could not be read: ${err.message}`);
	}
	await pause(DEEZER_PAUSE_MS);
}

// Count again from the server, so the last line is what the pool is.
const after = options.dryRun ? pool : await loadPool();
console.log(`\n${total.added} songs ${options.dryRun ? 'would be ' : ''}added, ${total.retagged} given a genre, ${total.failed} failed.`);
console.log(`The pool ${options.dryRun ? 'would be' : 'now'}: ${counts(after)}`);
process.exit(total.failed > 0 ? 1 : 0);
