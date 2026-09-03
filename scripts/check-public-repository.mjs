#!/usr/bin/env node

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, extname, join, normalize, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const repositoryRoot = resolve(scriptDirectory, '..');

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function read(path) {
  return readFileSync(join(repositoryRoot, path), 'utf8');
}

function run(command, arguments_) {
  const result = spawnSync(command, arguments_, {
    cwd: repositoryRoot,
    encoding: 'utf8',
  });
  if (result.error) throw new Error(`${command} could not start: ${result.error.message}`);
  if (result.status !== 0) {
    throw new Error(`${command} ${arguments_.join(' ')} failed: ${result.stderr.trim()}`);
  }
  return result.stdout;
}

const requiredFiles = [
  '.github/CODEOWNERS',
  '.github/ISSUE_TEMPLATE/bug_report.yml',
  '.github/ISSUE_TEMPLATE/config.yml',
  '.github/ISSUE_TEMPLATE/feature_request.yml',
  '.github/PULL_REQUEST_TEMPLATE.md',
  'AGENTS.md',
  'CHANGELOG.md',
  'CODE_OF_CONDUCT.md',
  'CONTRIBUTING.md',
  'LICENSE',
  'README.md',
  'SECURITY.md',
  'THIRD_PARTY_NOTICES.md',
  'TRADEMARKS.md',
  'docs/architecture-overview.md',
  'docs/configuration.md',
  'docs/dependency-policy.md',
  'docs/development.md',
  'docs/getting-started.md',
  'docs/protocol.md',
  'docs/security-model.md',
  'scripts/verify-public-repository',
];
for (const path of requiredFiles) {
  assert(existsSync(join(repositoryRoot, path)), `required public file is absent: ${path}`);
}
assert(!existsSync(join(repositoryRoot, 'docs/development-workflow.md')), 'obsolete development workflow is still present');
assert(!existsSync(join(repositoryRoot, 'scripts/check-repository.mjs')), 'superseded repository checker is still present');

const licenseDigest = createHash('sha256').update(readFileSync(join(repositoryRoot, 'LICENSE'))).digest('hex');
assert(
  licenseDigest === 'c71d239df91726fc519c6eb72d318ec65820627232b2f796219e87dcf35d0ab4',
  'LICENSE differs from the canonical Apache License 2.0 text installed by the project',
);

const metadata = JSON.parse(run('cargo', ['metadata', '--locked', '--no-deps', '--format-version', '1']));
assert(metadata.packages.length === 1, 'the workspace must contain exactly one Rust package');
const package_ = metadata.packages[0];
assert(package_.name === 'craxii-server', 'unexpected workspace package');
assert(package_.license === 'Apache-2.0', 'the Rust package license must be Apache-2.0');

const registry = JSON.parse(read('docs/dependency-registry.json'));
assert(registry.schema_version === 1, 'dependency registry schema_version must be 1');
assert(Array.isArray(registry.approved_direct_cargo_dependencies), 'dependency registry is missing its direct dependency array');

function dependencyKey(packageName, kind, target) {
  return `${packageName}|${kind ?? 'normal'}|${target ?? 'all-targets'}`;
}

const declaredDependencies = new Map();
for (const dependency of package_.dependencies) {
  const key = dependencyKey(dependency.name, dependency.kind, dependency.target);
  assert(!declaredDependencies.has(key), `duplicate declared dependency key: ${key}`);
  declaredDependencies.set(key, dependency);
}

const approvedDependencies = new Map();
for (const entry of registry.approved_direct_cargo_dependencies) {
  const key = dependencyKey(entry.package, entry.dependency_kind, entry.target_restriction);
  assert(!approvedDependencies.has(key), `duplicate dependency registry key: ${key}`);
  assert(entry.approval_status === 'approved', `dependency is not approved: ${key}`);
  assert(typeof entry.decision_record_path === 'string', `dependency record path is missing: ${key}`);
  assert(existsSync(join(repositoryRoot, entry.decision_record_path)), `dependency record is absent: ${entry.decision_record_path}`);
  approvedDependencies.set(key, entry);
}

assert(declaredDependencies.size === approvedDependencies.size, 'Cargo declarations and dependency registry differ in size');
for (const [key, dependency] of declaredDependencies) {
  const entry = approvedDependencies.get(key);
  assert(entry, `Cargo dependency is not in the approved registry: ${key}`);
  assert(dependency.req === entry.approved_compatible_requirement, `version requirement differs for ${key}`);
  assert(dependency.source === entry.source, `source differs for ${key}`);
  assert(dependency.uses_default_features === entry.default_features, `default feature policy differs for ${key}`);
  assert(dependency.optional === entry.optional, `optional policy differs for ${key}`);
  const actualFeatures = [...dependency.features].sort();
  const approvedFeatures = [...entry.enabled_features].sort();
  assert(JSON.stringify(actualFeatures) === JSON.stringify(approvedFeatures), `enabled features differ for ${key}`);
}

const swiftManifest = read('clients/macos/CraxiiClient/Package.swift');
assert(!/\.package\s*\(/.test(swiftManifest), 'the Swift package unexpectedly declares an external package');

const fixtureDirectory = join(repositoryRoot, 'backend/tests/fixtures/protocol-v1');
const manifestLines = read('backend/tests/fixtures/protocol-v1/manifest.sha256')
  .trim()
  .split('\n');
const manifestNames = [];
for (const line of manifestLines) {
  const match = line.match(/^([a-f0-9]{64})  ([A-Za-z0-9.-]+\.json)$/);
  assert(match, `invalid protocol fixture manifest line: ${line}`);
  const [, expectedDigest, name] = match;
  const bytes = readFileSync(join(fixtureDirectory, name));
  const actualDigest = createHash('sha256').update(bytes).digest('hex');
  assert(actualDigest === expectedDigest, `protocol fixture digest differs: ${name}`);
  JSON.parse(bytes.toString('utf8'));
  manifestNames.push(name);
}
const fixtureNames = readdirSync(fixtureDirectory).filter((name) => extname(name) === '.json').sort();
assert(JSON.stringify(manifestNames.sort()) === JSON.stringify(fixtureNames), 'protocol fixture manifest inventory differs');

const trackedFiles = run('git', ['ls-files', '--cached', '--others', '--exclude-standard', '-z'])
  .split('\0')
  .filter(Boolean);
const forbiddenArtifact = /(^|\/)(target|\.build|DerivedData|node_modules|\.DS_Store)(\/|$)|\.(db|db-wal|db-shm|sqlite|sqlite3|sqlite-wal|sqlite-shm|log)$/;
for (const path of trackedFiles) {
  assert(!forbiddenArtifact.test(path), `tracked build/runtime artifact: ${path}`);
}

// During the pre-publication transition, private-only paths from the frozen
// baseline remain in this working tree for the later history rewrite. Only
// those pre-existing, unchanged paths are tolerated. Once the rewrite removes
// the baseline object, any reintroduction of a matching path fails closed.
const frozenPrePublicationCommit = 'f2e88160e484f44417f428e41583b5f20fbf4575';
const privateOnlyPath = /^(?:docs\/(?:decisions|temp)\/|docs\/(?:CRAXII_V0\.0\.01_DEEP_ARCHITECTURE_SOURCE_OF_TRUTH|craxii-identity-credential-architecture|craxii-v0\.0\.01-architecture(?:-annotated)?|craxii-v0\.0\.01-implementation-plan|craxii2)\.(?:md|html)|docs\/scripts\/(?:render-implementation-plan|synchronize-architecture-companion)\.mjs)$/;
for (const path of trackedFiles.filter((candidate) => privateOnlyPath.test(candidate))) {
  const baselineObject = spawnSync('git', ['cat-file', '-e', `${frozenPrePublicationCommit}:${path}`], {
    cwd: repositoryRoot,
    stdio: 'ignore',
  });
  const unchangedFromBaseline = spawnSync('git', ['diff', '--quiet', frozenPrePublicationCommit, '--', path], {
    cwd: repositoryRoot,
    stdio: 'ignore',
  });
  assert(
    baselineObject.status === 0 && unchangedFromBaseline.status === 0,
    `private-only path was added or changed and must not enter the public repository: ${path}`,
  );
}

const rootMarkdown = readdirSync(repositoryRoot)
  .filter((name) => extname(name).toLowerCase() === '.md')
  .sort();
const publicDocumentation = [
  ...rootMarkdown,
  'docs/architecture-overview.md',
  'docs/configuration.md',
  'docs/dependency-policy.md',
  'docs/development.md',
  'docs/getting-started.md',
  'docs/protocol.md',
  'docs/security-model.md',
  ...readdirSync(join(repositoryRoot, 'docs/dependencies'))
    .filter((name) => extname(name).toLowerCase() === '.md')
    .sort()
    .map((name) => `docs/dependencies/${name}`),
];

const privateNameDenylist = [
  'craxii-v0.0.01-architecture',
  'craxii-v0.0.01-implementation-plan',
  'DEEP_ARCHITECTURE_SOURCE_OF_TRUTH',
  'docs/decisions/',
  'docs/temp/',
  'check-repository.mjs',
];
for (const path of publicDocumentation) {
  const source = read(path);
  for (const denied of privateNameDenylist) {
    assert(!source.includes(denied), `public documentation ${path} references a private-only name`);
  }
}

function validateMarkdownLinks(path) {
  const source = read(path);
  const base = dirname(join(repositoryRoot, path));
  for (const match of source.matchAll(/\[[^\]]*\]\(([^)]+)\)/g)) {
    let target = match[1].trim();
    if (target.startsWith('<') && target.endsWith('>')) target = target.slice(1, -1);
    target = target.split(/\s+['"]/)[0].split('#')[0];
    if (target === '' || /^(?:https?:|mailto:)/.test(target)) continue;
    const resolved = normalize(resolve(base, decodeURIComponent(target)));
    assert(resolved === repositoryRoot || resolved.startsWith(`${repositoryRoot}/`), `link escapes repository in ${path}: ${target}`);
    assert(existsSync(resolved), `broken relative link in ${path}: ${target}`);
    assert(statSync(resolved).isFile() || statSync(resolved).isDirectory(), `invalid link target in ${path}: ${target}`);
  }
}
for (const path of publicDocumentation) validateMarkdownLinks(path);

const verifier = read('scripts/verify');
assert(verifier.includes('scripts/check-public-repository.mjs'), 'scripts/verify does not run the public checker');
assert(!verifier.includes('scripts/check-repository.mjs'), 'scripts/verify still runs the superseded checker');
assert(!verifier.includes('render-implementation-plan'), 'scripts/verify still depends on a non-public documentation renderer');

console.log(`PUBLIC_REPOSITORY_CHECK: PASSED files=${trackedFiles.length} dependencies=${declaredDependencies.size} fixtures=${fixtureNames.length}`);
