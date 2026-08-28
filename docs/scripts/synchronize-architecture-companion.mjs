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
const source = readFileSync(sourcePath, 'utf8');
const existing = readFileSync(outputPath, 'utf8');
const sourceStart = existing.indexOf('<!-- markdownlint-disable MD013 -->');
const supplementStart = existing.indexOf('\n\n<section class="supplement"');
if (sourceStart < 0 || supplementStart <= sourceStart) {
  throw new Error('annotated companion source/supplement markers are inconsistent');
}

const cards = new Map();
for (const match of existing.matchAll(
  /<aside class="explain-card[^>]*data-source-heading="([^"]+)"[^>]*>[\s\S]*?<\/aside>/g,
)) {
  const values = cards.get(match[1]) || [];
  values.push(match[0]);
  cards.set(match[1], values);
}

if (!cards.has('Stage 8 canonical evidence-attempt and artifact contract')) {
  cards.set('Stage 8 canonical evidence-attempt and artifact contract', [
    `<aside class="explain-card level-3" data-source-heading="Stage 8 canonical evidence-attempt and artifact contract">
  <div class="explain-card-label">Subsection guide · plain-English companion</div>
  <p><strong>Plain-English significance:</strong> Stage 8 freezes the exact evidence rows written around model and tool attempts and the durable content-addressed byte store they may reference. It keeps selection, dispatch, provider translation, process execution, context assembly, and completion orchestration in their later owning stages.</p>
  <p class="real-life-example"><strong>Real-life example:</strong> Before a courier or machine operator acts, the filing room prepares an immutable job packet and numbered evidence envelope. Large attachments are sealed in a hardened archive first; only a verified seal may be entered in the ledger.</p>
</aside>`,
  ]);
}

const temporaryDirectory = mkdtempSync(join(tmpdir(), 'craxii-architecture-html-'));
const renderedPath = join(temporaryDirectory, 'architecture.html');
let rendered;
try {
  execFileSync(
    'npx',
    ['--yes', 'marked@18.0.11', '-i', sourcePath, '-o', renderedPath, '--gfm', '--no-breaks'],
    { env: { ...process.env, PATH: `/opt/homebrew/bin:/opt/homebrew/sbin:${process.env.PATH || ''}` } },
  );
  rendered = readFileSync(renderedPath, 'utf8').trim();
} finally {
  rmSync(temporaryDirectory, { recursive: true, force: true });
}

rendered = rendered.replace(/^<h1>[^<]+<\/h1>\s*/, '');

function decodeHeading(value) {
  return value
    .replace(/<[^>]+>/g, '')
    .replaceAll('&amp;', '&')
    .replaceAll('&#39;', "'")
    .replaceAll('&quot;', '"')
    .replaceAll('&lt;', '<')
    .replaceAll('&gt;', '>')
    .trim();
}

let insertedCards = 0;
rendered = rendered.replace(/<(h[234])([^>]*)>([\s\S]*?)<\/\1>/g, (heading, tag, attributes, body) => {
  const key = decodeHeading(body);
  const queue = cards.get(key);
  if (!queue || queue.length === 0) {
    return heading;
  }
  insertedCards += 1;
  return `${heading}\n${queue.shift()}`;
});

const unmatched = [...cards.entries()].flatMap(([heading, values]) =>
  values.map(() => heading),
);
if (unmatched.length !== 0) {
  throw new Error(`annotated explanations no longer match headings: ${unmatched.join(', ')}`);
}

const majorSections = (rendered.match(/<h2(?:\s|>)/g) || []).length;
const detailedSections = (rendered.match(/<h[34](?:\s|>)/g) || []).length;
const sha256 = createHash('sha256').update(source).digest('hex');
let html = `${existing.slice(0, sourceStart)}${rendered}${existing.slice(supplementStart)}`;
html = html
  .replace(/(<meta name="craxii-source-sha256" content=")[0-9a-f]{64}(">)/, `$1${sha256}$2`)
  .replace(
    /<p><strong>\d+<\/strong> source sections carry companion explanations\.<\/p>/,
    `<p><strong>${insertedCards}</strong> source sections carry companion explanations.</p>`,
  )
  .replace(
    /Source SHA-256:<br><code>[0-9a-f]{16}…<\/code>/,
    `Source SHA-256:<br><code>${sha256.slice(0, 16)}…</code>`,
  )
  .replace(
    /<p class="companion-stats"><strong>Coverage:<\/strong>[\s\S]*?<\/p>/,
    `<p class="companion-stats"><strong>Coverage:</strong> one complete authoritative source, ${majorSections} major sections, ${detailedSections} detailed subsections, and ${insertedCards} nearby plain-English explanations.</p>`,
  )
  .replace(
    /<strong>Generated companion:<\/strong> [^·]+· Source:/,
    `<strong>Generated companion:</strong> ${new Date().toISOString()} · Source:`,
  )
  .replace(/SHA-256 <code>[0-9a-f]{64}<\/code>/, `SHA-256 <code>${sha256}</code>`);

writeFileSync(outputPath, html, 'utf8');
