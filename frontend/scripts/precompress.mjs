// Precompress built SPA assets so the Rust web layer can serve them with
// `ServeDir::precompressed_br()/precompressed_gzip()` instead of re-running
// Brotli over the same immutable bundle on every request (the tower-http
// CompressionLayer stays as the on-the-fly fallback for anything without a
// sibling). Runs as the `build` script's final step; uses only node:zlib —
// no dependencies. See benches/STATIC-PRECOMPRESSED.md for the measured win.
import { promises as fs } from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';

const OUT_DIR = process.argv[2] ?? '../static-dist';
// Compressible text assets; media formats are already compressed.
const EXTENSIONS = new Set([
	'.js',
	'.mjs',
	'.css',
	'.html',
	'.svg',
	'.json',
	'.txt',
	'.xml',
	'.map',
	'.webmanifest'
]);
// Below this size the encoding overhead outweighs the transfer win
// (mirrors the server's SizeAbove(256) predicate).
const MIN_BYTES = 256;

async function* walk(dir) {
	for (const entry of await fs.readdir(dir, { withFileTypes: true })) {
		const p = path.join(dir, entry.name);
		if (entry.isDirectory()) yield* walk(p);
		else yield p;
	}
}

let files = 0;
let inBytes = 0;
let brBytes = 0;
for await (const file of walk(OUT_DIR)) {
	if (!EXTENSIONS.has(path.extname(file))) continue;
	const data = await fs.readFile(file);
	if (data.length < MIN_BYTES) continue;
	const br = zlib.brotliCompressSync(data, {
		params: {
			[zlib.constants.BROTLI_PARAM_QUALITY]: 11,
			[zlib.constants.BROTLI_PARAM_SIZE_HINT]: data.length
		}
	});
	const gz = zlib.gzipSync(data, { level: 9 });
	// Only keep siblings that actually shrink the asset.
	if (br.length < data.length) await fs.writeFile(`${file}.br`, br);
	if (gz.length < data.length) await fs.writeFile(`${file}.gz`, gz);
	files += 1;
	inBytes += data.length;
	brBytes += Math.min(br.length, data.length);
}
console.log(
	`precompress: ${files} assets, ${(inBytes / 1024).toFixed(0)} KiB → ${(brBytes / 1024).toFixed(0)} KiB brotli (${inBytes ? ((1 - brBytes / inBytes) * 100).toFixed(0) : 0}% smaller)`
);
