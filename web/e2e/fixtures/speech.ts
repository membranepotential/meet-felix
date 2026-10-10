/**
 * The real speech in `web/e2e/audio` (see the README there), turned into what an SFU WebSocket
 * adapter delivers: 48 kHz stereo signed 16-bit little-endian PCM.
 */
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { AUDIO_DIR } from './paths.ts';

/** One entry of `snippets.json`. */
export interface Snippet {
	file: string;
	/** The device it was recorded on. */
	speaker: string;
	language: string;
	/** The words said. */
	text: string;
	/** Lower-case words a transcript of the snippet must contain. */
	keywords: string[];
}

export const SNIPPETS = JSON.parse(
	readFileSync(join(AUDIO_DIR, 'snippets.json'), 'utf8')
) as readonly Snippet[];

const RATE = 48_000;

/** The samples of a WAV file holding 48 kHz mono 16-bit PCM, as every snippet does. */
export function monoSamples(wav: Buffer): Buffer {
	if (wav.toString('ascii', 0, 4) !== 'RIFF' || wav.toString('ascii', 8, 12) !== 'WAVE') {
		throw new Error('not a WAV file');
	}
	let pcm16Mono48k = false;
	// RIFF chunks: a four-letter id, a 32-bit size, the body, padded to an even length.
	for (let at = 12; at + 8 <= wav.length;) {
		const id = wav.toString('ascii', at, at + 4);
		const size = wav.readUInt32LE(at + 4);
		const body = wav.subarray(at + 8, at + 8 + size);
		if (id === 'fmt ') {
			pcm16Mono48k =
				body.readUInt16LE(0) === 1 &&
				body.readUInt16LE(2) === 1 &&
				body.readUInt32LE(4) === RATE &&
				body.readUInt16LE(14) === 16;
		} else if (id === 'data') {
			if (!pcm16Mono48k) throw new Error('the WAV file is not 48 kHz mono 16-bit PCM');
			return body;
		}
		at += 8 + size + (size % 2);
	}
	throw new Error('the WAV file has no data chunk');
}

/** Every snippet in order, each followed by a second of silence, as stereo PCM. */
export function speechPcm(): Buffer {
	const silence = Buffer.alloc(RATE * 2);
	const mono = Buffer.concat(
		SNIPPETS.flatMap((s) => [monoSamples(readFileSync(join(AUDIO_DIR, s.file))), silence])
	);
	const stereo = Buffer.alloc(mono.length * 2);
	for (let i = 0; i + 1 < mono.length; i += 2) {
		mono.copy(stereo, i * 2, i, i + 2);
		mono.copy(stereo, i * 2 + 2, i, i + 2);
	}
	return stereo;
}
