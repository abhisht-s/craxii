#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
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

function findMatchingDelimiter(source, openingIndex, opening, closing) {
  let depth = 0;
  let quote = null;
  let escaped = false;
  let lineComment = false;
  let blockCommentDepth = 0;

  for (let index = openingIndex; index < source.length; index += 1) {
    const character = source[index];
    const next = source[index + 1];
    if (lineComment) {
      if (character === '\n') lineComment = false;
      continue;
    }
    if (blockCommentDepth > 0) {
      if (character === '/' && next === '*') {
        blockCommentDepth += 1;
        index += 1;
      } else if (character === '*' && next === '/') {
        blockCommentDepth -= 1;
        index += 1;
      }
      continue;
    }
    if (quote !== null) {
      if (escaped) {
        escaped = false;
      } else if (character === '\\') {
        escaped = true;
      } else if (character === quote) {
        quote = null;
      }
      continue;
    }
    if (character === '/' && next === '/') {
      lineComment = true;
      index += 1;
      continue;
    }
    if (character === '/' && next === '*') {
      blockCommentDepth = 1;
      index += 1;
      continue;
    }
    if (
      character === '"' ||
      (character === "'" && (source[index + 2] === "'" || (next === '\\' && source[index + 3] === "'")))
    ) {
      quote = character;
      continue;
    }
    if (character === opening) depth += 1;
    if (character === closing) {
      depth -= 1;
      if (depth === 0) return index;
    }
  }
  return -1;
}

function stripRustComments(source) {
  let result = '';
  let quote = null;
  let escaped = false;
  let lineComment = false;
  let blockCommentDepth = 0;
  for (let index = 0; index < source.length; index += 1) {
    const character = source[index];
    const next = source[index + 1];
    if (lineComment) {
      if (character === '\n') {
        lineComment = false;
        result += '\n';
      } else {
        result += ' ';
      }
      continue;
    }
    if (blockCommentDepth > 0) {
      if (character === '/' && next === '*') {
        blockCommentDepth += 1;
        result += '  ';
        index += 1;
      } else if (character === '*' && next === '/') {
        blockCommentDepth -= 1;
        result += '  ';
        index += 1;
      } else {
        result += character === '\n' ? '\n' : ' ';
      }
      continue;
    }
    if (quote !== null) {
      result += character;
      if (escaped) {
        escaped = false;
      } else if (character === '\\') {
        escaped = true;
      } else if (character === quote) {
        quote = null;
      }
      continue;
    }
    if (character === '/' && next === '/') {
      lineComment = true;
      result += '  ';
      index += 1;
    } else if (character === '/' && next === '*') {
      blockCommentDepth = 1;
      result += '  ';
      index += 1;
    } else {
      if (
        character === '"' ||
        (character === "'" && (source[index + 2] === "'" || (next === '\\' && source[index + 3] === "'")))
      ) {
        quote = character;
      }
      result += character;
    }
  }
  return result;
}

function withoutRustTestModules(source) {
  let result = source;
  const pattern = /#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]\s*(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+[A-Za-z_][A-Za-z0-9_]*\s*\{/g;
  while (true) {
    pattern.lastIndex = 0;
    const match = pattern.exec(result);
    if (!match) return result;
    const opening = match.index + match[0].lastIndexOf('{');
    const closing = findMatchingDelimiter(result, opening, '{', '}');
    assert(closing !== -1, 'a #[cfg(test)] Rust module has unbalanced braces');
    result = `${result.slice(0, match.index)}${' '.repeat(closing + 1 - match.index)}${result.slice(closing + 1)}`;
  }
}

function extractRustFunction(source, name) {
  const signature = new RegExp(`\\b(?:async\\s+)?fn\\s+${name}\\b`).exec(source);
  assert(signature, `Rust function ${name} is absent`);
  const opening = source.indexOf('{', signature.index);
  assert(opening !== -1, `Rust function ${name} has no body`);
  const closing = findMatchingDelimiter(source, opening, '{', '}');
  assert(closing !== -1, `Rust function ${name} has an unbalanced body`);
  return source.slice(signature.index, closing + 1);
}

function splitFirstTopLevelComma(source) {
  let parentheses = 0;
  let brackets = 0;
  let braces = 0;
  let quote = null;
  let escaped = false;
  for (let index = 0; index < source.length; index += 1) {
    const character = source[index];
    if (quote !== null) {
      if (escaped) escaped = false;
      else if (character === '\\') escaped = true;
      else if (character === quote) quote = null;
      continue;
    }
    if (character === '"') {
      quote = character;
    } else if (character === '(') parentheses += 1;
    else if (character === ')') parentheses -= 1;
    else if (character === '[') brackets += 1;
    else if (character === ']') brackets -= 1;
    else if (character === '{') braces += 1;
    else if (character === '}') braces -= 1;
    else if (character === ',' && parentheses === 0 && brackets === 0 && braces === 0) {
      return [source.slice(0, index), source.slice(index + 1)];
    }
  }
  return null;
}

const HTTP_METHODS = ['get', 'post', 'put', 'patch', 'delete', 'head', 'options'];
const ALLOWED_STAGE11_ROUTES = [
  'GET /health/live',
  'GET /health/ready',
  'GET /v1/bootstrap',
  'POST /v1/conversations/{conversation_id}/messages',
  'GET /v1/events',
  'POST /v1/work-items/{work_id}/cancel',
].sort();

function stage11RouteInventory(routerSource) {
  const routes = [];
  const routePattern = /\.route\s*\(/g;
  for (const match of routerSource.matchAll(routePattern)) {
    const opening = match.index + match[0].lastIndexOf('(');
    const closing = findMatchingDelimiter(routerSource, opening, '(', ')');
    assert(closing !== -1, 'Stage 11 route call has unbalanced parentheses');
    const argumentsSource = routerSource.slice(opening + 1, closing);
    const argumentsPair = splitFirstTopLevelComma(argumentsSource);
    assert(argumentsPair, 'Stage 11 route call must contain path and MethodRouter arguments');
    const pathMatch = argumentsPair[0].trim().match(/^"([^"]+)"$/);
    assert(pathMatch, `Stage 11 route path must be a static string: ${argumentsPair[0].trim()}`);
    const methodRouter = argumentsPair[1];
    const methods = new Set();
    for (const method of HTTP_METHODS) {
      if (new RegExp(`(?:^|[^A-Za-z0-9_])${method}\\s*\\(`).test(methodRouter)) {
        methods.add(method.toUpperCase());
      }
    }
    for (const filter of methodRouter.matchAll(
      /MethodFilter\s*::\s*(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\b/g,
    )) {
      methods.add(filter[1]);
    }
    if (/\b(?:any|any_service)\s*\(/.test(methodRouter)) {
      HTTP_METHODS.forEach((method) => methods.add(method.toUpperCase()));
    }
    assert(
      methods.size > 0,
      `Stage 11 route uses an opaque or unsupported MethodRouter expression for ${pathMatch[1]}`,
    );
    if (/\bon\s*\(/.test(methodRouter)) {
      assert(
        /MethodFilter\s*::/.test(methodRouter),
        `Stage 11 on(...) route must expose literal MethodFilter values for ${pathMatch[1]}`,
      );
    }
    const path = pathMatch[1].startsWith('/health/') ? pathMatch[1] : `/v1${pathMatch[1]}`;
    for (const method of methods) routes.push(`${method} ${path}`);
  }
  assert(
    !/\.route_service\s*\(/.test(routerSource),
    'Stage 11 route_service surfaces are forbidden because their method inventory is opaque',
  );
  return routes.sort();
}

function verifyStage11RouteInventory(routerSource) {
  const routes = stage11RouteInventory(routerSource);
  assert(
    equalStringArrays(routes, ALLOWED_STAGE11_ROUTES),
    `Stage 11 route inventory differs: ${routes.join(', ')}`,
  );
  return routes;
}

function verifyBootstrapSnapshotStructure(source) {
  const snapshotSource = stripRustComments(extractRustFunction(source, 'load_client_bootstrap_inner'));
  const beginMatch = /(?:self\s*\.\s*)?(?:runtime\s*\.\s*inner\s*\.\s*)?pool\s*\.\s*begin\s*\(\s*\)/.exec(
    snapshotSource,
  );
  assert(beginMatch, 'Stage 11 snapshot must begin one transaction from the SQLite pool');
  const headRead = snapshotSource.indexOf('SELECT max(journal_offset) FROM journal_events');
  const firstFetch = snapshotSource.search(/\.fetch_(?:one|all|optional)\s*\(/);
  const transactionCommit = snapshotSource.indexOf('.commit()');
  const publicReturn = snapshotSource.indexOf('Ok(ClientBootstrapCandidate');
  assert(
    headRead !== -1 && firstFetch !== -1 && headRead < firstFetch,
    'Stage 11 snapshot first read must establish the journal head',
  );
  assert(
    transactionCommit > firstFetch && publicReturn > transactionCommit,
    'Stage 11 snapshot must commit/release its transaction before returning public data',
  );
  const transactionRegion = snapshotSource.slice(beginMatch.index + beginMatch[0].length, transactionCommit);
  assert(
    !/(?:self\s*\.\s*)?(?:runtime\s*\.\s*inner\s*\.\s*)?pool\b/.test(transactionRegion) &&
      !/\b(?:global_)?(?:connection|conn)\b/.test(transactionRegion),
    'Stage 11 snapshot projection region must not access a pool-backed or global connection',
  );
  const fetches = [...transactionRegion.matchAll(/\.fetch_(?:one|all|optional)\s*\(([^)]*)\)/g)];
  assert(fetches.length === 5, `Stage 11 snapshot must retain exactly five reads; found ${fetches.length}`);
  assert(
    fetches.every((fetch) => /^\s*&mut\s+\*transaction\s*$/.test(fetch[1])),
    'every Stage 11 snapshot read must use the snapshot transaction handle',
  );
  for (const helper of transactionRegion.matchAll(/\bself\s*\.\s*([A-Za-z_][A-Za-z0-9_]*)\s*\(/g)) {
    if (helper[1] === 'fire_stage11_snapshot_hook') continue;
    const opening = helper.index + helper[0].lastIndexOf('(');
    const closing = findMatchingDelimiter(transactionRegion, opening, '(', ')');
    assert(closing !== -1, `snapshot helper ${helper[1]} has unbalanced arguments`);
    const argumentsSource = transactionRegion.slice(opening + 1, closing);
    assert(
      /&mut\s+\*?transaction\b/.test(argumentsSource),
      `snapshot helper ${helper[1]} must receive the snapshot transaction handle`,
    );
  }
  return snapshotSource;
}

function stage12ImplementationLeaks(productionFiles) {
  const leaks = [];
  const generalImplementation = /tokio::process|std::process::Command|Command::new|struct\s+ToolRegistry\b|(?:async\s+)?fn\s+execute_tool\s*\(|reqwest|hyper::client|(?:async\s+)?fn\s+(?:invoke|stream|execute)_model\s*\(|struct\s+ContextAssembler\b|struct\s+(?:Real)?WorkRunner\b|impl\s+WorkRunner\s+for|(?:async\s+)?fn\s+run_agent_loop\s*\(|(?:async\s+)?fn\s+generate_assistant_completion\s*\(|(?:async\s+)?fn\s+stream_draft\s*\(/;
  for (const file of productionFiles) {
    const source = stripRustComments(withoutRustTestModules(file.source));
    const workstationTrait = /\b(?:pub(?:\s*\([^)]*\))?\s+)?trait\s+[A-Za-z_][A-Za-z0-9_]*Workstation(?:Port|Adapter)?\b/.test(
      source,
    );
    const workstationAdapter = /\bimpl\b[^\{]*\b[A-Za-z_][A-Za-z0-9_]*Workstation(?:Port|Adapter)?\b[^\{]*\bfor\b/.test(
      source,
    );
    const readFileSurface = /\b(?:async\s+)?fn\s+read_file\s*\(/.test(source);
    let workstationCapabilitiesSurface = false;
    for (const block of source.matchAll(/\b(?:impl|trait)\b[^\{]*\bWorkstation[A-Za-z0-9_]*[^\{]*\{/g)) {
      const opening = block.index + block[0].lastIndexOf('{');
      const closing = findMatchingDelimiter(source, opening, '{', '}');
      assert(closing !== -1, `workstation block in ${file.path} has unbalanced braces`);
      if (/\b(?:async\s+)?fn\s+capabilities\s*\(/.test(source.slice(opening, closing + 1))) {
        workstationCapabilitiesSurface = true;
      }
    }
    const workstationFilesystem =
      (workstationTrait || workstationAdapter || readFileSurface || workstationCapabilitiesSurface) &&
      /(?:std|tokio)::fs::(?:read|read_to_string)|File::open\s*\(/.test(source);
    if (
      workstationTrait || workstationAdapter || readFileSurface || workstationCapabilitiesSurface ||
      workstationFilesystem || generalImplementation.test(source)
    ) {
      leaks.push(file.path);
    }
  }
  return sortedStrings(new Set(leaks));
}

function expectStructuralRejection(label, operation) {
  let rejected = false;
  try {
    operation();
  } catch {
    rejected = true;
  }
  assert(rejected, `checker negative probe was not rejected: ${label}`);
}

function verifyStage11CheckerNegativeProbes() {
  const allowedRouter = `
    Router::new()
      .route("/health/live", get(liveness))
      .route("/health/ready", get(readiness))
      .route("/bootstrap", get(bootstrap).delete(remove_bootstrap))
      .route("/conversations/{conversation_id}/messages", post(message))
      .route("/work-items/{work_id}/cancel", post(cancel))
      .route("/events", get(events));`;
  expectStructuralRejection('DELETE /v1/bootstrap', () => verifyStage11RouteInventory(allowedRouter));

  const poolBackedSnapshot = `
    async fn load_client_bootstrap_inner(&self) -> Result<ClientBootstrapCandidate, Error> {
      let mut transaction = self.pool.begin().await?;
      let head = query("SELECT max(journal_offset) FROM journal_events")
        .fetch_one(&mut *transaction).await?;
      let roots = query("roots").fetch_all(&mut *transaction).await?;
      let messages = query("messages").fetch_all(&mut *transaction).await?;
      let work = query("work").fetch_all(&mut *transaction).await?;
      let tools = query("tools").fetch_all(&mut *transaction).await?;
      let leaked = query("leaked").fetch_all(&self.pool).await?;
      transaction.commit().await?;
      Ok(ClientBootstrapCandidate { head, roots, messages, work, tools, leaked })
    }`;
  expectStructuralRejection('pool-backed bootstrap projection', () =>
    verifyBootstrapSnapshotStructure(poolBackedSnapshot),
  );

  const stage12Fixture = [{
    path: 'adapters/local_workstation.rs',
    source: `
      pub struct LocalWorkstation;
      impl LocalWorkstation {
        pub fn capabilities(&self) -> WorkstationCapabilities { todo!() }
        pub fn read_file(&self, path: &Path) -> Vec<u8> { std::fs::read(path).unwrap() }
      }`,
  }];
  assert(
    stage12ImplementationLeaks(stage12Fixture).length === 1,
    'checker negative probe was not rejected: concrete Stage 12 workstation implementation',
  );
  return 3;
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
  assert(
    html.split(actualHash).length >= 3,
    `${relative(repositoryRoot, htmlPath)} must show the complete source hash visibly as well as in metadata`,
  );
}

function walkFiles(root) {
  if (!existsSync(root)) return [];
  return readdirSync(root, { withFileTypes: true }).flatMap((entry) => {
    const path = join(root, entry.name);
    return entry.isDirectory() ? walkFiles(path) : [path];
  });
}

function trackedFiles() {
  const result = spawnSync('git', ['ls-files', '-z'], {
    cwd: repositoryRoot,
    encoding: 'utf8',
  });
  if (result.error) {
    throw new Error(`git ls-files could not start: ${result.error.message}`);
  }
  if (result.status !== 0) {
    throw new Error(`git ls-files failed with exit status ${result.status}`);
  }
  return result.stdout.split('\0').filter(Boolean);
}

function verifyStage11Boundaries() {
  const rustRoot = join(repositoryRoot, 'backend', 'src');
  const sqliteRoot = join(rustRoot, 'adapters', 'sqlite');
  const rustFiles = walkFiles(rustRoot).filter((path) => path.endsWith('.rs'));
  const sqlxLeaks = [];
  const productionSqliteLocationLeaks = [];

  for (const path of rustFiles) {
    const source = readFileSync(path, 'utf8');
    const pathName = relative(repositoryRoot, path);
    if (!path.startsWith(`${sqliteRoot}/`) && /\bsqlx\s*::|\bextern\s+crate\s+sqlx\b/.test(source)) {
      sqlxLeaks.push(pathName);
    }
    if (/["'](?:sqlite:[^"']*|:memory:)["']/.test(source)) {
      productionSqliteLocationLeaks.push(pathName);
    }
  }
  assert(
    sqlxLeaks.length === 0,
    `SQLx crate references escaped backend/src/adapters/sqlite: ${sqlxLeaks.join(', ')}`,
  );
  assert(
    productionSqliteLocationLeaks.length === 0,
    `production-style SQLite URLs or memory locations are forbidden: ${productionSqliteLocationLeaks.join(', ')}`,
  );

  const migrationRoot = join(repositoryRoot, 'backend', 'migrations');
  const migrationFiles = walkFiles(migrationRoot)
    .map((path) => relative(migrationRoot, path))
    .sort();
  assert(
    equalStringArrays(migrationFiles, [
      '0001_core_durable_schema.sql',
      '0002_journal_and_work_inputs.sql',
      '0003_context_model_tool_artifacts.sql',
    ]),
    `Stage 10 must contain exactly migrations 0001, 0002, and 0003; found ${migrationFiles.join(', ') || 'nothing'}`,
  );
  const migration1 = readFileSync(join(migrationRoot, migrationFiles[0]), 'utf8');
  const migration2 = readFileSync(join(migrationRoot, migrationFiles[1]), 'utf8');
  const migration3 = readFileSync(join(migrationRoot, migrationFiles[2]), 'utf8');
  const migrations = `${migration1}\n${migration2}\n${migration3}`;
  const migrationChecksums = [migration1, migration2, migration3].map((migration) =>
    createHash('sha384').update(migration).digest('hex'),
  );
  assert(
    equalStringArrays(migrationChecksums, [
      '717c44a33c94ccaadbdb6fd7a2cc3b4d99eb269216de241f379af7cce2c3557eb78e5a0ba98b1fe280d2b8449675dd8d',
      '677379cfb19c61d45c6a61bdeb978539490adcee97f57e51cab8794e63038b70950d715a90e7e524397007a97f875ebf',
      'e2f5cab2ac0921ce54e6ae8a741eb23c11766e847a5d17f21f7381ae4aa1d729287542c1ebaa12d25431f3b277cd5c39',
    ]),
    `migration checksum inventory differs: ${migrationChecksums.join(', ')}`,
  );

  const expectedStage6Tables = [
    'client_commands',
    'client_devices',
    'conversations',
    'craxii_principals',
    'messages',
    'runtime_instances',
    'work_items',
    'workspaces',
    'workstations',
  ];
  const actualStage6Tables = [...migration1.matchAll(/\bCREATE\s+TABLE\s+([a-z][a-z0-9_]*)/gi)]
    .map((match) => match[1])
    .sort();
  assert(
    equalStringArrays(actualStage6Tables, expectedStage6Tables),
    `migration 0001 table inventory differs: ${actualStage6Tables.join(', ')}`,
  );
  const expectedStage7Tables = ['journal_events', 'stream_heads', 'work_item_inputs'];
  const actualStage7Tables = [...migration2.matchAll(/\bCREATE\s+TABLE\s+([a-z][a-z0-9_]*)/gi)]
    .map((match) => match[1])
    .sort();
  assert(
    equalStringArrays(actualStage7Tables, expectedStage7Tables),
    `migration 0002 table inventory differs: ${actualStage7Tables.join(', ')}`,
  );
  for (const table of expectedStage7Tables) {
    assert(
      !new RegExp(`\\bCREATE\\s+TABLE\\s+${table}\\b`, 'i').test(migration1),
      `Stage 7 table is premature in migration 0001: ${table}`,
    );
  }
  const expectedStage8Tables = [
    'artifacts',
    'context_manifest_sources',
    'context_manifests',
    'model_invocations',
    'tool_executions',
  ];
  const actualStage8Tables = [...migration3.matchAll(/\bCREATE\s+TABLE\s+([a-z][a-z0-9_]*)/gi)]
    .map((match) => match[1])
    .sort();
  assert(
    equalStringArrays(actualStage8Tables, expectedStage8Tables),
    `migration 0003 table inventory differs: ${actualStage8Tables.join(', ')}`,
  );
  for (const table of expectedStage8Tables) {
    assert(
      !new RegExp(`\\bCREATE\\s+TABLE\\s+${table}\\b`, 'i').test(`${migration1}\n${migration2}`),
      `Stage 8 table is premature before migration 0003: ${table}`,
    );
  }
  assert(
    (migration3.match(/\)\s+STRICT,\s+WITHOUT\s+ROWID\s*;/gi) ?? []).length === 5,
    'every Stage 8 table must be STRICT and WITHOUT ROWID',
  );
  for (const table of [
    'artifact_blobs',
    'evidence_refs',
    'authority_evidence',
    'model_outputs',
    'model_output_items',
    'tool_results',
    'stdout_chunks',
    'stderr_chunks',
    'retention_policies',
    'artifact_deletions',
    'provider_conversations',
    'authentication_tokens',
    'command_idempotency',
    'schema_versions',
  ]) {
    assert(
      !new RegExp(`\\bCREATE\\s+TABLE\\s+${table}\\b`, 'i').test(migrations),
      `forbidden custom table is present in the frozen V3 schema: ${table}`,
    );
  }

  const expectedIndexes = [
    'ix_journal_events_conversation_offset',
    'ix_journal_events_work_offset',
    'ix_artifacts_content',
    'ix_artifacts_producer_kind_id',
    'ix_artifacts_producing_work',
    'ix_artifacts_storage_key',
    'ix_context_manifest_sources_artifact',
    'ix_context_manifest_sources_event',
    'ix_context_manifests_work_created',
    'ix_messages_conversation',
    'ix_model_invocations_context_attempt',
    'ix_model_invocations_runtime_nonterminal',
    'ix_runtime_instances_craxii_state',
    'ix_tool_executions_runtime_nonterminal',
    'ix_work_items_nonterminal_by_runtime',
    'ix_work_items_queued_fifo',
    'ix_workspaces_craxii_id',
    'ix_workstations_craxii_id',
    'ux_client_devices_token_hash',
    'ux_conversations_craxii_kind',
    'ux_context_manifests_logical_invocation',
    'ux_journal_events_event_id',
    'ux_journal_events_stream_sequence',
    'ux_messages_client_identity',
    'ux_messages_produced_by_work',
    'ux_model_invocations_logical_attempt',
    'ux_model_invocations_one_nonterminal_per_work',
    'ux_model_invocations_retry_of',
    'ux_model_invocations_work_step_attempt',
    'ux_tool_executions_execution_id',
    'ux_tool_executions_one_nonterminal_per_work',
    'ux_tool_executions_source_ordinal',
    'ux_tool_executions_source_provider_call',
    'ux_tool_executions_work_step_ordinal',
    'ux_work_item_inputs_work_ordinal',
    'ux_work_items_conversation_ordinal',
    'ux_work_items_current_model_invocation',
    'ux_work_items_current_tool_execution',
    'ux_work_items_one_active_per_conversation',
    'ux_workspaces_workstation_logical_name',
  ].sort();
  const actualIndexes = [...migrations.matchAll(/\bCREATE\s+(?:UNIQUE\s+)?INDEX\s+([a-z][a-z0-9_]*)/gi)]
    .map((match) => match[1])
    .sort();
  assert(
    equalStringArrays(actualIndexes, expectedIndexes),
    `Stage 10 named index inventory differs: ${actualIndexes.join(', ')}`,
  );
  assert(
    !/\b(?:raw_token|bearer_token|access_token|token)\s+TEXT\b/i.test(migrations),
    'migrations must not contain a raw bearer-token persistence column',
  );
  assert(
    /CREATE\s+INDEX\s+ix_messages_conversation\s+ON\s+messages\s*\(conversation_id\)/i.test(migration1),
    'message membership index must contain only conversation_id; ordering remains journal-derived',
  );
  const messageIndexColumns = [
    ...migration1.matchAll(/CREATE\s+(?:UNIQUE\s+)?INDEX[^;]*?ON\s+messages\s*\(([^)]*)\)/gi),
  ].map((match) => match[1]);
  assert(
    messageIndexColumns.every((columns) =>
      !/(?:^|,)\s*(?:committed_at|message_id)\s*(?:,|$)/i.test(columns)),
    'message indexes must not create timestamp/UUID ordering authority',
  );
  assert(
    !/\b(?:provider_request_json|provider_response_json|authorization_header|api_key|openai_|responses_api)\b/i.test(migrations),
    'provider wire types must not enter migrations',
  );
  assert(
    /requested_cwd\s+TEXT\s+NOT\s+NULL/i.test(migration3) &&
      !/requested_cwd\s+TEXT\s+NULL/i.test(migration3),
    'Stage 8 requested_cwd must persist one concrete non-null effective logical path',
  );
  assert(!/\bCREATE\s+TRIGGER\b/i.test(migrations), 'production trigger inventory must remain zero');
  assert(!/\bCREATE\s+VIEW\b/i.test(migrations), 'production view inventory must remain zero');
  assert(
    !/\b(?:visibility|public_payload)\b/i.test(migration2),
    'journal migration must not persist public replay shape',
  );

  const sqliteFiles = walkFiles(sqliteRoot).filter((path) => path.endsWith('.rs'));
  const sqliteSource = sqliteFiles.map((path) => readFileSync(path, 'utf8')).join('\n');
  assert(!/\bquery(?:_as)?!\s*\(/.test(sqliteSource), 'SQLx query macros are forbidden in persistence codecs');
  assert(!/derive\([^)]*FromRow/.test(sqliteSource), 'SQLx FromRow derives are forbidden on persistence rows');
  assert(!existsSync(join(repositoryRoot, '.sqlx')), 'SQLx offline metadata is not part of Stage 9');
  const productionSqliteSource = sqliteFiles
    .filter((path) => !/_tests\.rs$/.test(path))
    .map((path) => readFileSync(path, 'utf8'))
    .join('\n');
  assert(
    !/\b(?:UPDATE\s+journal_events|DELETE\s+FROM\s+journal_events)\b/i.test(productionSqliteSource),
    'production adapters must not mutate committed journal rows',
  );
  assert(
    !/\b(?:UPDATE\s+work_item_inputs|DELETE\s+FROM\s+work_item_inputs)\b/i.test(productionSqliteSource),
    'production adapters must not mutate Work input rows',
  );
  assert(
    !/\b(?:UPDATE\s+client_commands|DELETE\s+FROM\s+client_commands)\b/i.test(productionSqliteSource),
    'client_commands rows are insert-only durable receipts',
  );
  assert(
    !/UPDATE\s+client_devices\s+SET\s+(?:token_hash|display_name)\b/i.test(productionSqliteSource),
    'V0 device credentials and display names are immutable; rotation provisions a replacement',
  );
  assert(
    /impl\s+ReplayStateStore\s+for\s+SqliteStateStore/.test(productionSqliteSource),
    'Stage 11 public replay capability is absent',
  );
  assert(
    !/pub(?:\([^)]*\))?\s+async\s+fn\s+insert_artifact_metadata/.test(productionSqliteSource),
    'artifact metadata insertion must remain adapter-private and transaction-composed',
  );

  const artifactAdapterRoot = join(rustRoot, 'adapters', 'artifacts');
  assert(
    existsSync(join(artifactAdapterRoot, 'local.rs')) && existsSync(join(artifactAdapterRoot, 'mod.rs')),
    'the local artifact filesystem adapter must live outside the SQLite adapter',
  );
  const artifactPort = readFileSync(join(rustRoot, 'ports', 'artifact_store.rs'), 'utf8');
  assert(
    !/std::path|PathBuf|\bPath\b/.test(artifactPort),
    'ArtifactStore public types must not expose physical filesystem paths',
  );
  assert(
    !/impl\s+FinalizedArtifact\s*\{[\s\S]*?pub\s+(?:const\s+)?fn\s+new\s*\(/.test(artifactPort) &&
      /pub\(crate\)\s+const\s+fn\s+from_durable_publication\s*\(/.test(artifactPort),
    'FinalizedArtifact construction must remain sealed from public callers',
  );
  const finalizedArtifactFields = artifactPort.match(
    /pub struct FinalizedArtifact\s*\{([\s\S]*?)\n\}/,
  );
  assert(
    finalizedArtifactFields !== null &&
      !/\bpub(?:\([^)]*\))?\s+\w+\s*:/.test(finalizedArtifactFields[1]),
    'FinalizedArtifact fields must remain private',
  );
  const durablePublicationCallSites = rustFiles
    .filter((path) => readFileSync(path, 'utf8').includes('FinalizedArtifact::from_durable_publication('))
    .map((path) => relative(rustRoot, path));
  assert(
    equalStringArrays(durablePublicationCallSites, ['adapters/artifacts/local.rs']),
    `only the local artifact adapter may mint FinalizedArtifact: ${durablePublicationCallSites.join(', ')}`,
  );
  const stage8Transactions = readFileSync(join(sqliteRoot, 'stage8.rs'), 'utf8')
    .split('\n#[cfg(test)]')[0];
  assert(
    !/FinalizedArtifact::from_durable_publication/.test(stage8Transactions),
    'SQLite must reconstruct verification references, not finalized-publication capabilities',
  );
  assert(
    !/tokio::process|std::process|Command::new|reqwest|anthropic|openai/i.test(stage8Transactions),
    'Stage 8 transactions must not perform tool or provider side effects',
  );

  const stage9Transactions = readFileSync(join(sqliteRoot, 'stage9.rs'), 'utf8')
    .split('\n#[cfg(test)]')[0];
  assert(
    /impl\s+CommandStateStore\s+for\s+SqliteStateStore/.test(stage9Transactions),
    'Stage 9 CommandStateStore implementation is absent',
  );
  assert(
    /impl\s+DeviceCredentialStore\s+for\s+SqliteStateStore/.test(stage9Transactions),
    'Stage 9 device credential persistence implementation is absent',
  );
  assert(
    !/tokio::process|std::process::Command|Command::new|reqwest|anthropic|openai/i.test(stage9Transactions),
    'Stage 9 transactions must not perform scheduler, tool, or provider side effects',
  );

  const cargoManifest = readFileSync(join(repositoryRoot, 'backend', 'Cargo.toml'), 'utf8');
  assert(
    /^serde_json\s*=\s*"1\.0"$/m.test(cargoManifest) &&
      !/\[dev-dependencies\][\s\S]*^serde_json\s*=/m.test(cargoManifest),
    'serde_json must be one normal direct production dependency, not dev-only',
  );
  assert(
    /^getrandom\s*=\s*\{\s*version\s*=\s*"0\.4",\s*default-features\s*=\s*false\s*\}$/m.test(cargoManifest),
    'getrandom 0.4 must be a direct default-feature-free dependency',
  );
  assert(
    !/^\s*(?:rand|jsonwebtoken|base64|hmac|subtle|zeroize)\s*=/mi.test(cargoManifest),
    'Stage 9 must not add alternate RNG, JWT, base64, HMAC, subtle, or zeroization dependencies',
  );
  const cargoLock = readFileSync(join(repositoryRoot, 'Cargo.lock'), 'utf8');
  assert(
    /\[\[package\]\]\s+name = "getrandom"\s+version = "0\.4\.3"/m.test(cargoLock),
    'the direct getrandom dependency must retain locked resolution 0.4.3',
  );
  const compatibility = readFileSync(join(rustRoot, 'bootstrap', 'compatibility.rs'), 'utf8');
  const schema = readFileSync(join(sqliteRoot, 'schema.rs'), 'utf8');
  assert(
    /MAX_SUPPORTED_SCHEMA_VERSION:\s*u64\s*=\s*3;/.test(compatibility),
    'bootstrap schema compatibility ceiling must be 3',
  );
  assert(
    /MAX_SUPPORTED_SCHEMA_VERSION:\s*i64\s*=\s*3;/.test(schema),
    'SQLite schema compatibility ceiling must be 3',
  );

  const journalDomain = readFileSync(join(rustRoot, 'domain', 'journal.rs'), 'utf8');
  assert(/pub const ALL: \[Self; 28\]/.test(journalDomain), 'journal registry must contain 28 events');
  assert(/"model\.invocation_streaming"/.test(journalDomain), 'model streaming event is absent');
  assert(
    /"tool\.execution_interrupted_before_dispatch"/.test(journalDomain),
    'tool pre-dispatch interruption event is absent',
  );
  assert(
    /WorkCancelled\s*=>\s*\("work\.cancelled",\s*Work,\s*true,\s*Stage9,\s*true\)/.test(journalDomain),
    'work.cancelled first-emitter ownership must be Stage 9',
  );

  const sqliteArtifacts = trackedFiles().filter((path) =>
    /(?:\.sqlite3?|\.db)(?:-(?:wal|shm))?$/i.test(path),
  );
  assert(
    sqliteArtifacts.length === 0,
    `tracked SQLite database artifacts are forbidden: ${sqliteArtifacts.join(', ')}`,
  );
  const trackedArtifactObjects = trackedFiles().filter(
    (path) =>
      /(?:^|\/)artifacts\/(?:tmp|sha256)\//.test(path) ||
      /\.partial$/i.test(path),
  );
  assert(
    trackedArtifactObjects.length === 0,
    `tracked runtime artifact files are forbidden: ${trackedArtifactObjects.join(', ')}`,
  );
  const stateStore = readFileSync(join(rustRoot, 'ports', 'state_store.rs'), 'utf8');
  assert(!/\bsqlx\s*::/.test(stateStore), 'StateStore must remain free of SQLx crate types');
  assert(
    !/fn\s+\w*transaction|fn\s+transaction|with_transaction|begin_transaction/.test(stateStore),
    'StateStore must not expose a generic transaction operation',
  );
  assert(!/payload_json/.test(stateStore), 'StateStore must not expose raw journal payload JSON');
  assert(!/payload_json/.test(journalDomain), 'trusted journal domain types must not expose raw JSON');
  assert(
    /pub\s+requested_cwd:\s+LogicalPathReference/.test(stateStore) &&
      !/pub\s+requested_cwd:\s+Option\s*</.test(stateStore),
    'Stage 8 persistence port requested_cwd must be concrete and nonoptional',
  );
  assert(
    !/impl\s+CompletionStateStore\s+for\s+SqliteStateStore/.test(productionSqliteSource),
    'Stage 17 completion behavior must remain unimplemented',
  );
  const canonicalPersistence = [stateStore, artifactPort, journalDomain].join('\n');
  assert(
    !/\b(?:authorization_header|api_key|bearer_token|access_token)\b/i.test(canonicalPersistence),
    'canonical domain and ports must not persist raw authorization or credential fields',
  );
  assert(
    !/\b(?:stdout_bytes|stderr_bytes|raw_output|process_output)\b/i.test(journalDomain),
    'journal payloads must not contain raw process or filesystem output',
  );
  const projector = readFileSync(join(rustRoot, 'application', 'projector.rs'), 'utf8')
    .split('\n#[cfg(test)]')[0];
  assert(!/\bsqlx\s*::/.test(projector), 'pure projector must remain SQLx-free');
  assert(/stream_seq/.test(projector), 'projector must retain journal-derived stream ordering');

  const stage9CanonicalFiles = [
    join(rustRoot, 'domain', 'authentication.rs'),
    join(rustRoot, 'domain', 'command.rs'),
    join(rustRoot, 'application', 'authentication.rs'),
    join(rustRoot, 'application', 'command_service.rs'),
    join(rustRoot, 'ports', 'device_credentials.rs'),
    join(rustRoot, 'ports', 'state_store.rs'),
  ];
  const stage9Canonical = stage9CanonicalFiles
    .map((path) => readFileSync(path, 'utf8').split('\n#[cfg(test)]')[0])
    .join('\n');
  assert(
    !/\b(?:axum|tower|Authorization|HeaderMap|WebSocket)\b/.test(stage9Canonical),
    'Stage 9 canonical layers must remain transport-free',
  );
  assert(
    !/\b(?:jwt|jsonwebtoken)\b/i.test(stage9Canonical),
    'Stage 9 authentication must remain opaque bearer-token authentication, not JWT',
  );
  assert(
    !/pub\s+(?:raw_)?(?:bearer|token)(?:_text|_bytes)?\s*:/.test(stage9Canonical),
    'raw bearer material must not be a public persistence or command field',
  );
  assert(
    !/after_message_transaction_commit|after_cancel_requested_commit/.test(stage9Transactions),
    'Stage 10-owned named post-commit failpoints must have no Stage 9 callsite',
  );

  const stage10Transactions = readFileSync(join(sqliteRoot, 'stage10.rs'), 'utf8')
    .split('\n#[cfg(test)]')[0];
  assert(
    /impl\s+SchedulerStateStore\s+for\s+SqliteStateStore/.test(stage10Transactions) &&
      /impl\s+RuntimeStateStore\s+for\s+SqliteStateStore/.test(stage10Transactions) &&
      /impl\s+RecoveryStateStore\s+for\s+SqliteStateStore/.test(stage10Transactions),
    'Stage 10 scheduler, runtime, and recovery persistence implementations must all be present',
  );
  assert(
    /BEGIN IMMEDIATE/.test(readFileSync(join(sqliteRoot, 'transaction.rs'), 'utf8')) &&
      /ix_work_items_queued_fifo/.test(sqliteSource) &&
      /conversation_work_ordinal ASC, w\.work_id ASC/.test(stage10Transactions),
    'Stage 10 FIFO claim must retain BEGIN IMMEDIATE coordination and the frozen queued index/order',
  );
  assert(
    /ix_work_items_nonterminal_by_runtime/.test(stage10Transactions) &&
      /ix_model_invocations_runtime_nonterminal/.test(migrations) &&
      /ix_tool_executions_runtime_nonterminal/.test(migrations),
    'Stage 10 recovery must retain all three frozen runtime-recovery indexes',
  );
  assert(
    !/tokio::process|std::process::Command|Command::new|reqwest|anthropic|openai/i.test(stage10Transactions),
    'Stage 10 recovery transactions must not execute tools or call providers',
  );

  const runtimeDomain = readFileSync(join(rustRoot, 'domain', 'runtime.rs'), 'utf8');
  assert(
    /pub enum RuntimeState[\s\S]*Running[\s\S]*Stopping[\s\S]*Stopped/.test(runtimeDomain) &&
      /GracefulShutdown/.test(runtimeDomain) && /StartupFailure/.test(runtimeDomain),
    'the exact Stage 10 RuntimeInstance lifecycle is absent',
  );
  assert(
    /pub struct RuntimeStartedV1/.test(journalDomain) &&
      /pub struct RuntimeRecoveryPerformedV1/.test(journalDomain) &&
      /pub struct RuntimeStoppingV1/.test(journalDomain) &&
      !/pub struct RuntimeEventV1/.test(journalDomain),
    'runtime events must use three event-specific strict V1 DTOs',
  );

  const scheduler = readFileSync(join(rustRoot, 'application', 'scheduler.rs'), 'utf8')
    .split('\n#[cfg(test)]')[0];
  const runtimeController = readFileSync(join(rustRoot, 'application', 'runtime.rs'), 'utf8')
    .split('\n#[cfg(test)]')[0];
  assert(
    /pub trait WorkRunner/.test(scheduler) && /JoinSet/.test(scheduler) &&
      /TaskRegistryView/.test(scheduler) && /Duration::from_secs\(1\)/.test(scheduler),
    'Stage 10 must own a narrow WorkRunner, joined registry, and one-second fallback scan',
  );
  assert(
    /HEARTBEAT_CADENCE:\s*Duration\s*=\s*Duration::from_secs\(5\)/.test(runtimeController) &&
      /ShutdownController/.test(runtimeController) && /classify_unresolved_shutdown_work/.test(runtimeController),
    'Stage 10 heartbeat and conservative graceful-shutdown controller are incomplete',
  );
  assert(
    !/start_scheduler\s*\(/.test(readFileSync(join(rustRoot, 'bootstrap', 'startup.rs'), 'utf8')) &&
      !/mark_ready\s*\(/.test(readFileSync(join(rustRoot, 'bootstrap', 'startup.rs'), 'utf8')),
    'production Stage 10 bootstrap must remain live_unready until a real Stage 17 WorkRunner exists',
  );

  const commandService = readFileSync(join(rustRoot, 'application', 'command_service.rs'), 'utf8')
    .split('\n#[cfg(test)]')[0];
  const failpoints = readFileSync(join(rustRoot, 'test_failpoints.rs'), 'utf8')
    .split('\n#[cfg(test)]')[0];
  for (const variant of [
    'AfterMessageTransactionCommit',
    'AfterWorkClaimCommit',
    'AfterCancelRequestedCommit',
    'DuringGracefulShutdown',
  ]) {
    assert(
      new RegExp(`active_spec\\(\\s*FailpointName::${variant}`).test(failpoints),
      `Stage 10 failpoint ${variant} is not active`,
    );
  }
  assert(
    (commandService.match(/PhysicalHook::AfterMessageTransactionCommit/g) ?? []).length === 1 &&
      (commandService.match(/PhysicalHook::AfterCancelRequestedCommit/g) ?? []).length === 1 &&
      (scheduler.match(/PhysicalHook::AfterWorkClaimCommit/g) ?? []).length === 1 &&
      (runtimeController.match(/PhysicalHook::DuringGracefulShutdown/g) ?? []).length === 1,
    'the four Stage 10 failpoints must each have exactly one production callsite',
  );
  assert(
    /pub const ALL: \[Self; 14\]/.test(failpoints),
    'Stage 10 must not add a new public failpoint name',
  );

  const httpAdapter = readFileSync(join(rustRoot, 'adapters', 'http.rs'), 'utf8');
  const routerSource = extractRustFunction(httpAdapter, 'router');
  verifyStage11RouteInventory(routerSource);
  assert(
    /let health = Router::new\(\)[\s\S]*\/health\/live[\s\S]*\/health\/ready[\s\S]*let protected = Router::new\(\)/.test(routerSource) &&
      /let protected = Router::new\(\)[\s\S]*\.route\("\/bootstrap", get\(bootstrap\)\)[\s\S]*\.route\("\/events", get\(events\)\)[\s\S]*\.fallback\(not_found\)[\s\S]*\.method_not_allowed_fallback\(method_not_allowed\)[\s\S]*\.layer\(middleware::from_fn\([\s\S]*authenticate\(authentication_state\.clone\(\), request, next\)[\s\S]*\.merge\(health\)[\s\S]*\.nest\("\/v1", protected\)/.test(routerSource),
    'the /v1 subrouter, including its WebSocket route and fallbacks, must be authenticated while health remains outside that layer',
  );
  assert(
    /server delivery only/.test(httpAdapter) &&
      !/route\([^\n]*events[^\n]*post\(/.test(httpAdapter),
    'the WebSocket route must remain server-delivery-only with no mutation route',
  );
  assert(
    /SetSensitiveRequestHeadersLayer[\s\S]*AUTHORIZATION/.test(httpAdapter) &&
      /CACHE_CONTROL[\s\S]*no-store/.test(httpAdapter) &&
      /x-content-type-options[\s\S]*nosniff/.test(httpAdapter),
    'Stage 11 Authorization sensitivity and security response headers are incomplete',
  );

  const protocol = readFileSync(join(rustRoot, 'protocol.rs'), 'utf8');
  for (const constant of [
    ['MESSAGE_BODY_LIMIT', '512 \\* 1024'],
    ['CANCELLATION_BODY_LIMIT', '8 \\* 1024'],
    ['HTTP_CONCURRENCY_LIMIT', '64'],
    ['MUTATION_CONCURRENCY_LIMIT', '16'],
    ['WEBSOCKET_CONNECTION_LIMIT', '32'],
    ['REPLAY_PAGE_ROWS', '128'],
    ['CURSOR_BROADCAST_CAPACITY', '256'],
    ['WEBSOCKET_OUTBOUND_FRAMES', '16'],
    ['MAX_DURABLE_PAYLOAD_BYTES', '262_144'],
    ['MAX_WEBSOCKET_FRAME_BYTES', '270_336'],
  ]) {
    assert(
      new RegExp(`pub const ${constant[0]}:[^=]+= ${constant[1]};`).test(protocol),
      `Stage 11 protocol constant ${constant[0]} differs`,
    );
  }

  const publication = readFileSync(join(rustRoot, 'application', 'publication.rs'), 'utf8')
    .split('\n#[cfg(test)]')[0];
  assert(
    /JournalEventPayload::WorkWaitingOnTool/.test(publication) &&
      /JournalEventPayload::WorkResumed[\s\S]*"transition_kind": "resumed"/.test(publication) &&
      /JournalEventPayload::RuntimeStopping\(_\) => return Ok\(None\)/.test(publication),
    'Stage 11 explicit public event allowlist/omission mapping is incomplete',
  );
  assert(
    !/stream_id|correlation_id|causation_id|state_version|runtime_instance_id|provider_call_id|request_hash/.test(
      publication.replace(/JournalEventPayload/g, ''),
    ),
    'Stage 11 publication serializer contains an internal envelope field',
  );

  const stage11Storage = readFileSync(join(sqliteRoot, 'stage11.rs'), 'utf8');
  const snapshotSource = verifyBootstrapSnapshotStructure(stage11Storage);
  const snapshotStart = stage11Storage.indexOf('pub(super) async fn load_client_bootstrap_inner');
  const snapshotEnd = stage11Storage.indexOf('pub(super) async fn list_replay_page_inner', snapshotStart);
  assert(snapshotStart !== -1 && snapshotEnd > snapshotStart, 'Stage 11 snapshot function is absent');
  const replayStart = snapshotEnd;
  const replayEnd = stage11Storage.indexOf('\n    #[cfg(test)]\n    pub(super) fn set_stage11_snapshot_hook', replayStart);
  assert(replayEnd > replayStart, 'Stage 11 replay function boundary is absent');
  const replaySource = stage11Storage.slice(replayStart, replayEnd);
  assert(
    /request\.limit == 0 \|\| request\.limit > crate::protocol::REPLAY_PAGE_ROWS/.test(replaySource) &&
      /journal_offset > \? AND journal_offset <= \?[\s\S]*ORDER BY journal_offset ASC LIMIT \?/.test(replaySource) &&
      /\.bind\(request\.through\.get\(\)\)/.test(replaySource) &&
      /let has_more =/.test(replaySource) && /let scanned_through =/.test(replaySource) &&
      /REPLAY_PAGE_ROWS:\s*u32\s*=\s*128/.test(protocol),
    'Stage 11 replay structure must retain its fixed through bound, ascending offset order, 128-row limit, scanned progress, and has-more check',
  );

  const stage11Tests = readFileSync(join(sqliteRoot, 'stage11_tests.rs'), 'utf8');
  for (const testName of [
    'real_http_health_auth_message_replay_conflict_limits_and_redaction',
    'route_methods_and_authenticated_v1_fallback_boundary_are_real',
    'real_http_lost_postcommit_response_retries_exactly_once_over_new_connection',
    'websocket_slow_consumer_closes_1013_without_durable_change_and_reconnect_recovers',
    'websocket_connection_limit_rejects_thirty_third_upgrade_retryably',
    'shutdown_waits_for_pending_upgrade_then_initial_replay_observes_latched_shutdown',
    'stage11_second_repair_deadline_keeps_pending_connection_owned_until_callback_terminal_record_is_consumed',
    'upgrade_callback_panic_is_observed_and_isolated',
    'binary_websocket_application_frame_closes_1008_without_mutation',
    'shared_server_failure_supervisor_triggers_existing_shutdown_and_preserves_cause',
    'stage11_second_repair_server_return_after_accept_stop_but_before_stage10_latch_is_unexpected_and_fatal',
    'stage11_second_repair_server_return_after_stage10_latch_is_expected_and_not_fatal',
    'stage11_second_repair_primary_server_failure_precedes_connection_cleanup_failure',
    'stage11_second_repair_connection_cleanup_failure_surfaces_when_server_completion_is_graceful',
    'shared_server_child_panic_is_observed_with_join_cause',
    'bootstrap_first_head_barrier_defines_both_sides_of_concurrent_commit',
    'replay_over_three_pages_crosses_mixed_and_all_filtered_underlying_rows',
  ]) {
    assert(
      new RegExp(`async fn ${testName}\\(`).test(stage11Tests),
      `Stage 11 behavioral test inventory is missing ${testName}`,
    );
  }
  for (const canary of [
    'PROVIDER_CANARY',
    'MODEL_CANARY',
    'TOOL_ARGUMENTS_CANARY',
    'TOOL_RESULT_CANARY',
    'ARTIFACT_METADATA_CANARY',
  ]) {
    assert(stage11Tests.includes(canary), `Stage 11 redaction inventory is missing ${canary}`);
  }
  assert(
    /fn public_event_frame_size_boundary_encodes_without_truncation_and_rejects_oversize\(/.test(
      readFileSync(join(rustRoot, 'application', 'publication.rs'), 'utf8'),
    ),
    'Stage 11 behavioral test inventory is missing the public-event frame boundary test',
  );
  assert(
    /fn stage11_second_repair_duplicate_or_late_terminal_record_is_rejected_without_double_accounting\(/.test(
      httpAdapter,
    ),
    'Stage 11 behavioral test inventory is missing duplicate/late completion accounting coverage',
  );

  const fixtureRoot = join(repositoryRoot, 'backend', 'tests', 'fixtures', 'protocol-v1');
  const fixtureNames = walkFiles(fixtureRoot)
    .map((path) => relative(fixtureRoot, path))
    .filter((name) => name.endsWith('.json'))
    .sort();
  assert(
    equalStringArrays(fixtureNames, [
      'bootstrap-snapshot.json',
      'cancellation-request.json',
      'cancellation-response.json',
      'durable-events.json',
      'error-envelope.json',
      'health.json',
      'message-request.json',
      'message-response.json',
      'sync-complete.json',
    ]),
    `Stage 11 protocol golden inventory differs: ${fixtureNames.join(', ')}`,
  );
  const manifestLines = readFileSync(join(fixtureRoot, 'manifest.sha256'), 'utf8')
    .trim().split('\n');
  assert(manifestLines.length === fixtureNames.length, 'Stage 11 golden manifest length differs');
  const manifestNames = [];
  for (const line of manifestLines) {
    const match = line.match(/^([a-f0-9]{64})  ([a-z0-9-]+\.json)$/);
    assert(match, `invalid Stage 11 golden manifest line: ${line}`);
    const actual = createHash('sha256').update(readFileSync(join(fixtureRoot, match[2]))).digest('hex');
    assert(actual === match[1], `Stage 11 golden hash differs for ${match[2]}`);
    manifestNames.push(match[2]);
  }
  assert(equalStringArrays(manifestNames.sort(), fixtureNames), 'Stage 11 golden manifest names differ');

  assert(
    /^axum\s*=\s*\{[^\n]*version\s*=\s*"0\.8\.9"[^\n]*features\s*=\s*\["http1", "json", "matched-path", "query", "tokio", "tracing", "ws"\][^\n]*\}$/m.test(cargoManifest) &&
      /^tower\s*=\s*\{[^\n]*version\s*=\s*"0\.5\.3"[^\n]*features\s*=\s*\["limit", "util"\][^\n]*\}$/m.test(cargoManifest) &&
      /^tower-http\s*=\s*\{[^\n]*version\s*=\s*"0\.7\.0"[^\n]*features\s*=\s*\["limit", "sensitive-headers", "set-header", "timeout", "trace"\][^\n]*\}$/m.test(cargoManifest),
    'Stage 11 production dependency versions/features differ',
  );
  assert(
    /\[dev-dependencies\][\s\S]*^futures-util\s*=\s*\{[^\n]*version\s*=\s*"0\.3\.34"[^\n]*features\s*=\s*\["sink", "std"\]/m.test(cargoManifest) &&
      /\[dev-dependencies\][\s\S]*^tokio-tungstenite\s*=\s*\{[^\n]*version\s*=\s*"0\.29\.0"[^\n]*features\s*=\s*\["connect"\]/m.test(cargoManifest),
    'Stage 11 development dependency versions/features differ',
  );
  assert(
    !/^\s*(?:hyper|http)\s*=/m.test(cargoManifest) &&
      !/^tokio-tungstenite\s*=/m.test(cargoManifest.split('[dev-dependencies]')[0]) &&
      !/^\s*(?:rustls|native-tls|openssl|tower-http-cors|cors)\s*=/mi.test(cargoManifest),
    'forbidden direct Hyper/HTTP/production WebSocket/TLS/CORS dependency is present',
  );
  const laterStageModuleLeaks = rustFiles
    .filter((path) => !/_tests\.rs$/.test(path))
    .map((path) => relative(rustRoot, path))
    .filter((path) => /(?:^|\/)(?:workstation|read-file|read_file|process-executor|process_executor|tool-registry|tool_registry|provider-client|provider_client|context-assembler|context_assembler|agent-loop|agent_loop|assistant-completion|assistant_completion|draft-stream|draft_stream)\.rs$/.test(path));
  const productionImplementationFiles = rustFiles
    .filter((path) => !/_tests\.rs$/.test(path))
    .map((path) => ({
      path: relative(rustRoot, path),
      source: readFileSync(path, 'utf8'),
    }));
  const laterStageImplementationLeaks = stage12ImplementationLeaks(productionImplementationFiles);
  assert(
    laterStageModuleLeaks.length === 0 &&
      laterStageImplementationLeaks.length === 0,
    `Stage 12+ workstation/process/tool/provider/context/runner/assistant/draft implementation is forbidden in Stage 11: ${[...laterStageModuleLeaks, ...laterStageImplementationLeaks].join(', ')}`,
  );

  const checkerNegativeProbeCount = verifyStage11CheckerNegativeProbes();

  assert(
    /^tokio\s*=\s*\{[^\n]*features\s*=\s*\["io-util", "macros", "net", "rt-multi-thread", "signal", "sync", "time"\][^\n]*\}$/m.test(cargoManifest),
    'Tokio features must equal the Stage 10 set plus only Stage 11 net/io-util',
  );

  const trackedAdminResidue = trackedFiles().filter((path) =>
    /(?:^|\/)(?:craxii-admin-output|device-token|admin-result)(?:\.|\/|$)/i.test(path),
  );
  assert(
    trackedAdminResidue.length === 0,
    `tracked admin output residue is forbidden: ${trackedAdminResidue.join(', ')}`,
  );

  return {
    rustFileCount: rustFiles.length,
    migrationFileCount: migrationFiles.length,
    tableCount: actualStage6Tables.length + actualStage7Tables.length + actualStage8Tables.length,
    indexCount: actualIndexes.length,
    checkerNegativeProbeCount,
  };
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
  const stage11 = verifyStage11Boundaries();

  assert(directDependencyCount > 0 && stage11.checkerNegativeProbeCount === 3, 'checker summary evidence is incomplete');
  console.log('Stage 11 structural invariants passed.');
} catch (error) {
  console.error(`Repository invariant failed: ${error.message}`);
  process.exitCode = 1;
}
