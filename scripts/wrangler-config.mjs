// Writes `wrangler.local.toml`: `wrangler.toml` with each `${NAME}` placeholder replaced by the
// value of NAME from `.env`. Both `.env` and the output are git-ignored, so the deployment's
// own values never reach the repository.
import { readFileSync, writeFileSync } from 'node:fs';
import { parseEnv } from 'node:util';

const root = new URL('../', import.meta.url);
const template = readFileSync(new URL('wrangler.toml', root), 'utf8');

let env;
try {
	env = parseEnv(readFileSync(new URL('.env', root), 'utf8'));
} catch (e) {
	if (e.code !== 'ENOENT') throw e;
	console.error('No .env: copy .env.example to .env and fill it in.');
	process.exit(1);
}

const missing = new Set();
// Placeholders only ever stand inside a TOML basic string, which allows no quote, backslash or
// control character.
const config = template.replace(/\$\{([A-Z_]+)\}/g, (placeholder, name) => {
	const value = env[name];
	if (value === undefined || value === '' || /["\\\u0000-\u001f\u007f]/.test(value)) {
		missing.add(name);
		return placeholder;
	}
	return value;
});
if (missing.size > 0) {
	console.error(`.env lacks a usable value for: ${[...missing].join(', ')}`);
	process.exit(1);
}
writeFileSync(new URL('wrangler.local.toml', root), config);
