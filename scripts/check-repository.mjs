#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repositoryRoot = join(scriptDir, '..');

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function cargoMetadata() {
  const result = spawnSync(
    'cargo',
    ['metadata', '--locked', '--no-deps', '--format-version', '1'],
    { cwd: repositoryRoot, encoding: 'utf8' },
  );

  if (result.error) {
    throw new Error(`cargo metadata could not start: ${result.error.message}`);
  }
  if (result.status !== 0) {
    const detail = result.stderr.trim() || `exit status ${result.status}`;
    throw new Error(`cargo metadata failed: ${detail}`);
  }

  try {
    return JSON.parse(result.stdout);
  } catch (error) {
    throw new Error(`cargo metadata returned invalid JSON: ${error.message}`);
  }
}

function dependencyRegistry() {
  const registryPath = join(repositoryRoot, 'docs', 'dependency-registry.json');
  let registry;

  try {
    registry = JSON.parse(readFileSync(registryPath, 'utf8'));
  } catch (error) {
    throw new Error(`dependency registry is invalid: ${error.message}`);
  }

  assert(registry?.schema_version === 1, 'dependency registry schema_version must be 1');
  assert(
    Array.isArray(registry.approved_direct_cargo_dependencies),
    'dependency registry must contain approved_direct_cargo_dependencies',
  );
  return registry.approved_direct_cargo_dependencies;
}

function dependencyKind(dependency) {
  return dependency.kind ?? 'normal';
}

function dependencyKey(dependency) {
  return [
    dependency.package ?? dependency.name,
    dependency.dependency_kind ?? dependencyKind(dependency),
    dependency.target_restriction ?? dependency.target ?? 'all-targets',
  ].join('|');
}

function sortedStrings(values) {
  return [...values].sort((left, right) => left.localeCompare(right));
}

function equalStringArrays(left, right) {
  return left.length === right.length && left.every((value, index) => value === right[index]);
}

function verifyDirectDependencies(metadata, workspacePackages) {
  const registryEntries = dependencyRegistry();
  const registryByKey = new Map();

  for (const entry of registryEntries) {
    assert(typeof entry.package === 'string' && entry.package.length > 0, 'registry package is required');
    assert(typeof entry.source === 'string' && entry.source.length > 0, `${entry.package} registry source is required`);
    assert(
      ['normal', 'dev', 'build'].includes(entry.dependency_kind),
      `${entry.package} has invalid dependency kind ${entry.dependency_kind}`,
    );
    assert(
      typeof entry.approved_compatible_requirement === 'string' &&
        entry.approved_compatible_requirement.length > 0,
      `${entry.package} approved compatible requirement is required`,
    );
    assert(Array.isArray(entry.enabled_features), `${entry.package} enabled_features must be an array`);
    assert(
      entry.enabled_features.every((feature) => typeof feature === 'string'),
      `${entry.package} enabled_features must contain only strings`,
    );
    assert(typeof entry.default_features === 'boolean', `${entry.package} default_features must be boolean`);
    assert(typeof entry.optional === 'boolean', `${entry.package} optional must be boolean`);
    assert(
      entry.target_restriction === null || typeof entry.target_restriction === 'string',
      `${entry.package} target_restriction must be null or a string`,
    );
    assert(entry.approval_status === 'approved', `${entry.package} is not approved`);
    assert(
      typeof entry.approver_role === 'string' && entry.approver_role.length > 0,
      `${entry.package} approver role is required`,
    );
    assert(
      /^\d{4}-\d{2}-\d{2}$/.test(entry.approval_date),
      `${entry.package} approval date must use YYYY-MM-DD`,
    );
    assert(
      typeof entry.decision_record_path === 'string' && entry.decision_record_path.length > 0,
      `${entry.package} decision record path is required`,
    );
    assert(
      existsSync(join(repositoryRoot, entry.decision_record_path)),
      `${entry.package} decision record is absent: ${entry.decision_record_path}`,
    );

    const key = dependencyKey(entry);
    assert(!registryByKey.has(key), `duplicate dependency registry entry ${key}`);
    registryByKey.set(key, entry);
  }

  const declarations = workspacePackages.flatMap((workspacePackage) =>
    workspacePackage.dependencies.map((dependency) => ({
      ...dependency,
      workspacePackage: workspacePackage.name,
    })),
  );
  const declarationsByKey = new Map();

  for (const declaration of declarations) {
    const kind = dependencyKind(declaration);
    assert(
      ['normal', 'dev', 'build'].includes(kind),
      `${declaration.workspacePackage} declares unsupported dependency kind ${kind}`,
    );

    const key = dependencyKey(declaration);
    assert(!declarationsByKey.has(key), `duplicate Cargo dependency declaration ${key}`);
    declarationsByKey.set(key, declaration);

    const approved = registryByKey.get(key);
    assert(approved, `${declaration.workspacePackage} declares unlisted direct dependency ${key}`);
    assert(
      declaration.source === approved.source,
      `${key} source ${declaration.source ?? 'path'} does not match registry ${approved.source}`,
    );
    assert(
      declaration.req === approved.approved_compatible_requirement,
      `${key} requirement ${declaration.req} does not match registry ${approved.approved_compatible_requirement}`,
    );
    assert(
      equalStringArrays(sortedStrings(declaration.features), sortedStrings(approved.enabled_features)),
      `${key} features do not match the dependency registry`,
    );
    assert(
      declaration.uses_default_features === approved.default_features,
      `${key} default-features setting does not match the dependency registry`,
    );
    assert(
      declaration.optional === approved.optional,
      `${key} optional setting does not match the dependency registry`,
    );
    assert(
      (declaration.target ?? null) === approved.target_restriction,
      `${key} target restriction does not match the dependency registry`,
    );
  }

  for (const key of registryByKey.keys()) {
    assert(declarationsByKey.has(key), `dependency registry entry is absent from Cargo manifests: ${key}`);
  }

  return declarations.length;
}

function attribute(tag, name) {
  const match = tag.match(new RegExp(`\\b${name}\\s*=\\s*["']([^"']+)["']`, 'i'));
  return match?.[1];
}

function embeddedSourceHash(html, htmlPath) {
  const metaTags = html.match(/<meta\b[^>]*>/gi) ?? [];
  const sourceHashTag = metaTags.find(
    (tag) => attribute(tag, 'name')?.toLowerCase() === 'craxii-source-sha256',
  );
  assert(sourceHashTag, `${relative(repositoryRoot, htmlPath)} has no source-hash metadata`);

  const hash = attribute(sourceHashTag, 'content');
  assert(
    /^[a-f0-9]{64}$/.test(hash ?? ''),
    `${relative(repositoryRoot, htmlPath)} has invalid source-hash metadata`,
  );
  return hash;
}

function verifyGeneratedCompanion(sourceName, htmlName) {
  const sourcePath = join(repositoryRoot, 'docs', sourceName);
  const htmlPath = join(repositoryRoot, 'docs', htmlName);
  const source = readFileSync(sourcePath);
  const html = readFileSync(htmlPath, 'utf8');
  const actualHash = createHash('sha256').update(source).digest('hex');
  const recordedHash = embeddedSourceHash(html, htmlPath);

  assert(
    recordedHash === actualHash,
    `${relative(repositoryRoot, htmlPath)} source hash does not match ${relative(repositoryRoot, sourcePath)}`,
  );
}

try {
  const metadata = cargoMetadata();
  assert(
    Array.isArray(metadata.workspace_members) && metadata.workspace_members.length === 1,
    `expected exactly one workspace member, found ${metadata.workspace_members?.length ?? 0}`,
  );
  assert(
    Array.isArray(metadata.packages) && metadata.packages.length === 1,
    `expected exactly one workspace package, found ${metadata.packages?.length ?? 0}`,
  );

  const workspacePackage = metadata.packages.find(
    (candidate) => candidate.id === metadata.workspace_members[0],
  );
  assert(workspacePackage, 'workspace member package is absent from cargo metadata');
  assert(
    workspacePackage.name === 'craxii-server',
    `expected workspace package craxii-server, found ${workspacePackage.name}`,
  );

  const libraryTargets = workspacePackage.targets.filter((target) => target.kind.includes('lib'));
  const binaryTargets = workspacePackage.targets.filter((target) => target.kind.includes('bin'));
  assert(libraryTargets.length > 0, 'craxii-server must define a library target');
  assert(binaryTargets.length > 0, 'craxii-server must define a binary target');
  assert(
    libraryTargets.some((library) =>
      binaryTargets.every((binary) => library.src_path !== binary.src_path)),
    'craxii-server library and binary targets must be distinct',
  );
  assert(
    Array.isArray(workspacePackage.features?.default) && workspacePackage.features.default.length === 0,
    'craxii-server default feature set must be empty',
  );
  assert(
    Array.isArray(workspacePackage.features?.['test-failpoints']),
    'craxii-server must define the test-failpoints feature',
  );
  assert(
    workspacePackage.features['test-failpoints'].length === 0,
    'craxii-server test-failpoints feature must enable no dependencies',
  );

  const workspacePackages = metadata.packages.filter((candidate) =>
    metadata.workspace_members.includes(candidate.id),
  );
  const directDependencyCount = verifyDirectDependencies(metadata, workspacePackages);

  verifyGeneratedCompanion(
    'craxii-v0.0.01-architecture.md',
    'craxii-v0.0.01-architecture-annotated.html',
  );
  verifyGeneratedCompanion(
    'craxii-v0.0.01-implementation-plan.md',
    'craxii-v0.0.01-implementation-plan.html',
  );

  console.log(
    `Repository invariants passed: 1 workspace member/package, craxii-server lib/bin, empty defaults, dependency-free test-failpoints feature, ${directDependencyCount} approved direct Cargo dependencies, 2 generated HTML source hashes`,
  );
} catch (error) {
  console.error(`Repository invariant failed: ${error.message}`);
  process.exitCode = 1;
}
