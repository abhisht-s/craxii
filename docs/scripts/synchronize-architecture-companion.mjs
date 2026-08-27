#!/usr/bin/env node
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptsDir = dirname(fileURLToPath(import.meta.url));
const docsDir = join(scriptsDir, '..');
const sourcePath = join(docsDir, 'craxii-v0.0.01-architecture.md');
const outputPath = join(docsDir, 'craxii-v0.0.01-architecture-annotated.html');
const heading = '### Stage 7 canonical journal and bootstrap contract';
const nextHeading = '### `craxii_principals`';

const source = readFileSync(sourcePath, 'utf8');
const start = source.indexOf(heading);
const end = source.indexOf(nextHeading, start);
if (start < 0 || end < 0) {
  throw new Error('Stage 7 architecture section markers are missing');
}

const temporaryDirectory = mkdtempSync(join(tmpdir(), 'craxii-architecture-html-'));
const fragmentSource = join(temporaryDirectory, 'stage-7.md');
const fragmentOutput = join(temporaryDirectory, 'stage-7.html');
let fragment;
try {
  writeFileSync(fragmentSource, source.slice(start, end), 'utf8');
  execFileSync(
    'npx',
    ['--yes', 'marked@18.0.11', '-i', fragmentSource, '-o', fragmentOutput, '--gfm', '--no-breaks'],
    { env: { ...process.env, PATH: `/opt/homebrew/bin:/opt/homebrew/sbin:${process.env.PATH || ''}` } },
  );
  fragment = readFileSync(fragmentOutput, 'utf8').trim();
} finally {
  rmSync(temporaryDirectory, { recursive: true, force: true });
}

fragment = fragment.replace(
  '</h3>',
  `</h3>\n<aside class="explain-card level-3" data-source-heading="Stage 7 canonical journal and bootstrap contract">
  <div class="explain-card-label">Subsection guide · plain-English companion</div>
  <p><strong>Plain-English significance:</strong> Stage 7 adds the append-only ledger, deterministic checker, and one-time root identity transaction while keeping later attempt, command, runtime, and client protocol behavior out of scope.</p>
  <p class="real-life-example"><strong>Real-life example:</strong> The filing room now receives a numbered, tamper-evident ledger and its first two linked entries. Reopening checks the ledger against the identity cards instead of quietly rewriting either one.</p>
</aside>`,
);

let html = readFileSync(outputPath, 'utf8');
const renderedHeading = '<h3>Stage 7 canonical journal and bootstrap contract</h3>';
const insertionPoint = '<h3><code>craxii_principals</code></h3>';
const existingStart = html.indexOf(renderedHeading);
const existingEnd = html.indexOf(insertionPoint);
if (existingEnd < 0 || (existingStart >= 0 && existingStart >= existingEnd)) {
  throw new Error('annotated companion insertion markers are inconsistent');
}
if (existingStart >= 0) {
  html = `${html.slice(0, existingStart)}${fragment}\n${html.slice(existingEnd)}`;
} else {
  html = html.replace(insertionPoint, `${fragment}\n${insertionPoint}`);
}

const sha256 = createHash('sha256').update(source).digest('hex');
html = html
  .replace(/(<meta name="craxii-source-sha256" content=")[0-9a-f]{64}(">)/, `$1${sha256}$2`)
  .replace(/<strong>333<\/strong> source sections/, '<strong>334</strong> source sections')
  .replace(/and 334 nearby plain-English explanations/, 'and 335 nearby plain-English explanations')
  .replace(/Source SHA-256:<br><code>[0-9a-f]{16}…<\/code>/, `Source SHA-256:<br><code>${sha256.slice(0, 16)}…</code>`)
  .replace(
    /<strong>Generated companion:<\/strong> [^·]+· Source:/,
    `<strong>Generated companion:</strong> ${new Date().toISOString()} · Source:`,
  )
  .replace(/SHA-256 <code>[0-9a-f]{64}<\/code>/, `SHA-256 <code>${sha256}</code>`);

writeFileSync(outputPath, html, 'utf8');
