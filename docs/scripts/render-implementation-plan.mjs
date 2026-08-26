#!/usr/bin/env node
import { readFileSync, writeFileSync, mkdtempSync, rmSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { tmpdir } from 'node:os';

const __dirname = dirname(fileURLToPath(import.meta.url));
const docsDir = join(__dirname, '..');
const sourcePath = join(docsDir, 'craxii-v0.0.01-implementation-plan.md');
const outputPath = join(docsDir, 'craxii-v0.0.01-implementation-plan.html');

const source = readFileSync(sourcePath, 'utf8');
const sha256 = createHash('sha256').update(source).digest('hex');
const generatedAt = new Date().toISOString();

const tempDir = mkdtempSync(join(tmpdir(), 'craxii-plan-html-'));
const bodyPath = join(tempDir, 'body.html');
let bodyHtml;

try {
  execFileSync(
    'npx',
    ['--yes', 'marked', '-i', sourcePath, '-o', bodyPath, '--gfm', '--no-breaks'],
    { env: { ...process.env, PATH: `/opt/homebrew/bin:/opt/homebrew/sbin:${process.env.PATH || ''}` } },
  );
  bodyHtml = readFileSync(bodyPath, 'utf8');
} finally {
  rmSync(tempDir, { recursive: true, force: true });
}

const html = `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta name="description" content="Readable HTML rendering of the Craxii V0.0.01 implementation plan.">
  <meta name="craxii-source-sha256" content="${sha256}">
  <title>Craxii V0.0.01 implementation plan</title>
  <style>
:root {
  color-scheme: light;
  --bg: #f3f4f6;
  --paper: #ffffff;
  --text: #17202b;
  --muted: #56606d;
  --line: #d6d9de;
  --blue: #1f4f82;
  --blue-soft: #f3f6f9;
  --code-bg: #f4f5f7;
  --code-text: #17202b;
  --sidebar: 280px;
}

* { box-sizing: border-box; }
html { scroll-behavior: smooth; }
body {
  margin: 0;
  background: var(--bg);
  color: var(--text);
  font: 15.5px/1.52 -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif;
  -webkit-font-smoothing: antialiased;
}
a { color: var(--blue); text-underline-offset: .18em; }
a:hover { text-decoration-thickness: 2px; }

#reading-progress {
  position: fixed; z-index: 100; inset: 0 auto auto 0; height: 2px; width: 0;
  background: var(--blue);
}

.site-header {
  position: sticky; top: 0; z-index: 40; height: 48px;
  display: flex; align-items: center; justify-content: space-between; gap: 16px;
  padding: 0 16px; border-bottom: 1px solid var(--line);
  background: rgba(255,255,255,.96);
}
.brand { min-width: 0; display: flex; align-items: baseline; gap: 10px; }
.brand strong { font-size: 14px; letter-spacing: .01em; white-space: nowrap; }
.brand span { color: var(--muted); font-size: 12px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.header-actions button, .sidebar button {
  border: 1px solid var(--line); background: var(--paper); color: var(--text); border-radius: 5px;
  padding: 6px 9px; cursor: pointer; font: 600 12px/1.2 inherit;
}
.header-actions button:hover, .sidebar button:hover { border-color: var(--blue); }

.layout { display: grid; grid-template-columns: var(--sidebar) minmax(0, 1fr); align-items: start; }
.sidebar {
  position: sticky; top: 48px; height: calc(100vh - 48px); overflow: auto;
  padding: 15px 10px 28px 14px; border-right: 1px solid var(--line); background: var(--paper);
}
.sidebar h2 { margin: 0 0 7px; font-size: 11px; text-transform: uppercase; letter-spacing: .12em; color: var(--muted); }
.toc-search {
  width: 100%; border: 1px solid var(--line); border-radius: 5px; padding: 7px 8px;
  background: var(--bg); color: var(--text); font: inherit; font-size: 12px;
}
#toc { margin-top: 8px; }
#toc a {
  display: block; padding: 3px 6px; border-radius: 4px; color: var(--muted);
  text-decoration: none; font-size: 11.5px; line-height: 1.3;
}
#toc a.level-3 { padding-left: 16px; font-size: 10.75px; }
#toc a.level-4 { padding-left: 26px; font-size: 10.25px; }
#toc a:hover, #toc a.active { color: var(--blue); background: var(--blue-soft); }
#toc a.hidden { display: none; }
.sidebar-meta {
  margin: 12px 6px 0; padding-top: 10px; border-top: 1px solid var(--line);
  color: var(--muted); font-size: 10.5px;
}
.sidebar-meta code { font-size: 9.5px; word-break: break-all; }

main {
  width: min(1240px, calc(100% - 28px));
  margin: 18px auto 54px;
  padding: 36px 46px 58px;
  background: var(--paper);
  border: 1px solid var(--line);
  border-radius: 7px;
}
main > h1 {
  margin: 0 0 12px;
  font-size: clamp(2.2rem, 4vw, 3.3rem);
  line-height: 1.05;
  letter-spacing: -.03em;
}
h2, h3, h4 { scroll-margin-top: 60px; }
main h2 {
  margin: 2.5rem 0 .65rem;
  padding-top: .65rem;
  border-top: 1px solid var(--line);
  font-size: clamp(1.45rem, 2.2vw, 1.85rem);
  line-height: 1.2;
}
main h3 {
  margin: 1.7rem 0 .5rem;
  font-size: clamp(1.12rem, 1.7vw, 1.3rem);
  line-height: 1.28;
}
main h4 { margin: 1rem 0 .35rem; font-size: 1.02rem; line-height: 1.3; }
p { margin: .65em 0; max-width: 88ch; }
ul, ol { margin: .5em 0 .8em; padding-left: 1.45em; max-width: 88ch; }
li { margin: .12em 0; }
strong { color: color-mix(in srgb, var(--text) 90%, var(--blue)); }
blockquote {
  margin: .85rem 0; padding: .45rem .8rem; border-left: 3px solid var(--blue);
  background: #f5f6f7; max-width: 88ch;
}
hr { border: 0; border-top: 1px solid var(--line); margin: 1.5rem 0; }

code {
  padding: .12em .34em; border-radius: 3px; background: #eef0f2;
  font: .88em/1.4 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
}
pre {
  overflow: auto; margin: .75rem 0 1rem; padding: 11px 13px;
  border: 1px solid var(--line); border-radius: 4px;
  background: var(--code-bg); color: var(--code-text);
}
pre code { padding: 0; background: transparent; color: inherit; font-size: 12px; line-height: 1.4; }

table {
  display: block; width: 100%; overflow-x: auto; border-collapse: collapse;
  margin: .85rem 0 1.1rem; font-size: 12.5px;
}
th, td { min-width: 105px; padding: 6px 8px; border: 1px solid var(--line); text-align: left; vertical-align: top; }
th { background: #f0f2f4; font-weight: 750; }
tbody tr:nth-child(even) { background: color-mix(in srgb, var(--bg) 65%, transparent); }

.source-footer {
  margin: 30px 0 0; padding-top: 12px; border-top: 1px solid var(--line);
  color: var(--muted); font-size: 12px;
}
.source-footer p { max-width: none; }

@media (max-width: 1080px) {
  .layout { grid-template-columns: 1fr; }
  .sidebar { display: none; }
  main { width: min(100% - 18px, 1240px); padding: 28px 30px 45px; }
}
@media (max-width: 760px) {
  body { font-size: 15px; }
  .site-header { padding: 0 12px; }
  .brand span { display: none; }
  main { width: 100%; padding: 24px 14px 42px; border-radius: 0; border-left: 0; border-right: 0; }
  main > h1 { font-size: 2.2rem; }
}
@media print {
  :root { --bg: #fff; --paper: #fff; --text: #000; --muted: #333; --line: #aaa; }
  .site-header, .sidebar, #reading-progress { display: none !important; }
  .layout { display: block; }
  main { width: 100%; margin: 0; padding: 0; border: 0; }
  table, pre { break-inside: avoid; }
  a { color: inherit; }
}
  </style>
</head>
<body>
  <div id="reading-progress" aria-hidden="true"></div>
  <header class="site-header">
    <div class="brand">
      <strong>Craxii V0.0.01</strong>
      <span>Implementation plan</span>
    </div>
    <div class="header-actions">
      <button id="print-button" type="button">Print</button>
    </div>
  </header>
  <div class="layout">
    <aside class="sidebar" aria-label="Document navigation">
      <h2>Contents</h2>
      <input id="toc-search" class="toc-search" type="search" placeholder="Filter sections…" aria-label="Filter table of contents">
      <nav id="toc"></nav>
      <div class="sidebar-meta">
        <p>Generated ${generatedAt}</p>
        <p>Source SHA-256:<br><code>${sha256.slice(0, 16)}…</code></p>
        <p><a href="craxii-v0.0.01-implementation-plan.md">Open source Markdown</a></p>
      </div>
    </aside>
    <main id="content">
${bodyHtml}
      <footer class="source-footer">
        <p><strong>Generated view:</strong> ${generatedAt} · Source: <a href="craxii-v0.0.01-implementation-plan.md"><code>craxii-v0.0.01-implementation-plan.md</code></a> · SHA-256 <code>${sha256}</code>.</p>
        <p>The Markdown source remains authoritative. This standalone HTML has no external stylesheet, font, script, or network dependency.</p>
      </footer>
    </main>
  </div>
  <script>
(() => {
  document.getElementById('print-button').addEventListener('click', () => window.print());

  const used = new Map();
  const slug = (text) => {
    let base = text.toLowerCase().replace(/[\`'""]/g, '').replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '') || 'section';
    const count = used.get(base) || 0;
    used.set(base, count + 1);
    return count ? \`\${base}-\${count + 1}\` : base;
  };

  const headings = [...document.querySelectorAll('main h2, main h3, main h4')];
  const toc = document.getElementById('toc');
  headings.forEach((heading) => {
    if (!heading.id) heading.id = slug(heading.textContent.trim());
    const link = document.createElement('a');
    link.href = \`#\${heading.id}\`;
    link.className = \`level-\${heading.tagName.slice(1)}\`;
    link.textContent = heading.textContent.trim();
    link.dataset.target = heading.id;
    toc.appendChild(link);
  });

  const links = [...toc.querySelectorAll('a')];
  const observer = new IntersectionObserver((entries) => {
    entries.filter(e => e.isIntersecting).forEach((entry) => {
      links.forEach(link => link.classList.toggle('active', link.dataset.target === entry.target.id));
    });
  }, { rootMargin: '-15% 0px -78% 0px', threshold: 0 });
  headings.forEach(h => observer.observe(h));

  document.getElementById('toc-search').addEventListener('input', (event) => {
    const query = event.target.value.trim().toLowerCase();
    links.forEach(link => link.classList.toggle('hidden', query && !link.textContent.toLowerCase().includes(query)));
  });

  const progress = document.getElementById('reading-progress');
  const updateProgress = () => {
    const max = document.documentElement.scrollHeight - innerHeight;
    progress.style.width = \`\${max > 0 ? (scrollY / max) * 100 : 0}%\`;
  };
  addEventListener('scroll', updateProgress, { passive: true });
  addEventListener('resize', updateProgress);
  updateProgress();
})();
  </script>
</body>
</html>`;

writeFileSync(outputPath, html, 'utf8');
console.log(`Wrote ${outputPath}`);
console.log(`Source SHA-256: ${sha256}`);
