#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  cpSync,
  existsSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
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

function stage17PlusImplementationLeaks(productionFiles) {
  const leaks = [];
  const generalImplementation = /reqwest|hyper::client|\b(?:OpenAI|Anthropic)(?:Client|Adapter)\b|\b(?:struct|trait|impl)\s+ModelGateway\b|struct\s+(?:Real)?WorkRunner\b|impl\s+WorkRunner\s+for|(?:async\s+)?fn\s+run_agent_loop\s*\(|(?:async\s+)?fn\s+generate_assistant_completion\s*\(|(?:async\s+)?fn\s+stream_draft\s*\(|\b(?:struct|impl)\s+RemoteWorkstation\b|\b(?:struct|impl)\s+Mcp(?:Client|Server|Transport)\b|\bfn\s+(?:register|load)_(?:plugin|dynamic_tool)s?\s*\(/;
  for (const file of productionFiles) {
    const source = stripRustComments(withoutRustTestModules(file.source));
    if (generalImplementation.test(source)) {
      leaks.push(file.path);
    }
  }
  return sortedStrings(new Set(leaks));
}

function stage14HandlerViolations(source) {
  const production = stripRustComments(withoutRustTestModules(source));
  const violations = [];
  const forbidden = [
    ['handler StateStore access', /\b(?:Tool)?StateStore\b/],
    ['handler SQLx access', /\bsqlx\b|\bSqlite[A-Za-z0-9_]*\b/],
    ['handler journal access', /\bJournal[A-Za-z0-9_]*\b|\bjournal(?:_|\b)/],
    ['handler direct filesystem access', /\b(?:std|tokio)::fs\b|\b(?:File|OpenOptions)::open\s*\(/],
    ['handler direct process access', /\b(?:std|tokio)::process\b|\bCommand::new\s*\(/],
  ];
  for (const [label, pattern] of forbidden) {
    if (pattern.test(production)) violations.push(label);
  }
  if (/\.capabilities\s*\(|\.inspect_execution\s*\(|\.cancel_execution\s*\(/.test(production)) {
    violations.push('handler owns a non-action Workstation lifecycle operation');
  }
  const functions = rustFunctionBlocks(production);
  const inventory = stage14FunctionInventory(functions);
  const analysis = newStage14CallGraphAnalysis(inventory, 'handler');
  const invokes = functions.filter((block) => block.name === 'invoke');
  const summary = invokes.reduce(
    (combined, block) => mergeStage14Summaries(
      combined,
      analysis.region(block.body, stage14TypedWorkstationReceivers(block, false), block.name),
    ),
    emptyStage14Summary(),
  );
  if (summary.read !== 1) violations.push('read_file action is not single-shot from handler roots');
  if (summary.execute !== 1) violations.push('execute action is not single-shot from handler roots');
  if (summary.repeated) violations.push('handler machine operation is reachable through retry or recursion');
  if (summary.depthExceeded) violations.push('handler helper call graph exceeded the finite analysis bound');
  return violations;
}

const STAGE14_CALL_GRAPH_MAX_DEPTH = 16;

function rustFunctionBlocks(source) {
  const blocks = [];
  const pattern = /\b(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\b/g;
  for (const match of source.matchAll(pattern)) {
    const parameters = source.indexOf('(', match.index);
    if (parameters === -1) continue;
    const parameterEnd = findMatchingDelimiter(source, parameters, '(', ')');
    if (parameterEnd === -1) continue;
    const opening = source.indexOf('{', parameterEnd);
    const semicolon = source.indexOf(';', parameterEnd);
    if (opening === -1 || (semicolon !== -1 && semicolon < opening)) continue;
    const closing = findMatchingDelimiter(source, opening, '{', '}');
    if (closing === -1) continue;
    blocks.push({
      name: match[1],
      signature: source.slice(match.index, opening),
      parameters: source.slice(parameters + 1, parameterEnd),
      body: source.slice(opening + 1, closing),
      source: source.slice(match.index, closing + 1),
    });
  }
  return blocks;
}

function stage14FunctionInventory(functions) {
  const inventory = new Map();
  for (const block of functions) {
    const definitions = inventory.get(block.name) ?? [];
    definitions.push(block);
    inventory.set(block.name, definitions);
  }
  return inventory;
}

function stage14TypedWorkstationReceivers(block, includeServiceField) {
  const receivers = new Set();
  for (const match of `${block.parameters},${block.body}`.matchAll(
    /\b([A-Za-z_][A-Za-z0-9_]*)\s*:\s*[^,)]*\bWorkstation\b[^,)]*/g,
  )) {
    receivers.add(match[1]);
  }
  if (includeServiceField && /(?:^|,)\s*&(?:'\w+\s+)?self\b/.test(block.parameters)) {
    receivers.add('self.workstation');
  }
  let changed = true;
  while (changed) {
    changed = false;
    for (const match of block.body.matchAll(
      /\blet\s+(?:mut\s+)?([A-Za-z_][A-Za-z0-9_]*)[^=;]*=\s*&?\s*((?:self\s*\.\s*)?[A-Za-z_][A-Za-z0-9_]*)\s*(?:\.\s*as_ref\s*\(\s*\))?\s*;/g,
    )) {
      const source = match[2].replace(/\s+/g, '');
      if (receivers.has(source) && !receivers.has(match[1])) {
        receivers.add(match[1]);
        changed = true;
      }
    }
  }
  return receivers;
}

function emptyStage14Summary() {
  return {
    read: 0,
    execute: 0,
    handoff: 0,
    cancel: 0,
    cancelSites: [],
    repeated: false,
    cycle: false,
    cycleSites: [],
    depthExceeded: false,
    visited: new Set(),
  };
}

function mergeStage14Summaries(left, right, multiplier = 1) {
  left.read += right.read * multiplier;
  left.execute += right.execute * multiplier;
  left.handoff += right.handoff * multiplier;
  left.cancel += right.cancel * multiplier;
  left.cancelSites.push(...right.cancelSites);
  left.repeated ||= right.repeated || multiplier > 1 && stage14MachineActionCount(right) > 0;
  left.cycle ||= right.cycle;
  left.cycleSites.push(...right.cycleSites);
  left.depthExceeded ||= right.depthExceeded;
  for (const name of right.visited) left.visited.add(name);
  return left;
}

function stage14MachineActionCount(summary) {
  return summary.read + summary.execute + summary.handoff;
}

function stage14DirectSinks(body, receivers, sourceName, moduleKind) {
  const summary = emptyStage14Summary();
  for (const match of body.matchAll(
    /\b((?:self\s*\.\s*)?[A-Za-z_][A-Za-z0-9_]*)\s*\.\s*(read_file|execute|cancel_execution)\s*\(/g,
  )) {
    const receiver = match[1].replace(/\s+/g, '');
    if (!receivers.has(receiver)) continue;
    if (match[2] === 'read_file') summary.read += 1;
    else if (match[2] === 'execute') summary.execute += 1;
    else {
      summary.cancel += 1;
      summary.cancelSites.push(sourceName);
    }
  }
  for (const match of body.matchAll(
    /\bWorkstation\s*::\s*(read_file|execute|cancel_execution)\s*\(\s*&?\s*([A-Za-z_][A-Za-z0-9_]*)/g,
  )) {
    if (!receivers.has(match[2])) continue;
    if (match[1] === 'read_file') summary.read += 1;
    else if (match[1] === 'execute') summary.execute += 1;
    else {
      summary.cancel += 1;
      summary.cancelSites.push(sourceName);
    }
  }
  if (moduleKind === 'service') {
    summary.handoff += [...body.matchAll(/\bhandler\s*\.\s*invoke\s*\(/g)].length;
  }
  return summary;
}

function stage14LocalCallCounts(body, inventory) {
  const calls = new Map();
  for (const name of inventory.keys()) {
    const callPattern = new RegExp(
      `(?:\\bself\\s*\\.\\s*|\\bSelf\\s*::\\s*|(?<![A-Za-z0-9_.:]))${name}\\s*\\(`,
      'g',
    );
    const count = [...body.matchAll(callPattern)].length;
    if (count > 0) calls.set(name, count);
  }
  return calls;
}

function newStage14CallGraphAnalysis(inventory, moduleKind) {
  const memo = new Map();
  const visited = new Set();

  const summarizeFunction = (name, depth, active) => {
    if (memo.has(name)) return memo.get(name);
    if (active.has(name)) {
      const cycle = emptyStage14Summary();
      cycle.cycle = true;
      cycle.cycleSites.push(name);
      return cycle;
    }
    if (depth > STAGE14_CALL_GRAPH_MAX_DEPTH) {
      const exhausted = emptyStage14Summary();
      exhausted.depthExceeded = true;
      return exhausted;
    }
    const nextActive = new Set(active);
    nextActive.add(name);
    visited.add(name);
    const result = emptyStage14Summary();
    result.visited.add(name);
    for (const block of inventory.get(name) ?? []) {
      const receivers = stage14TypedWorkstationReceivers(block, moduleKind === 'service');
      const direct = stage14DirectSinks(block.body, receivers, name, moduleKind);
      mergeStage14Summaries(result, direct);
      for (const [callee, count] of stage14LocalCallCounts(block.body, inventory)) {
        const child = summarizeFunction(callee, depth + 1, nextActive);
        mergeStage14Summaries(result, child, count);
      }
      if (
        /\bloop\s*\{|\bwhile\s+[^;{]+\{|\bfor\s+[A-Za-z_][A-Za-z0-9_]*\s+in\b/.test(block.body) &&
        stage14MachineActionCount(result) > 0
      ) {
        result.repeated = true;
      }
    }
    if (result.cycle && stage14MachineActionCount(result) > 0) result.repeated = true;
    memo.set(name, result);
    return result;
  };

  return {
    region(body, receivers = new Set(), sourceName = '<root>') {
      const result = stage14DirectSinks(body, receivers, sourceName, moduleKind);
      for (const [callee, count] of stage14LocalCallCounts(body, inventory)) {
        mergeStage14Summaries(result, summarizeFunction(callee, 1, new Set()), count);
      }
      for (const name of visited) result.visited.add(name);
      return result;
    },
  };
}

function stage14ServiceViolations(source) {
  const production = stripRustComments(withoutRustTestModules(source));
  const violations = [];
  const functions = rustFunctionBlocks(production);
  const executeBlock = functions.find((block) => block.name === 'execute_call');
  if (!executeBlock) {
    return ['execute_call is absent or malformed'];
  }
  const executeCall = executeBlock.body;
  const inventory = stage14FunctionInventory(functions.filter((block) => block !== executeBlock));
  const analysis = newStage14CallGraphAnalysis(inventory, 'service');
  const serviceReceivers = stage14TypedWorkstationReceivers(executeBlock, true);
  const requested = executeCall.indexOf('.request_tool_execution(');
  const dispatch = executeCall.indexOf('.commit_tool_dispatch_intent(');
  const handoff = executeCall.indexOf('handler.invoke(');
  if (!(requested !== -1 && dispatch > requested && handoff > dispatch)) {
    violations.push('machine handoff is not dominated by committed dispatch intent');
  }
  const beforeDispatch = dispatch === -1 ? executeCall : executeCall.slice(0, dispatch);
  const preDispatch = analysis.region(beforeDispatch, serviceReceivers, 'execute_call');
  if (stage14MachineActionCount(preDispatch) > 0) {
    violations.push('pre-dispatch machine action is reachable');
  }
  if (preDispatch.depthExceeded) {
    violations.push('pre-dispatch helper call graph exceeded the finite analysis bound');
  }
  const total = analysis.region(executeCall, serviceReceivers, 'execute_call');
  if (stage14MachineActionCount(total) !== 1 || total.handoff !== 1) {
    violations.push('one tool attempt does not have exactly one reachable machine handoff');
  }
  if (total.repeated) {
    violations.push(`machine action is reachable through retry, loop, or recursion (${total.read}/${total.execute}/${total.handoff}; repeated=${total.repeated}; cycle=${total.cycle}; cycle_sites=${sortedStrings(new Set(total.cycleSites)).join(',')})`);
  }
  if (total.depthExceeded) {
    violations.push('service helper call graph exceeded the finite analysis bound');
  }
  if (
    total.cancelSites.some((site) => site !== 'await_handler') ||
    total.cancel > 1 ||
    total.cancel > 0 && !/handoff_started[\s\S]*cancellable_execution[\s\S]*cancellation_sent/.test(
      (inventory.get('await_handler') ?? [])[0]?.body ?? '',
    )
  ) {
    violations.push('cancel_execution escapes the active post-handoff cancellation path');
  }
  const freeze = executeCall.indexOf('freeze_tool_deadline(');
  const afterFreeze = freeze === -1 ? '' : executeCall.slice(freeze);
  if (
    freeze === -1 ||
    requested < freeze ||
    /Instant\s*::\s*now\s*\(\s*\)\s*(?:\+|\.checked_add\s*\()[^;]*(?:effective_timeout|timeout)/.test(afterFreeze) ||
    /monotonic_now\s*\(\s*\)\s*\.\s*elapsed\s*\(\s*\)\s*\.\s*checked_add/.test(afterFreeze)
  ) {
    violations.push('Stage 14 machine deadline is reconstructed after the freeze point');
  }
  return violations;
}

function stage14RegistryViolations(source) {
  const production = stripRustComments(withoutRustTestModules(source));
  const violations = [];
  if (!/Self::try_new\s*\(\s*vec!\[\s*read_file_definition\(policy\)\s*,\s*run_shell_definition\(policy\)\s*,?\s*\]\s*\)/s.test(production)) {
    violations.push('registry inventory or order differs');
  }
  if (!/pub const V0_TOOL_IMPLEMENTATION_VERSION:\s*&str\s*=\s*"1\.0\.0"/.test(production)) {
    violations.push('tool implementation version differs');
  }
  if (!/pub const V0_TOOL_SCHEMA_VERSION:\s*i64\s*=\s*1\s*;/.test(production)) {
    violations.push('tool schema version differs');
  }
  if (/pub\s+fn\s+(?:add|insert|register|remove|replace|load_plugin)\b/.test(production)) {
    violations.push('dynamic registry mutation surface');
  }
  if (!/definitions:\s*Box<\[ToolDefinition\]>/.test(production) || !/fingerprint:\s*Sha256Digest/.test(production)) {
    violations.push('registry is not immutable and fingerprinted');
  }
  if (!/Value::Array\s*\(\s*definitions\s*\.iter\(\)\s*\.map\(ToolDefinition::semantic_value\)/s.test(production)) {
    violations.push('registry fingerprint does not use stable-order semantic definitions');
  }
  if (/fn\s+(?:read_file_definition|run_shell_definition)\b[\s\S]*?(?:SystemTime|Instant|process::id|Arc::as_ptr|as_ptr\s*\()/m.test(production)) {
    violations.push('registry semantic definition contains runtime identity');
  }
  return violations;
}

function verifyStage14CheckerNegativeProbes(handlerSource, registrySource, serviceSource) {
  let probeCount = 0;
  const handlerCases = [
    ['handler journaling', `${handlerSource}\nfn injected() { let _: JournalEventId; }`],
    ['handler SQLx', `${handlerSource}\nfn injected() { sqlx::query("select 1"); }`],
    ['read_file shell fallback', handlerSource.replace('.read_file(request)', '.execute(request)')],
    ['direct filesystem read', `${handlerSource}\nfn injected() { std::fs::read("x"); }`],
    ['run_shell direct Command', `${handlerSource}\nfn injected() { std::process::Command::new("sh"); }`],
  ];
  for (const [label, fixture] of handlerCases) {
    assert(
      stage14HandlerViolations(fixture).length > 0,
      `checker negative probe was not rejected: ${label}`,
    );
    probeCount += 1;
  }
  const registryCases = [
    ['dynamic registry mutation', `${registrySource}\nimpl ToolRegistry { pub fn register(&mut self) {} }`],
    [
      'third production tool',
      registrySource.replace(
        'vec![\n            read_file_definition(policy),\n            run_shell_definition(policy),\n        ]',
        'vec![\n            read_file_definition(policy),\n            run_shell_definition(policy),\n            browser_definition(policy),\n        ]',
      ),
    ],
  ];
  for (const [label, fixture] of registryCases) {
    assert(
      stage14RegistryViolations(fixture).length > 0,
      `checker negative probe was not rejected: ${label}`,
    );
    probeCount += 1;
  }
  const serviceCases = [
    ['Workstation call before dispatch commit', serviceSource.replace(
      'let dispatch_at = self.wall_now()?;',
      'let _ = self.workstation.read_file(panic!("checker probe")).await; let dispatch_at = self.wall_now()?;',
    )],
    ['direct Workstation action plus handler', serviceSource.replace(
      'let handler = resolve_handler(definition.handler());',
      'let _ = self.workstation.execute(panic!("checker probe")).await; let handler = resolve_handler(definition.handler());',
    )],
    ['duplicate handler handoff', serviceSource.replace(
      'handler.invoke(',
      'handler.invoke(arguments.input(), handler_context.clone(), self.workstation.as_ref()); handler.invoke(',
    )],
  ];
  for (const [label, fixture] of serviceCases) {
    assert(stage14ServiceViolations(fixture).length > 0, `checker negative probe was not rejected: ${label}`);
    probeCount += 1;
  }
  const helperPreDispatchRead = serviceSource
    .replace(
      'let dispatch_at = self.wall_now()?;',
      'helper_pre_read(self.workstation.as_ref()).await; let dispatch_at = self.wall_now()?;',
    )
    .concat('\nasync fn helper_pre_read(machine: &dyn Workstation) { let _ = machine.read_file(todo!()).await; }\n');
  assert(
    stage14ServiceViolations(helperPreDispatchRead).length > 0,
    'checker negative probe was not rejected: helper-mediated pre-dispatch read',
  );
  probeCount += 1;
  const helperPreDispatchExecute = serviceSource
    .replace(
      'let dispatch_at = self.wall_now()?;',
      'helper_pre_execute(self.workstation.as_ref()).await; let dispatch_at = self.wall_now()?;',
    )
    .concat('\nasync fn helper_pre_execute(renamed_machine: &dyn Workstation) { let _ = renamed_machine.execute(todo!()).await; }\n');
  assert(
    stage14ServiceViolations(helperPreDispatchExecute).length > 0,
    'checker negative probe was not rejected: helper-mediated pre-dispatch execute with renamed receiver',
  );
  probeCount += 1;
  const directReadAndHandler = serviceSource.replace(
    'let handler = resolve_handler(definition.handler());',
    'let _ = self.workstation.read_file(panic!("checker probe")).await; let handler = resolve_handler(definition.handler());',
  );
  assert(
    stage14ServiceViolations(directReadAndHandler).length > 0,
    'checker negative probe was not rejected: direct service read plus normal handler',
  );
  probeCount += 1;
  const helperReadAndHandler = serviceSource
    .replace(
      'let handler = resolve_handler(definition.handler());',
      'helper_after_dispatch(self.workstation.as_ref()).await; let handler = resolve_handler(definition.handler());',
    )
    .concat('\nasync fn helper_after_dispatch(machine: &dyn Workstation) { let _ = machine.read_file(todo!()).await; }\n');
  assert(
    stage14ServiceViolations(helperReadAndHandler).length > 0,
    'checker negative probe was not rejected: helper read plus normal handler',
  );
  probeCount += 1;
  const recursiveRetry = serviceSource
    .replace(
      'let handler = resolve_handler(definition.handler());',
      'recursive_retry(self.workstation.as_ref()).await; let handler = resolve_handler(definition.handler());',
    )
    .concat(`
      fn recursive_retry<'a>(machine: &'a dyn Workstation) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
          let _ = machine.read_file(todo!()).await;
          recursive_retry(machine).await;
        })
      }
    `);
  assert(
    stage14ServiceViolations(recursiveRetry).length > 0,
    'checker negative probe was not rejected: recursive helper retry',
  );
  probeCount += 1;
  const loopRetry = serviceSource
    .replace(
      'let handler = resolve_handler(definition.handler());',
      'loop_retry(self.workstation.as_ref()).await; let handler = resolve_handler(definition.handler());',
    )
    .concat('\nasync fn loop_retry(machine: &dyn Workstation) { loop { let _ = machine.execute(todo!()).await; } }\n');
  assert(
    stage14ServiceViolations(loopRetry).length > 0,
    'checker negative probe was not rejected: loop helper retry',
  );
  probeCount += 1;
  const deadlineReconstruction = serviceSource.replace(
    'let handler = resolve_handler(definition.handler());',
    'let reconstructed_deadline = std::time::Instant::now() + Duration::from_millis(effective_timeout_ms); let handler = resolve_handler(definition.handler());',
  );
  assert(
    stage14ServiceViolations(deadlineReconstruction).length > 0,
    'checker negative probe was not rejected: post-freeze deadline reconstruction',
  );
  probeCount += 1;
  const duplicateRead = handlerSource.replace(
    'workstation\n                .read_file(request)',
    'workstation.read_file(request.clone()).await?; workstation.read_file(request)',
  );
  assert(stage14HandlerViolations(duplicateRead).length > 0, 'checker negative probe was not rejected: duplicate direct read_file');
  probeCount += 1;
  const duplicateExecute = handlerSource.replace(
    'workstation\n                .execute(request)',
    'workstation.execute(request.clone()).await?; workstation.execute(request)',
  );
  assert(stage14HandlerViolations(duplicateExecute).length > 0, 'checker negative probe was not rejected: duplicate direct execute');
  probeCount += 1;
  const helperRetry = handlerSource
    .replace('workstation\n                .read_file(request)', 'read_once(workstation, request.clone()).await?; read_once(workstation, request)')
    .concat('\nasync fn read_once(workstation: &dyn Workstation, request: FileReadRequest) -> Result<FileReadResult, WorkstationError> { workstation.read_file(request).await }\n');
  assert(stage14HandlerViolations(helperRetry).length > 0, 'checker negative probe was not rejected: helper-mediated second Workstation action');
  probeCount += 1;

  const harmlessHandlerChain = handlerSource
    .replace('let request = FileReadRequest {', 'harmless_a(); let request = FileReadRequest {')
    .concat('\nfn harmless_a() { harmless_b(); } fn harmless_b() {}\n');
  assert(
    stage14HandlerViolations(harmlessHandlerChain).length === 0,
    'checker false-positive probe rejected harmless handler helpers',
  );
  const harmlessPreDispatch = serviceSource
    .replace(
      'let dispatch_at = self.wall_now()?;',
      'harmless_service_a(); let dispatch_at = self.wall_now()?;',
    )
    .concat('\nfn harmless_service_a() { harmless_service_b(); } fn harmless_service_b() {}\n');
  assert(
    stage14ServiceViolations(harmlessPreDispatch).length === 0,
    'checker false-positive probe rejected harmless pre-dispatch helpers',
  );
  const harmlessRetryName = serviceSource
    .replace(
      'let dispatch_at = self.wall_now()?;',
      'harmless_retry(); let dispatch_at = self.wall_now()?;',
    )
    .concat('\nfn harmless_retry() { let _ = 1_u8.checked_add(1); }\n');
  assert(
    stage14ServiceViolations(harmlessRetryName).length === 0,
    'checker false-positive probe rejected a retry-named helper without machine operations',
  );
  const preparationBeforeDispatch = serviceSource.replace(
    'let dispatch_at = self.wall_now()?;',
    'let _ = self.preparation.prepare(panic!("checker probe")).await; let dispatch_at = self.wall_now()?;',
  );
  assert(
    stage14ServiceViolations(preparationBeforeDispatch).length === 0,
    'checker false-positive probe rejected WorkstationPreparation before dispatch',
  );
  const testOnlyFake = serviceSource.concat(`
    #[cfg(test)]
    mod injected_test_only {
      async fn fake(machine: &dyn Workstation) {
        let _ = machine.read_file(todo!()).await;
        let _ = machine.execute(todo!()).await;
      }
    }
  `);
  assert(
    stage14ServiceViolations(testOnlyFake).length === 0,
    'checker false-positive probe rejected test-only fake Workstation code',
  );
  const unreachableLocalInternals = serviceSource.concat(`
    fn stage13_local_workstation_internal(local: &LocalWorkstation) {
      let _ = local.execute(todo!());
    }
  `);
  assert(
    stage14ServiceViolations(unreachableLocalInternals).length === 0,
    'checker false-positive probe rejected unreachable Stage 13 LocalWorkstation internals',
  );
  const executeRouter = `
    Router::new()
      .route("/health/live", get(liveness))
      .route("/health/ready", get(readiness))
      .route("/bootstrap", get(bootstrap))
      .route("/conversations/{conversation_id}/messages", post(message))
      .route("/work-items/{work_id}/cancel", post(cancel))
      .route("/events", get(events))
      .route("/tools/{name}/execute", post(execute));`;
  expectStructuralRejection('public tool endpoint', () => verifyStage11RouteInventory(executeRouter));
  probeCount += 1;
  assert(
    stage17PlusImplementationLeaks([{ path: 'application/model_gateway.rs', source: 'struct ModelGateway;' }]).length === 1,
    'checker negative probe was not rejected: ModelGateway',
  );
  probeCount += 1;
  assert(
    stage17PlusImplementationLeaks([{ path: 'application/work_runner.rs', source: 'struct WorkRunner;' }]).length === 1,
    'checker negative probe was not rejected: production WorkRunner',
  );
  probeCount += 1;
  assert(
    stage17PlusImplementationLeaks([{ path: 'application/agent_loop.rs', source: 'async fn run_agent_loop() {}' }]).length === 1,
    'checker negative probe was not rejected: agent loop',
  );
  probeCount += 1;
  for (const [label, path, source] of [
    ['ModelGateway', 'ports/model_gateway.rs', 'pub trait ModelGateway {}'],
    ['OpenAI provider client', 'adapters/openai.rs', 'struct OpenAIClient;'],
    ['MCP transport', 'adapters/mcp.rs', 'struct McpTransport;'],
    ['RemoteWorkstation', 'adapters/remote.rs', 'struct RemoteWorkstation;'],
    ['dynamic plugin registration', 'application/plugins.rs', 'fn register_plugins() {}'],
  ]) {
    assert(stage17PlusImplementationLeaks([{ path, source }]).length === 1, `checker negative probe was not rejected: ${label}`);
    probeCount += 1;
  }
  const failpointIsGated = (source) =>
    !/\breach\s*\(/.test(source) ||
    /#\[cfg\(feature = "test-failpoints"\)\][\s\S]{0,120}\breach\s*\(/.test(source);
  assert(
    failpointIsGated('#[cfg(feature = "test-failpoints")] fn call() { reach(); }') &&
      !failpointIsGated('fn call() { reach(); }'),
    'checker negative probe was not rejected: failpoint release leakage',
  );
  probeCount += 1;
  return probeCount;
}

function verifyStage14ToolStructure(rustRoot, productionFiles) {
  const applicationRoot = join(rustRoot, 'application');
  const portsRoot = join(rustRoot, 'ports');
  const registry = readFileSync(join(applicationRoot, 'tool_registry.rs'), 'utf8');
  const handlers = readFileSync(join(applicationRoot, 'tool_handlers.rs'), 'utf8');
  const authority = readFileSync(join(applicationRoot, 'authority.rs'), 'utf8');
  const service = readFileSync(join(applicationRoot, 'tool_execution_service.rs'), 'utf8');
  const preparation = readFileSync(join(portsRoot, 'workstation_preparation.rs'), 'utf8');
  const bootstrap = readFileSync(join(rustRoot, 'bootstrap', 'startup.rs'), 'utf8');
  const failpoints = readFileSync(join(rustRoot, 'test_failpoints.rs'), 'utf8');
  const artifactStore = readFileSync(join(rustRoot, 'adapters', 'artifacts', 'local.rs'), 'utf8');
  const execution = readFileSync(join(rustRoot, 'adapters', 'local_workstation', 'execution.rs'), 'utf8');
  const stage8Tests = readFileSync(join(rustRoot, 'adapters', 'sqlite', 'stage8_tests.rs'), 'utf8');

  assert(stage14RegistryViolations(registry).length === 0, `Stage 14 registry invariant differs: ${stage14RegistryViolations(registry).join(', ')}`);
  assert(stage14HandlerViolations(handlers).length === 0, `Stage 14 handler boundary differs: ${stage14HandlerViolations(handlers).join(', ')}`);
  assert(stage14ServiceViolations(service).length === 0, `Stage 14 service dispatch boundary differs: ${stage14ServiceViolations(service).join(', ')}`);
  assert(/pub trait AuthorityEvaluator:\s*Send\s*\+\s*Sync/.test(authority), 'Stage 14 typed authority evaluator seam is absent');
  assert(authority.includes('v0-development-workstation'), 'Stage 14 stable authority policy name differs');
  for (const reason of [
    'unregistered_tool', 'malformed_arguments', 'cancelled_work', 'authority_widening',
    'wrong_workstation', 'stale_generation', 'wrong_workspace', 'unsupported_capability',
    'administrative_unavailable', 'limit_exceeded',
  ]) {
    assert(authority.includes(`"${reason}"`), `Stage 14 authority reason is absent: ${reason}`);
  }
  const preparationTrait = extractRustNamedBlock(
    preparation,
    /pub\s+trait\s+WorkstationPreparation\b/,
    'WorkstationPreparation trait',
  );
  assert(equalStringArrays(rustMethodNames(preparationTrait), ['prepare']), 'Stage 14 preparation seam must expose exactly prepare');
  assert(/spawn_blocking/.test(readFileSync(join(rustRoot, 'adapters', 'local_workstation.rs'), 'utf8')), 'LocalWorkstation preparation must resolve adapter-observed cwd');

  const productionService = stripRustComments(withoutRustTestModules(service));
  const deadlineFreeze = productionService.indexOf('freeze_tool_deadline(');
  const requested = productionService.indexOf('.request_tool_execution(');
  const requestedHook = productionService.indexOf('AfterToolRequestedCommit');
  const capabilities = productionService.indexOf('.capabilities(');
  const prepared = productionService.indexOf('.prepare(');
  const dispatch = productionService.indexOf('.commit_tool_dispatch_intent(');
  const dispatchHook = productionService.indexOf('AfterToolDispatchIntentCommit');
  const handler = productionService.indexOf('handler.invoke(');
  assert(
    deadlineFreeze !== -1 && requested > deadlineFreeze && requestedHook > requested && capabilities > requestedHook && prepared > capabilities &&
      dispatch > prepared && dispatchHook > dispatch && handler > dispatchHook,
    'Stage 14 deadline/requested/preparation/dispatch/Workstation ordering differs',
  );
  assert([...productionService.matchAll(/handler\.invoke\s*\(/g)].length === 1, 'Stage 14 service may hand off to a handler exactly once');
  for (const operation of ['request_tool_execution', 'commit_tool_dispatch_intent', 'finish_tool_execution']) {
    assert(productionService.includes(`.${operation}(`), `ToolExecutionService does not own ${operation}`);
  }
  assert(!/tokio::process|std::process::Command|Command::new|(?:std|tokio)::fs/.test(productionService), 'ToolExecutionService bypasses Workstation or ArtifactStore');
  assert(/CatchHandlerPanic/.test(productionService) && /persist_outcome_unknown/.test(productionService), 'Stage 14 conservative handler panic boundary is absent');
  assert(
    /effective_outer_deadline/.test(productionService) && /freeze_tool_deadline/.test(productionService) &&
      !/effective_timeout_and_deadline/.test(productionService),
    'Stage 14 frozen minimum deadline composition is absent',
  );
  assert(
    /PreparedCwdEvidence/.test(preparation) && /PreparedCwdObjectIdentity/.test(preparation) &&
      /device/.test(preparation) && /inode/.test(preparation),
    'Stage 14 stable cwd object-identity evidence is absent',
  );
  const crashWorker = extractRustFunction(stage8Tests, 'stage14_crash_window_child');
  assert(
    /ToolExecutionService::new\s*\(/.test(crashWorker) &&
      /LocalWorkstation::new\s*\(/.test(crashWorker) &&
      /\.execute_call\s*\(/.test(crashWorker) &&
      !/test_failpoints\s*::\s*reach\s*\(/.test(crashWorker),
    'Stage 14 crash worker must reach failpoints only through the production service lifecycle',
  );

  for (const construction of ['ToolRegistry::v0(ToolSemanticPolicy', 'V0AuthorityEvaluator', 'ToolExecutionService::new(']) {
    assert(bootstrap.includes(construction), `Stage 14 bootstrap composition is absent: ${construction}`);
  }
  for (const hook of [
    'AfterToolRequestedCommit', 'AfterToolDispatchIntentCommit', 'AfterToolProcessSpawn',
    'AfterToolProcessExitBeforeOutcomeCommit', 'AfterArtifactRenameBeforeDbCommit',
  ]) {
    assert(failpoints.includes(hook), `Stage 14 failpoint inventory is absent: ${hook}`);
  }
  assert(/#\[cfg\(feature = "test-failpoints"\)\][\s\S]{0,160}AfterArtifactRenameBeforeDbCommit/.test(artifactStore), 'artifact rename failpoint is not feature-gated');
  assert(/#\[cfg\(feature = "test-failpoints"\)\][\s\S]{0,160}AfterToolProcessSpawn/.test(execution), 'process-spawn failpoint is not feature-gated');
  const finishShell = extractRustFunction(productionService, 'finish_run_shell');
  const cleanupKnown = finishShell.indexOf('result.execution_id != execution_id');
  const exitHook = finishShell.indexOf('AfterToolProcessExitBeforeOutcomeCommit');
  const outcomeCommit = finishShell.indexOf('.finish_tool_execution(');
  assert(
    /#\[cfg\(feature = "test-failpoints"\)\][\s\S]{0,180}AfterToolProcessExitBeforeOutcomeCommit/.test(finishShell) &&
      cleanupKnown !== -1 && exitHook > cleanupKnown && outcomeCommit > exitHook,
    'process-exit failpoint is not after terminal cleanup classification and before outcome commit',
  );

  for (const testName of [
    'v0_registry_inventory_order_versions_and_lookup_are_exact',
    'duplicate_registry_is_rejected_and_fingerprint_is_deterministic_and_sensitive',
    'duplicate_keys_are_rejected_recursively_before_typed_decode',
    'read_file_orders_both_commits_before_machine_action_and_commits_result_before_return',
    'validation_unknown_admin_and_request_failure_never_dispatch',
    'run_shell_maps_definite_result_classes_without_retry',
    'run_shell_effective_deadline_is_the_minimum_and_never_widens_privilege_or_cwd',
    'requested_capability_preparation_and_dispatch_all_consume_the_frozen_budget',
    'deadline_expiry_during_requested_persistence_prevents_capability_and_machine_action',
    'deadline_expiry_during_capability_acquisition_prevents_preparation_and_dispatch',
    'deadline_expiry_during_preparation_prevents_dispatch_and_machine_action',
    'active_shutdown_absolute_deadline_shortens_the_same_frozen_deadline',
    'dispatch_failure_and_outcome_failure_do_not_repeat_machine_action',
    'duplicate_logical_call_is_rejected_before_a_second_dispatch',
    'cleanup_ambiguity_and_handler_panic_commit_outcome_unknown_without_redispatch',
    'handler_panic_before_handoff_is_caught_without_a_workstation_call',
    'cancellation_before_intent_requested_and_active_execution_are_distinct',
    'large_read_is_generic_artifact_backed_without_stream_column_reuse',
    'artifact_finalization_failure_after_handoff_is_durable_outcome_unknown',
    'emitted_schema_evaluator_matches_typed_decoder_boundary_matrix',
    'emitted_schema_mutations_prove_evaluator_decoder_independence',
    'crash_after_tool_requested_commit_recovers_without_redispatch',
    'crash_after_tool_dispatch_intent_commit_recovers_outcome_unknown',
    'crash_after_tool_process_spawn_records_one_side_effect',
    'crash_after_tool_process_exit_preserves_definite_observation_marker',
    'crash_after_artifact_rename_recovers_and_reports_one_orphan',
  ]) {
    assert(new RegExp(`(?:async\\s+)?fn\\s+${testName}\\s*\\(`).test(`${registry}\n${service}\n${stage8Tests}`), `Stage 14 permanent test inventory is missing ${testName}`);
  }
  return verifyStage14CheckerNegativeProbes(handlers, registry, service);
}

function stage13ProcessBoundaryLeaks(productionFiles) {
  return productionFiles
    .filter((file) => {
      const source = stripRustComments(withoutRustTestModules(file.source));
      const ownsProcesses = /tokio::process|std::process::Command|Command::new|\bkillpg\s*\(|\bsetsid\s*\(|cgroup\.kill|cgroup\.procs/.test(source);
      const localBoundary = file.path === 'adapters/local_workstation.rs' ||
        file.path.startsWith('adapters/local_workstation/');
      return ownsProcesses && !localBoundary;
    })
    .map((file) => file.path)
    .sort();
}

function extractRustNamedBlock(source, pattern, label) {
  const match = pattern.exec(source);
  assert(match, `${label} is absent`);
  const opening = source.indexOf('{', match.index);
  assert(opening !== -1, `${label} has no body`);
  const closing = findMatchingDelimiter(source, opening, '{', '}');
  assert(closing !== -1, `${label} has an unbalanced body`);
  return source.slice(match.index, closing + 1);
}

function rustMethodNames(source) {
  return [...source.matchAll(/\bfn\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(/g)].map((match) => match[1]);
}

function stage12ModelFilesystemLeaks(productionFiles) {
  const leaks = [];
  for (const file of productionFiles) {
    if (!/^(?:application|domain|ports)\//.test(file.path)) continue;
    const source = stripRustComments(withoutRustTestModules(file.source));
    if (
      /\b(?:std|tokio)::fs\b|\b(?:File|OpenOptions)::open\s*\(|\.read_to_(?:end|string)\s*\(/.test(
        source,
      )
    ) {
      leaks.push(file.path);
    }
  }
  return sortedStrings(new Set(leaks));
}

function stage13ExecutionViolations(source) {
  const production = stripRustComments(withoutRustTestModules(source));
  const violations = [];
  if (/Command::new\s*\(/.test(production) && !/\.env_clear\s*\(\)/.test(production)) {
    violations.push('ambient environment inheritance');
  }
  if (/\.read_to_(?:end|string)\s*\(/.test(production)) {
    violations.push('unbounded output buffering');
  }
  if (/\.kill\s*\(\s*\)\s*\.await/.test(production) && !/killpg|cgroup\.kill/.test(production)) {
    violations.push('parent-only kill');
  }
  if (/tokio::spawn\s*\(/.test(production) && !/(?:let\s+\w+\s*=|join\s*:\s*Some\s*\()\s*tokio::spawn\s*\(/.test(production)) {
    violations.push('detached lifecycle task');
  }
  if (/ExecutionId[^;\n]*(?:pid|pgid)|ExecutionId::[^;\n]*(?:pid|pgid)/i.test(production)) {
    violations.push('PID-as-ExecutionId');
  }
  if (/(?:\.env|command\.arg)\s*\([^;]*(?:OPENAI_API_KEY|ANTHROPIC_API_KEY|AWS_SECRET_ACCESS_KEY|SSH_AUTH_SOCK)/.test(production)) {
    violations.push('provider or ambient secret inclusion');
  }
  if (/tracing::(?:trace|debug|info|warn|error)!\([^;]*(?:request\.command|resolved_cwd|cgroup_path|\bstdout\s*=|\bstderr\s*=|\bcommand\s*=)/s.test(production)) {
    violations.push('raw sensitive tracing');
  }
  if (/privilege_administrative:\s*true|cgroup_cleanup:\s*true/.test(production)) {
    violations.push('optimistic privileged capability');
  }
  return violations;
}

function stage13DeadlinePropagationViolations(startup, execution) {
  const violations = [];
  if (
    !/let\s+deadline\s*=\s*self\.shutdown\.latch_shutdown_request\(\)/.test(startup) ||
    !/begin_execution_shutdown\(deadline\)/.test(startup) ||
    !/shutdown_executions_before\(deadline\)/.test(startup)
  ) {
    violations.push('Stage 10 deadline not propagated unchanged');
  }
  if (
    !/fn\s+effective_cleanup_remaining\s*\(/.test(execution) ||
    !/request_remaining\.min\(shutdown_remaining\)/.test(execution) ||
    !/finish_owned_process_tree[\s\S]*effective_cleanup_remaining\(request_deadline\)/.test(execution)
  ) {
    violations.push('request-only cleanup deadline');
  }
  return violations;
}

function stage13OwnershipBoundaryViolations(execution) {
  const violations = [];
  const execute = extractRustFunction(execution, 'execute');
  const shutdown = extractRustFunction(execution, 'begin_shutdown');
  const manager = extractRustFunction(execution, 'manager_loop');
  const supervise = extractRustFunction(execution, 'supervise');
  const superviseInner = extractRustFunction(execution, 'supervise_inner');
  const claimSpawn = extractRustFunction(execution, 'claim_spawn');
  const reservationScopeMatch = /\{\s*let\s+mut\s+registry\s*=\s*lock\(&self\.registry\)/.exec(execute);
  let reservationScope = '';
  let reservationScopeEnd = -1;
  if (reservationScopeMatch) {
    const opening = execute.indexOf('{', reservationScopeMatch.index);
    const closing = findMatchingDelimiter(execute, opening, '{', '}');
    if (closing !== -1) {
      reservationScope = execute.slice(opening, closing + 1);
      reservationScopeEnd = closing;
    }
  }
  const reservation = reservationScope.search(/registry\s*\.entries\s*\.insert\s*\(/);
  const admissionRejection = reservationScope.lastIndexOf('WorkstationUnavailable');
  const duplicateRejection = reservationScope.search(/registry\s*\.entries\s*\.contains_key\s*\(/);
  if (
    !/struct\s+ExecutionRegistry\s*\{[\s\S]*admission_open:\s*bool[\s\S]*entries:\s*HashMap<ExecutionId, Arc<ExecutionEntry>>/.test(execution) ||
    !/if\s+!registry\.admission_open/.test(reservationScope) ||
    reservation === -1 ||
    admissionRejection === -1 ||
    admissionRejection > reservation ||
    duplicateRejection === -1 ||
    duplicateRejection > reservation
  ) {
    violations.push('reservation not atomic ownership boundary');
  }
  if (
    /admission_open|WorkstationUnavailable/.test(`${manager}\n${superviseInner}`) ||
    !/fn\s+claim_spawn\s*\(/.test(execution) ||
    !/pre_spawn_terminal_result/.test(execution)
  ) {
    violations.push('post-reservation admission rejection');
  }
  const managerOwnershipRelease = execute.indexOf('self.ensure_manager()');
  const managerDispatch = execute.search(/\.send\s*\(\s*ManagerCommand::Launch\s*\(/);
  const supervisorDispatch = manager.search(/supervisors\s*\.spawn\s*\(\s*supervise\s*\(/);
  const supervisorHandoff = supervise.search(/supervise_inner\s*\(\s*&runtime\s*,\s*launch\s*\)/);
  const spawnClaim = superviseInner.search(/launch\s*\.entry\s*\.claim_spawn\s*\(\s*\)/);
  const processSpawn = superviseInner.search(/\bcommand\s*\.spawn\s*\(\s*\)/);
  const registryInsertions = execution.match(/registry\s*\.entries\s*\.insert\s*\(/g) ?? [];
  const processSpawns = execution.match(/\bcommand\s*\.spawn\s*\(\s*\)/g) ?? [];
  if (
    reservation === -1 ||
    reservationScopeEnd === -1 ||
    managerOwnershipRelease < reservationScopeEnd ||
    managerDispatch < managerOwnershipRelease ||
    supervisorDispatch === -1 ||
    supervisorHandoff === -1 ||
    spawnClaim === -1 ||
    processSpawn < spawnClaim ||
    registryInsertions.length !== 1 ||
    processSpawns.length !== 1 ||
    /\bcommand\s*\.spawn\s*\(/.test(`${execute}\n${manager}`) ||
    !/ExecutionPhase::Reserved/.test(claimSpawn) ||
    !/lifecycle\.phase\s*=\s*ExecutionPhase::Spawning/.test(claimSpawn)
  ) {
    violations.push('execution ownership reservation does not precede OS process spawn');
  }
  const lockIndex = shutdown.indexOf('lock(&self.registry)');
  const closeIndex = shutdown.indexOf('registry.admission_open = false');
  const latchIndex = shutdown.indexOf('entry.latch(TerminalCause::Shutdown)');
  if (
    lockIndex === -1 ||
    closeIndex < lockIndex ||
    latchIndex < closeIndex ||
    !/registry\.entries\.values\s*\(\)/.test(shutdown)
  ) {
    violations.push('shutdown does not atomically close and latch reserved entries');
  }
  return violations;
}

const STAGE13_EINTR_HELPER_MAX_DEPTH = 16;

function rustTopLevelFunctions(source) {
  const functions = new Map();
  let depth = 0;
  let quote = null;
  let escaped = false;

  for (let index = 0; index < source.length; index += 1) {
    const character = source[index];
    const next = source[index + 1];
    if (quote !== null) {
      if (escaped) escaped = false;
      else if (character === '\\') escaped = true;
      else if (character === quote) quote = null;
      continue;
    }
    if (
      character === '"' ||
      (character === "'" && (source[index + 2] === "'" || (next === '\\' && source[index + 3] === "'")))
    ) {
      quote = character;
      continue;
    }
    if (character === '{') {
      depth += 1;
      continue;
    }
    if (character === '}') {
      depth -= 1;
      continue;
    }
    if (
      depth !== 0 ||
      !source.startsWith('fn', index) ||
      /[A-Za-z0-9_]/.test(source[index - 1] ?? '') ||
      !/\s/.test(source[index + 2] ?? '')
    ) {
      continue;
    }
    const signature = /^fn\s+([A-Za-z_][A-Za-z0-9_]*)\b/.exec(source.slice(index));
    if (!signature) continue;
    const opening = source.indexOf('{', index + signature[0].length);
    assert(opening !== -1, `Rust function ${signature[1]} has no body`);
    const closing = findMatchingDelimiter(source, opening, '{', '}');
    assert(closing !== -1, `Rust function ${signature[1]} has an unbalanced body`);
    const definitions = functions.get(signature[1]) ?? [];
    definitions.push({
      source: source.slice(index, closing + 1),
      body: source.slice(opening + 1, closing),
    });
    functions.set(signature[1], definitions);
    index = closing;
  }
  return functions;
}

function rustLocalFunctionCalls(source, localFunctionNames) {
  const calls = new Set();
  for (const match of source.matchAll(/\b([A-Za-z_][A-Za-z0-9_]*)\s*\(/g)) {
    if (localFunctionNames.has(match[1])) calls.add(match[1]);
  }
  return calls;
}

function callsConcreteWaitidObserver(source) {
  if (
    /\bself\s*\.\s*observe\s*\(/.test(source) ||
    /<\s*(?:(?:crate|self|super|[A-Za-z_][A-Za-z0-9_]*)\s*::\s*)*WaitIdLeaderObserver\s+as\s+(?:(?:crate|self|super|[A-Za-z_][A-Za-z0-9_]*)\s*::\s*)*LeaderObserver\s*>\s*::\s*observe\s*\(/.test(source) ||
    /<\s*Self\s+as\s+(?:(?:crate|self|super|[A-Za-z_][A-Za-z0-9_]*)\s*::\s*)*LeaderObserver\s*>\s*::\s*observe\s*\(/.test(source) ||
    /\bWaitIdLeaderObserver\s*(?:::|\.)\s*observe\s*\(/.test(source) ||
    /\bLeaderObserver\s*::\s*observe\s*\(\s*self\b/.test(source)
  ) {
    return true;
  }

  const concreteReceivers = new Set();
  for (const match of source.matchAll(
    /\b([A-Za-z_][A-Za-z0-9_]*)\s*:\s*&?\s*(?:'[A-Za-z_][A-Za-z0-9_]*\s+)?(?:mut\s+)?(?:(?:crate|self|super|[A-Za-z_][A-Za-z0-9_]*)\s*::\s*)*WaitIdLeaderObserver\b/g,
  )) {
    concreteReceivers.add(match[1]);
  }
  for (const match of source.matchAll(
    /\blet\s+(?:mut\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*&?\s*(?:(?:crate|self|super|[A-Za-z_][A-Za-z0-9_]*)\s*::\s*)*WaitIdLeaderObserver\b/g,
  )) {
    concreteReceivers.add(match[1]);
  }
  return [...concreteReceivers].some((receiver) =>
    new RegExp(
      `(?:\\b${receiver}\\s*\\.\\s*observe|\\bLeaderObserver\\s*::\\s*observe\\s*\\(\\s*&?\\s*${receiver}\\b)`,
    ).test(source),
  );
}

function stage13EintrPath(normalize) {
  const condition = /\bif\s+(?:error\s*==\s*nix::libc::EINTR|nix::libc::EINTR\s*==\s*error)\s*\{/.exec(normalize);
  if (!condition) return null;
  const opening = condition.index + condition[0].lastIndexOf('{');
  const closing = findMatchingDelimiter(normalize, opening, '{', '}');
  assert(closing !== -1, 'Stage 13 EINTR normalization branch has unbalanced braces');
  const errorNormalization = normalize.lastIndexOf('let error', condition.index);
  return {
    branch: normalize.slice(opening + 1, closing),
    source: normalize.slice(errorNormalization === -1 ? condition.index : errorNormalization, closing + 1),
  };
}

function stage13EintrHelperAnalysis(execution, observe, normalize, eintrPath) {
  const localFunctions = rustTopLevelFunctions(execution);
  const localFunctionNames = new Set(localFunctions.keys());
  const observerCalls = rustLocalFunctionCalls(observe, localFunctionNames);
  observerCalls.delete('normalize_waitid_attempt');
  const eintrCalls = rustLocalFunctionCalls(eintrPath?.source ?? '', localFunctionNames);
  const initialCalls = new Set([...observerCalls, ...eintrCalls]);
  initialCalls.delete('normalize_waitid_attempt');

  const visited = new Set();
  const edges = new Map();
  const reachableSources = [];
  const queue = [...initialCalls].map((name) => ({ name, depth: 1 }));
  let dangerousRetry = false;
  let normalizationCycle = eintrCalls.has('normalize_waitid_attempt');
  let depthExceeded = false;

  while (queue.length > 0) {
    const { name, depth } = queue.shift();
    if (visited.has(name)) continue;
    visited.add(name);
    const definitions = localFunctions.get(name) ?? [];
    const callees = new Set();
    for (const definition of definitions) {
      reachableSources.push(definition.source);
      if (
        /\bwaitid\s*\(/.test(definition.source) ||
        callsConcreteWaitidObserver(definition.source)
      ) {
        dangerousRetry = true;
      }
      for (const callee of rustLocalFunctionCalls(definition.body, localFunctionNames)) {
        if (callee === 'normalize_waitid_attempt') normalizationCycle = true;
        else callees.add(callee);
      }
    }
    edges.set(name, callees);
    for (const callee of callees) {
      if (visited.has(callee)) continue;
      if (depth >= STAGE13_EINTR_HELPER_MAX_DEPTH) {
        depthExceeded = true;
      } else {
        queue.push({ name: callee, depth: depth + 1 });
      }
    }
  }

  const indegree = new Map([...visited].map((name) => [name, 0]));
  for (const [caller, callees] of edges) {
    if (!visited.has(caller)) continue;
    for (const callee of callees) {
      if (visited.has(callee)) indegree.set(callee, indegree.get(callee) + 1);
    }
  }
  const acyclicQueue = [...indegree]
    .filter(([, degree]) => degree === 0)
    .map(([name]) => name);
  let acyclicCount = 0;
  while (acyclicQueue.length > 0) {
    const name = acyclicQueue.shift();
    acyclicCount += 1;
    for (const callee of edges.get(name) ?? []) {
      if (!indegree.has(callee)) continue;
      const degree = indegree.get(callee) - 1;
      indegree.set(callee, degree);
      if (degree === 0) acyclicQueue.push(callee);
    }
  }

  return {
    cooperativeResult: [eintrPath?.branch ?? '', ...reachableSources].some((source) =>
      /(?:return\s+)?Ok\s*\(\s*LeaderObservationStatus::Interrupted\s*\)/.test(source),
    ),
    synchronousRetry:
      dangerousRetry || normalizationCycle || depthExceeded || acyclicCount !== visited.size,
  };
}

function stage13WaitidEintrViolations(execution) {
  const violations = [];
  const observerImpl = extractRustNamedBlock(
    execution,
    /impl\s+LeaderObserver\s+for\s+WaitIdLeaderObserver\s*\{/,
    'Stage 13 waitid leader observer',
  );
  const observe = extractRustFunction(observerImpl, 'observe');
  const normalize = extractRustFunction(execution, 'normalize_waitid_attempt');
  const eintrPath = stage13EintrPath(normalize);
  const helperAnalysis = stage13EintrHelperAnalysis(execution, observe, normalize, eintrPath);
  const cooperativeEintrResult = helperAnalysis.cooperativeResult;
  const observerReturnsNormalizer =
    /normalize_waitid_attempt\s*\(\s*result\s*,\s*observed_pid\s*,\s*error\s*\)\s*\}$/.test(observe);
  if (
    (observe.match(/\bwaitid\s*\(/g) ?? []).length !== 1 ||
    !/\bWNOWAIT\b/.test(observe) ||
    /\bloop\s*\{|\bcontinue\b|\bEINTR\b/.test(observe) ||
    callsConcreteWaitidObserver(observe) ||
    !observerReturnsNormalizer ||
    /\bwaitid\s*\(|\bloop\s*\{|\bcontinue\b/.test(normalize) ||
    callsConcreteWaitidObserver(normalize) ||
    (normalize.match(/\bnix::libc::EINTR\b/g) ?? []).length !== 1 ||
    !cooperativeEintrResult ||
    helperAnalysis.synchronousRetry
  ) {
    violations.push('unbounded synchronous EINTR retry');
  }
  if (
    !observerReturnsNormalizer ||
    !cooperativeEintrResult
  ) {
    violations.push('EINTR is not normalized cooperatively');
  }
  if (
    !/nix::libc::EINTR[\s\S]*LeaderObservationStatus::Interrupted/.test(execution) ||
    !/Pending\s*\|\s*LeaderObservationStatus::Interrupted/.test(execution) ||
    !/tokio::select!/.test(execution) ||
    !/GROUP_POLL_INTERVAL/.test(execution)
  ) {
    violations.push('interrupted observation bypasses async deadline control');
  }
  return violations;
}

function stage13StableGroupOrderingViolations(execution, finishOwnedTree) {
  const violations = [];
  const release = finishOwnedTree.indexOf('release_for_reap()');
  const tryReap = finishOwnedTree.indexOf('child.try_wait()');
  const finalSignal = finishOwnedTree.lastIndexOf('signal_owned_tree(');
  if (
    !/waitid\s*\(/.test(execution) ||
    !/WNOWAIT/.test(execution) ||
    !/struct\s+StableProcessGroup\b/.test(execution)
  ) {
    violations.push('leader exit observed by reaping');
  }
  if (release === -1 || tryReap === -1 || finalSignal === -1 || finalSignal > release || release > tryReap) {
    violations.push('stale PGID signal after leader identity release');
  }
  return violations;
}

function verifyStage13WorkstationStructure(rustRoot, productionFiles) {
  const portPath = join(rustRoot, 'ports', 'workstation.rs');
  const adapterPath = join(rustRoot, 'adapters', 'local_workstation.rs');
  const executionPath = join(rustRoot, 'adapters', 'local_workstation', 'execution.rs');
  assert(existsSync(portPath), 'Stage 13 Workstation port is absent');
  assert(existsSync(adapterPath), 'Stage 13 LocalWorkstation adapter is absent');
  assert(existsSync(executionPath), 'Stage 13 owned execution runtime is absent');

  const port = readFileSync(portPath, 'utf8');
  const adapter = readFileSync(adapterPath, 'utf8');
  const execution = readFileSync(executionPath, 'utf8');
  const portProduction = stripRustComments(withoutRustTestModules(port));
  const adapterProduction = stripRustComments(withoutRustTestModules(adapter));
  const executionProduction = stripRustComments(withoutRustTestModules(execution));
  const workstationTraits = productionFiles.reduce(
    (count, file) => count + (stripRustComments(withoutRustTestModules(file.source)).match(/\btrait\s+Workstation\b/g) ?? []).length,
    0,
  );
  const localWorkstationStructs = productionFiles.reduce(
    (count, file) => count + (stripRustComments(withoutRustTestModules(file.source)).match(/\bstruct\s+LocalWorkstation\b/g) ?? []).length,
    0,
  );
  assert(workstationTraits === 1, `expected one Workstation port, found ${workstationTraits}`);
  assert(
    localWorkstationStructs === 1,
    `expected one production LocalWorkstation, found ${localWorkstationStructs}`,
  );

  const trait = extractRustNamedBlock(
    portProduction,
    /\bpub\s+trait\s+Workstation\s*:[^{]+\{/,
    'Stage 13 Workstation trait',
  );
  assert(
    equalStringArrays(rustMethodNames(trait), [
      'capabilities',
      'read_file',
      'execute',
      'inspect_execution',
      'cancel_execution',
    ]),
    `Workstation methods differ: ${rustMethodNames(trait).join(', ')}`,
  );
  assert(
    !/\b(?:PathBuf|RawFd|OwnedFd|BorrowedFd|DiagnosticPid|Pid)\b|\b(?:std::path|std::fs|std::process|tokio|sqlx|axum)::/.test(
      portProduction,
    ),
    'Workstation port exposes a local filesystem, process, runtime, SQLx, or Axum type',
  );
  for (const required of [
    'OperationId',
    'WorkstationId',
    'WorkstationGeneration',
    'WorkspaceId',
    'ExecutionId',
    'WorkId',
    'LogicalPathReference',
    'WorkstationFuture',
  ]) {
    assert(portProduction.includes(required), `Workstation port is missing ${required}`);
  }
  for (const code of [
    'workstation_unavailable',
    'generation_mismatch',
    'workspace_not_found',
    'invalid_path',
    'not_found',
    'permission_denied',
    'binary_content',
    'file_too_large',
    'changed_during_read',
    'unsupported_capability',
    'timeout',
    'cancelled',
    'spawn_failed',
    'signal_terminated',
    'inspection_not_found',
    'cleanup_failed',
    'io_error',
    'internal_workstation_error',
  ]) {
    assert(portProduction.includes(`"${code}"`), `Workstation error code is missing: ${code}`);
  }
  assert(
    /DEFAULT_FILE_READ_MAX_BYTES:\s*u64\s*=\s*1_048_576/.test(portProduction) &&
      /HARD_FILE_READ_MAX_BYTES:\s*u64\s*=\s*8_388_608/.test(portProduction),
    'Stage 12 file-read default/hard limits differ',
  );

  const workstationImpl = extractRustNamedBlock(
    adapterProduction,
    /\bimpl\s+Workstation\s+for\s+LocalWorkstation\s*\{/,
    'Stage 13 LocalWorkstation implementation',
  );
  assert(
    equalStringArrays(rustMethodNames(workstationImpl), [
      'capabilities',
      'read_file',
      'execute',
      'inspect_execution',
      'cancel_execution',
    ]),
    `LocalWorkstation methods differ: ${rustMethodNames(workstationImpl).join(', ')}`,
  );
  const executeMethod = extractRustFunction(workstationImpl, 'execute');
  const inspectMethod = extractRustFunction(workstationImpl, 'inspect_execution');
  const cancelMethod = extractRustFunction(workstationImpl, 'cancel_execution');
  assert(
    /prepare_committed_execution_cwd/.test(executeMethod) && /runtime\.execute\(request, cwd\?\)\.await/.test(executeMethod),
    'Stage 13 execute must validate committed cwd evidence and delegate to the owned execution runtime',
  );
  assert(
    /self\.execution\s*\.inspect/.test(inspectMethod) && /runtime\s*\.cancel/.test(cancelMethod),
    'Stage 13 inspect/cancel must be real execution-runtime operations',
  );

  const capabilities = extractRustFunction(adapterProduction, 'stage13_capabilities');
  assert(
    /cpu_architecture:\s*std::env::consts::ARCH/.test(capabilities) &&
      /os_release:\s*std::env::consts::OS/.test(capabilities) &&
      /filesystem_read:\s*true/.test(capabilities) &&
      /privilege_user:\s*true/.test(capabilities) &&
      /foreground_execute:\s*support\.foreground/.test(capabilities) &&
      /cancel_execution:\s*support\.foreground/.test(capabilities) &&
      /inspect_execution:\s*support\.foreground/.test(capabilities) &&
      /privilege_administrative:\s*support\.administrative/.test(capabilities) &&
      /process_group_cleanup:\s*support\.process_group/.test(capabilities) &&
      /cgroup_cleanup:\s*support\.cgroup/.test(capabilities) &&
      /HARD_EXECUTION_TIMEOUT_MS/.test(capabilities) &&
      (capabilities.match(/HARD_EXECUTION_STREAM_CAPTURE_BYTES/g) ?? []).length === 2,
    'LocalWorkstation Stage 13 capability observation or limits differ',
  );

  const processBoundaryLeaks = stage13ProcessBoundaryLeaks(productionFiles);
  assert(
    processBoundaryLeaks.length === 0,
    `process APIs escaped LocalWorkstation lifecycle ownership: ${processBoundaryLeaks.join(', ')}`,
  );
  assert(
    /entries:\s*HashMap<ExecutionId, Arc<ExecutionEntry>>/.test(executionProduction) &&
      /registry\.entries\.contains_key\(&request\.execution_id\)/.test(executionProduction) &&
      /\.insert\(request\.execution_id/.test(executionProduction) &&
      /registry\.entries\.remove\(&execution_id\)/.test(executionProduction) &&
      !/ExecutionId::[^;\n]*(?:pid|pgid)/i.test(executionProduction),
    'Stage 13 registry must be keyed by ExecutionId and reject duplicate live ownership',
  );
  assert(
    stage13OwnershipBoundaryViolations(executionProduction).length === 0,
    `Stage 13 reservation-ownership violations: ${stage13OwnershipBoundaryViolations(executionProduction).join(', ')}`,
  );
  assert(
    stage13WaitidEintrViolations(executionProduction).length === 0,
    `Stage 13 waitid EINTR violations: ${stage13WaitidEintrViolations(executionProduction).join(', ')}`,
  );
  assert(
    /Command::new\(SUDO_PATH\)/.test(executionProduction) &&
      /Command::new\(shell\)/.test(executionProduction) &&
      /\.arg\("--noprofile"\)[\s\S]*\.arg\("--norc"\)[\s\S]*\.arg\("-o"\)[\s\S]*\.arg\("pipefail"\)[\s\S]*\.arg\("-c"\)[\s\S]*\.arg\(&request\.command\)/.test(executionProduction) &&
      /\.stdin\(Stdio::null\(\)\)/.test(executionProduction) &&
      (executionProduction.match(/\.env_clear\(\)/g) ?? []).length >= 2 &&
      !/ExecutionEnvironmentVariable|environment:\s*(?:Vec|HashMap)/.test(portProduction),
    'Stage 13 launch must use fixed Bash argv, a closed stdin, and an exact cleared environment',
  );
  for (const variable of ['HOME', 'USER', 'LOGNAME', 'SHELL', 'LANG', 'PATH', 'CRAXII_WORK_ID', 'CRAXII_WORKSPACE_ID']) {
    assert(executionProduction.includes(`"${variable}"`), `Stage 13 child environment is missing ${variable}`);
  }
  const childEnvironment = extractRustFunction(executionProduction, 'child_environment');
  assert(
    !/(?:OPENAI_API_KEY|ANTHROPIC_API_KEY|AWS_SECRET_ACCESS_KEY|SSH_AUTH_SOCK)/.test(childEnvironment),
    'Stage 13 production child launcher mentions forbidden ambient/provider secrets',
  );
  assert(
    /\.pre_exec\(/.test(executionProduction) && /libc::fchdir/.test(executionProduction) &&
      /libc::setsid/.test(executionProduction) && /killpg/.test(executionProduction) &&
      /Signal::SIGTERM/.test(executionProduction) && /Signal::SIGKILL/.test(executionProduction) &&
      /child\.try_wait\(\)/.test(executionProduction),
    'Stage 13 process session ownership or TERM/KILL/reap cleanup is incomplete',
  );
  const finishOwnedTree = extractRustFunction(executionProduction, 'finish_owned_process_tree');
  assert(
    stage13StableGroupOrderingViolations(executionProduction, finishOwnedTree).length === 0,
    `Stage 13 stable process-group violations: ${stage13StableGroupOrderingViolations(executionProduction, finishOwnedTree).join(', ')}`,
  );
  for (const token of ['cgroup.procs', 'cgroup.kill', 'cgroup.events']) {
    assert(executionProduction.includes(token), `Stage 13 Linux cgroup implementation is missing ${token}`);
  }
  assert(
      /let mut drains = JoinSet::new\(\)/.test(executionProduction) &&
      (executionProduction.match(/drains\.spawn\(drain_stream/g) ?? []).length === 2 &&
      /while !drains\.is_empty\(\)/.test(executionProduction) &&
      /drains\.join_next_with_id\(\)/.test(executionProduction) &&
      /drains\.abort_all\(\)/.test(executionProduction) &&
      /HARD_EXECUTION_STREAM_CAPTURE_BYTES/.test(executionProduction) &&
      /observed\.overflowing_add/.test(executionProduction) &&
      !/\.read_to_end\s*\(/.test(executionProduction),
    'Stage 13 stdout/stderr capture must be concurrent, bounded, saturating, and fully drained',
  );
  assert(
    /manager:\s*Mutex<Option<ManagerOwnership>>/.test(executionProduction) &&
      /join:\s*Option<JoinHandle<\(\)>>/.test(executionProduction) &&
      /let mut supervisors = JoinSet::new\(\)/.test(executionProduction) &&
      /supervisors\.join_next_with_id\(\)/.test(executionProduction),
    'Stage 13 lifecycle tasks must retain, join, and observe supervisor ownership',
  );
  assert(
    stage13ExecutionViolations(execution).length === 0,
    `Stage 13 execution safety violations: ${stage13ExecutionViolations(execution).join(', ')}`,
  );

  const resolver = extractRustFunction(adapterProduction, 'resolve_existing_path');
  const blockingRead = extractRustFunction(adapterProduction, 'read_blocking');
  const asyncRead = extractRustFunction(workstationImpl, 'read_file');
  assert(
    /LogicalPathKind::WorkspaceRelative/.test(resolver) &&
      /LogicalPathKind::Absolute/.test(resolver) &&
      /std::fs::canonicalize/.test(resolver) &&
      /ResolvedPathEvidence::try_new/.test(resolver) &&
      !/\.starts_with\s*\(/.test(resolver),
    'Stage 12 shared path resolver is incomplete or claims prefix confinement',
  );
  assert(
    /tokio::task::spawn_blocking/.test(asyncRead) &&
      /OpenOptions::new/.test(blockingRead) &&
      /O_CLOEXEC/.test(blockingRead) &&
      /O_NONBLOCK/.test(blockingRead) &&
      (blockingRead.match(/file\.metadata\s*\(/g) ?? []).length === 2 &&
      /ensure_regular_file/.test(blockingRead) &&
      /file\.read\s*\(/.test(blockingRead) &&
      /next_length as u64 > request\.max_bytes/.test(blockingRead) &&
      /Sha256Digest::hash_bytes/.test(blockingRead) &&
      /String::from_utf8/.test(blockingRead) &&
      /truncated:\s*false/.test(blockingRead),
    'Stage 12 descriptor-based bounded strict-UTF-8 read structure is incomplete',
  );
  const filesystemLeaks = stage12ModelFilesystemLeaks(productionFiles);
  assert(
    filesystemLeaks.length === 0,
    `model/application/domain/port filesystem access escaped LocalWorkstation: ${filesystemLeaks.join(', ')}`,
  );

  const startup = readFileSync(join(rustRoot, 'bootstrap', 'startup.rs'), 'utf8');
  assert(
    stage13DeadlinePropagationViolations(startup, executionProduction).length === 0,
    `Stage 13 shutdown-deadline violations: ${stage13DeadlinePropagationViolations(startup, executionProduction).join(', ')}`,
  );
  assert(
    /LocalWorkstation::new\([\s\S]*LocalWorkstationOptions/.test(startup) &&
      /let local_workstation = Arc::new\(\s*LocalWorkstation::new/.test(startup) &&
      /let workstation:\s*Arc<dyn Workstation>\s*=\s*local_workstation\.clone\(\)/.test(startup) &&
      /workstation:\s*Arc<dyn Workstation>/.test(startup) &&
      /local_workstation\.begin_execution_shutdown\(deadline\)/.test(startup) &&
      /shutdown_executions_before\(deadline\)/.test(startup) &&
      startup.indexOf('shutdown_executions_before(deadline)') <
        startup.indexOf('self.sqlite_runtime.shutdown().await') &&
      !/mark_ready\s*\(/.test(startup),
    'Stage 13 bootstrap must retain and shut down LocalWorkstation under the Stage 10 deadline while remaining live_unready',
  );

  const stateStore = readFileSync(join(rustRoot, 'adapters', 'sqlite', 'state_store.rs'), 'utf8');
  const refresh = extractRustFunction(stateStore, 'validate_existing_bootstrap_in_write');
  assert(
    /UPDATE workstations SET capabilities_json = \?, last_seen_at = \?/.test(refresh) &&
      /UPDATE workspaces SET local_resolved_root = \?/.test(refresh) &&
      !/UPDATE journal_events/.test(refresh),
    'Stage 13 current capability/root refresh is incomplete or rewrites journal history',
  );

  const pathTests = readFileSync(join(rustRoot, 'domain', 'path.rs'), 'utf8');
  const stage7Tests = readFileSync(join(rustRoot, 'adapters', 'sqlite', 'stage7_tests.rs'), 'utf8');
  for (const [source, testName] of [
    [port, 'fake_proves_the_port_has_no_local_descriptor_or_path_handle_requirement'],
    [port, 'operation_and_execution_ids_are_distinct_uuidv7_domain_types'],
    [adapter, 'capabilities_are_exact_truthful_stage13_runtime_facts'],
    [adapter, 'identity_generation_and_workspace_guards_precede_path_io'],
    [adapter, 'normal_utf8_empty_multibyte_bom_newlines_nul_and_hashes_are_exact'],
    [adapter, 'absolute_nested_unicode_and_control_character_paths_are_supported_and_redacted'],
    [adapter, 'invalid_utf8_returns_only_safe_binary_length_and_digest_evidence'],
    [adapter, 'exact_request_and_hard_limits_succeed_while_oversize_and_sparse_fail'],
    [adapter, 'missing_regular_target_is_not_found'],
    [adapter, 'directory_fifo_socket_and_character_device_reject_without_blocking'],
    [adapter, 'symlinks_inside_outside_and_chains_succeed_broken_and_loops_fail'],
    [adapter, 'replacement_after_open_returns_one_complete_original_file_object'],
    [adapter, 'deterministic_mutation_growth_and_shrink_are_changed_during_read'],
    [adapter, 'expired_and_inflight_deadlines_are_honest_without_cancellation_claims'],
    [adapter, 'read_has_no_target_or_directory_side_effects'],
    [adapter, 'stage13_executes_bash_with_separate_capture_and_terminal_registry_removal'],
    [adapter, 'execution_form_quoting_pipes_redirection_profiles_and_fresh_shell_are_exact'],
    [adapter, 'exact_child_environment_excludes_parent_secrets_and_stdin_is_closed_without_tty'],
    [adapter, 'cwd_relative_absolute_outside_symlink_missing_file_and_open_handle_race_are_honest'],
    [adapter, 'output_empty_separate_simultaneous_binary_and_newlines_are_exact'],
    [adapter, 'capture_ceiling_continues_draining_and_projection_is_head_tail'],
    [adapter, 'exit_signal_spawn_and_request_validation_results_remain_distinct'],
    [adapter, 'registry_inspect_duplicate_cancel_repeat_concurrent_and_natural_race_are_coherent'],
    [adapter, 'caller_drop_keeps_execution_owned_and_shutdown_closes_admission_and_joins'],
    [adapter, 'shutdown_and_execution_reservation_have_only_two_atomic_outcomes'],
    [adapter, 'natural_exit_cleanup_signals_descendants_before_releasing_leader_identity'],
    [adapter, 'cancellation_cleanup_signals_before_releasing_leader_identity'],
    [adapter, 'timeout_cleanup_signals_before_releasing_leader_identity'],
    [adapter, 'shutdown_cleanup_uses_original_deadline_and_releases_identity_after_signals'],
    [adapter, 'stage10_expired_deadline_forces_kill_reports_uncertain_and_joins_before_return'],
    [adapter, 'repeated_waitid_eintr_yields_to_shutdown_cancellation_and_stage10_deadline'],
    [adapter, 'timeout_and_completion_remove_background_children_with_term_kill_escalation'],
    [adapter, 'execution_debug_and_errors_redact_command_cwd_environment_and_output_canaries'],
    [execution, 'first_terminal_cause_wins_cancel_timeout_and_natural_exit_races'],
    [execution, 'waitid_eintr_is_one_cooperative_nonterminal_observation'],
    [execution, 'launcher_argv_and_user_admin_environment_are_exact'],
    [adapter, 'linux_target_ubuntu_nonroot_systemd_cgroup_git_and_service_contract'],
    [adapter, 'linux_target_user_admin_identity_clean_environment_and_cgroup_cleanup'],
    [adapter, 'linux_target_cgroup_kills_session_escape_and_repeated_process_trees'],
    [adapter, 'linux_target_crash_marker_probe_execution'],
    [adapter, 'linux_target_docker_disposable_service_crash_restart_and_reboot_leak_harness'],
    [pathTests, 'workspace_relative_paths_normalize_lexically'],
    [pathTests, 'workspace_relative_escape_and_empty_results_are_rejected'],
    [pathTests, 'absolute_paths_normalize_and_clamp_at_root'],
    [pathTests, 'backslash_and_nul_are_rejected_for_both_kinds'],
    [pathTests, 'canonical_utf8_boundary_is_exact'],
    [pathTests, 'debug_redacts_path_text'],
    [stage7Tests, 'stage13_refreshes_current_capabilities_root_and_last_seen_without_rewriting_initial_event'],
  ]) {
    assert(
      new RegExp(`(?:async\\s+)?fn\\s+${testName}\\s*\\(`).test(source),
      `Stage 13 permanent test inventory is missing ${testName}`,
    );
  }
  assert(
    existsSync(join(repositoryRoot, 'scripts', 'verify-stage13-ubuntu-target')),
    'Stage 13 Ubuntu target-verification harness is absent',
  );
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

function verifyStage13CheckerNegativeProbes() {
  const executionPath = join(
    repositoryRoot,
    'backend',
    'src',
    'adapters',
    'local_workstation',
    'execution.rs',
  );
  const executionSource = readFileSync(executionPath, 'utf8');
  const executionProduction = stripRustComments(
    withoutRustTestModules(executionSource),
  );
  const executeRouter = `
    Router::new()
      .route("/health/live", get(liveness))
      .route("/health/ready", get(readiness))
      .route("/bootstrap", get(bootstrap))
      .route("/conversations/{conversation_id}/messages", post(message))
      .route("/work-items/{work_id}/cancel", post(cancel))
      .route("/events", get(events))
      .route("/workstations/{id}/execute", post(execute));`;
  expectStructuralRejection('execute HTTP route', () => verifyStage11RouteInventory(executeRouter));

  const cases = [
    ['ambient environment inheritance', 'fn launch() { Command::new("/bin/bash").spawn(); }'],
    ['provider or ambient secret inclusion', 'fn env() { command.env("OPENAI_API_KEY", secret); }'],
    ['parent-only kill', 'async fn stop() { child.kill().await; }'],
    ['unbounded output buffering', 'async fn drain() { stdout.read_to_end(&mut bytes).await; }'],
    ['detached lifecycle task', 'fn launch() { tokio::spawn(async { supervise().await }); }'],
    ['PID-as-ExecutionId', 'fn id(pid: u32) { ExecutionId::from_pid(pid); }'],
    ['raw sensitive tracing', 'fn log(request: R) { tracing::info!(command = request.command); }'],
    ['optimistic privileged capability', 'fn capabilities() { privilege_administrative: true, cgroup_cleanup: true }'],
  ];
  for (const [violation, fixture] of cases) {
    assert(
      stage13ExecutionViolations(fixture).includes(violation),
      `checker negative probe was not rejected: ${violation}`,
    );
  }

  const processEscape = [{ path: 'application/tool_service.rs', source: 'fn run() { Command::new("bash"); }' }];
  assert(
    stage13ProcessBoundaryLeaks(processEscape).length === 1,
    'checker negative probe was not rejected: process API outside LocalWorkstation',
  );
  const stage15 = [{ path: 'application/model_gateway.rs', source: 'struct ModelGateway;' }];
  assert(
    stage17PlusImplementationLeaks(stage15).length === 1,
    'checker negative probe was not rejected: Stage 16 ModelGateway implementation',
  );
  const missingStage10Deadline = stage13DeadlinePropagationViolations(
    'self.local_workstation.begin_execution_shutdown(); self.local_workstation.shutdown_executions_before(deadline);',
    'fn cleanup(request: Request) { terminate(request.deadline); }',
  );
  assert(
    missingStage10Deadline.includes('Stage 10 deadline not propagated unchanged') &&
      missingStage10Deadline.includes('request-only cleanup deadline'),
    'checker negative probe was not rejected: request-only shutdown cleanup deadline',
  );
  const staleGroupFixture = `
    struct StableProcessGroup;
    fn finish_owned_process_tree() {
      let status = child.try_wait();
      let released = process_group.release_for_reap();
      signal_owned_tree(&process_group);
    }
    fn observe() { waitid(P_PID, pid, WNOWAIT); }`;
  const staleFinish = extractRustFunction(staleGroupFixture, 'finish_owned_process_tree');
  assert(
    stage13StableGroupOrderingViolations(staleGroupFixture, staleFinish)
      .includes('stale PGID signal after leader identity release'),
    'checker negative probe was not rejected: stale PGID signal after reap',
  );
  const postReservationRejection = `
    struct ExecutionRegistry {
      admission_open: bool,
      entries: HashMap<ExecutionId, Arc<ExecutionEntry>>,
    }
    fn execute() {
      let mut registry = lock(&self.registry);
      if !registry.admission_open { return WorkstationUnavailable; }
      if registry.entries.contains_key(&request.execution_id) { return SpawnFailed; }
      registry.entries.insert(request.execution_id, entry);
      let sender = self.ensure_manager();
      sender.send(ManagerCommand::Launch(launch));
    }
    fn begin_shutdown() {
      let mut registry = lock(&self.registry);
      registry.admission_open = false;
      for entry in registry.entries.values() { entry.latch(TerminalCause::Shutdown); }
    }
    fn claim_spawn() {
      let phase = ExecutionPhase::Reserved;
      lifecycle.phase = ExecutionPhase::Spawning;
      pre_spawn_terminal_result();
    }
    fn supervise() { supervise_inner(&runtime, launch); }
    fn supervise_inner() {
      launch.entry.claim_spawn();
      command.spawn();
    }
    fn manager_loop() {
      supervisors.spawn(supervise(runtime, launch));
      if !runtime.admission_open { return WorkstationUnavailable; }
    }`;
  assert(
    stage13OwnershipBoundaryViolations(postReservationRejection)
      .includes('post-reservation admission rejection'),
    'checker negative probe was not rejected: post-reservation admission rejection',
  );
  const executeProduction = extractRustFunction(executionProduction, 'execute');
  const reservationStatement = /registry\s*\.entries\s*\.insert\s*\(\s*request\.execution_id\s*,\s*Arc::clone\(&entry\)\s*\)\s*;/.exec(executeProduction)?.[0];
  assert(reservationStatement, 'checker reservation-after-spawn mutation source is absent');
  const reservationAfterSpawn = executionProduction
    .replace(reservationStatement, '')
    .replace(
      /let\s+mut\s+child\s*=\s*match\s+command\.spawn\(\)\s*\{/,
      (spawn) => `${spawn}\n${reservationStatement}`,
    );
  assert(
    reservationAfterSpawn.indexOf(reservationStatement) >
      reservationAfterSpawn.indexOf('command.spawn()'),
    'checker reservation-after-spawn mutation was not constructed',
  );
  assert(
    stage13OwnershipBoundaryViolations(reservationAfterSpawn)
      .includes('execution ownership reservation does not precede OS process spawn'),
    'checker negative probe was not rejected: reservation after OS process spawn',
  );
  const unboundedEintrRetry = `
    enum LeaderObservationStatus { Pending, Interrupted, Terminal }
    struct WaitIdLeaderObserver;
    impl LeaderObserver for WaitIdLeaderObserver {
      fn observe() {
        loop {
          waitid(P_PID, pid, WNOWAIT);
          if error == EINTR { continue; }
        }
      }
    }
    fn supervise() {
      match status { Pending | LeaderObservationStatus::Interrupted => {} }
      tokio::select! { _ = GROUP_POLL_INTERVAL => {} }
    }
    fn normalize_waitid_attempt() { nix::libc::EINTR; LeaderObservationStatus::Interrupted; }`;
  assert(
    stage13WaitidEintrViolations(unboundedEintrRetry)
      .includes('unbounded synchronous EINTR retry'),
    'checker negative probe was not rejected: unbounded synchronous EINTR retry',
  );
  const recursiveEintrRetry = executionProduction.replace(
    /normalize_waitid_attempt\(result, observed_pid, error\)/,
    `if error == Some(nix::libc::EINTR) {
            self.observe(pid)
        } else {
            normalize_waitid_attempt(result, observed_pid, error)
        }`,
  );
  assert(
    recursiveEintrRetry !== executionProduction,
    'checker recursive-EINTR mutation source is absent',
  );
  assert(
    stage13WaitidEintrViolations(recursiveEintrRetry)
      .includes('unbounded synchronous EINTR retry'),
    'checker negative probe was not rejected: recursive EINTR retry',
  );

  const errorNormalization = 'let error = error.unwrap_or(nix::libc::EIO);';
  assert(
    executionProduction.includes(errorNormalization),
    'checker helper-chain EINTR mutation source is absent',
  );
  const withEintrPathHelper = (call, helpers) => `${executionProduction.replace(
    errorNormalization,
    `let error = {
        ${call};
        error.unwrap_or(nix::libc::EIO)
    };`,
  )}
${helpers}`;

  const helperToSameObserver = withEintrPathHelper(
    'let _ = eintr_retry_observer(&WaitIdLeaderObserver, 1)',
    `fn eintr_retry_observer(
        observer: &WaitIdLeaderObserver,
        pid: i32,
    ) -> std::io::Result<LeaderObservationStatus> {
        observer.observe(pid)
    }`,
  );
  const multiHopFullyQualifiedObserver = withEintrPathHelper(
    'let _ = eintr_observer_helper_a(&WaitIdLeaderObserver, 1)',
    `fn eintr_observer_helper_a(
        observer: &WaitIdLeaderObserver,
        pid: i32,
    ) -> std::io::Result<LeaderObservationStatus> {
        eintr_observer_helper_b(observer, pid)
    }
    fn eintr_observer_helper_b(
        observer: &WaitIdLeaderObserver,
        pid: i32,
    ) -> std::io::Result<LeaderObservationStatus> {
        <WaitIdLeaderObserver as LeaderObserver>::observe(observer, pid)
    }`,
  );
  assert(
    [helperToSameObserver, multiHopFullyQualifiedObserver].every((fixture) =>
      stage13WaitidEintrViolations(fixture).includes('unbounded synchronous EINTR retry'),
    ),
    'checker negative probe was not rejected: same-observer EINTR helper retry chain',
  );

  const multiHopSecondWaitid = withEintrPathHelper(
    'eintr_waitid_helper_a(1)',
    `fn eintr_waitid_helper_a(pid: i32) {
        eintr_waitid_helper_b(pid);
    }
    fn eintr_waitid_helper_b(pid: i32) {
        let mut siginfo: nix::libc::siginfo_t = unsafe { std::mem::zeroed() };
        unsafe {
            nix::libc::waitid(
                nix::libc::P_PID,
                pid as nix::libc::id_t,
                &raw mut siginfo,
                nix::libc::WEXITED | nix::libc::WNOHANG | nix::libc::WNOWAIT,
            );
        }
    }`,
  );
  assert(
    stage13WaitidEintrViolations(multiHopSecondWaitid)
      .includes('unbounded synchronous EINTR retry'),
    'checker negative probe was not rejected: second waitid through EINTR helper chain',
  );

  const cooperativeHelperChain = executionProduction.replace(
    'Ok(LeaderObservationStatus::Interrupted)',
    'eintr_interrupted_helper_a()',
  );
  assert(
    cooperativeHelperChain !== executionProduction,
    'checker cooperative helper-chain mutation source is absent',
  );
  const falsePositiveScope = stripRustComments(withoutRustTestModules(`${cooperativeHelperChain}
    fn eintr_interrupted_helper_a() -> std::io::Result<LeaderObservationStatus> {
      eintr_interrupted_helper_b()
    }
    fn eintr_interrupted_helper_b() -> std::io::Result<LeaderObservationStatus> {
      Ok(LeaderObservationStatus::Interrupted)
    }
    fn unrelated_recursion(depth: usize) {
      if depth > 0 { unrelated_recursion(depth - 1); }
    }
    fn unrelated_loop() { loop { break; } }
    fn unrelated_waitid_reference() { waitid(P_PID, pid, WNOWAIT); }
    #[cfg(test)]
    mod injected_observer {
      fn observe() { loop { waitid(P_PID, pid, WNOWAIT); continue; } }
    }`));
  assert(
    stage13WaitidEintrViolations(falsePositiveScope).length === 0,
    'checker false-positive probe rejected cooperative helper depth, unrelated recursion, loops, or waitid references',
  );
  return 19;
}

function stage15ProductionViolations(file) {
  const source = stripRustComments(withoutRustTestModules(file.source));
  const violations = [];
  if (stage17PlusImplementationLeaks([{ path: file.path, source }]).length > 0) {
    violations.push('Stage 17+ implementation');
  }
  const providerBoundary = file.path === 'ports/model_provider.rs' ||
    file.path === 'adapters/scripted_provider.rs';
  if (providerBoundary) {
    for (const [label, pattern] of [
      ['StateStore or SQLx access', /\b(?:StateStore|SqliteStateStore|sqlx)\b/],
      ['journal access', /\bJournal[A-Za-z0-9_]*\b|\bjournal_(?:event|write|append)|\b(?:append|write)_journal/],
      ['ToolExecutionService call', /\bToolExecutionService\b|\.execute_call\s*\(/],
      ['Workstation access', /\bWorkstation\b|\.read_file\s*\(|\.execute\s*\(/],
      ['provider HTTP or SSE', /\b(?:reqwest|hyper|Sse|EventSource)\b|authorization_header|https?:\/\//i],
      ['provider credential handling', /\b(?:api_key|authorization_header|bearer_token|CredentialRef)\b/i],
      ['filesystem or process access', /\b(?:std|tokio)::(?:fs|process)\b|\bCommand::new\s*\(/],
      ['wall-clock sleep', /tokio::time::sleep\s*\(|std::thread::sleep\s*\(/],
    ]) {
      if (pattern.test(source)) violations.push(label);
    }
  }
  if (file.path === 'application/model_selection.rs') {
    if (stage15DynamicRegistryMutation(source)) {
      violations.push('dynamic target/provider mutation');
    }
    if (stage15FallbackSelection(source)) {
      violations.push('silent model fallback');
    }
    violations.push(...stage15ModelSelectionContractViolations(source));
  }
  if (file.path === 'domain/model.rs' || /^adapters\/.*provider.*\.rs$/.test(file.path)) {
    if (stage15OutputReordering(source)) {
      violations.push('provider output sorting');
    }
    if (stage15UnknownItemDropping(source)) {
      violations.push('unknown provider item dropping');
    }
  }
  if (file.path === 'domain/model.rs') {
    violations.push(...stage15ModelResponseContractViolations(source));
    if (/parallel_tool_calls[\s\S]{0,40}(?:true|=\s*true)/.test(source)) {
      violations.push('parallel tool calls enabled');
    }
    if (/MAX_MODEL_OUTPUT_ITEMS\s*:\s*usize\s*=/.test(source) &&
        !/MAX_MODEL_OUTPUT_ITEMS\s*:\s*usize\s*=\s*64\s*;/.test(source)) {
      violations.push('output-item limit differs');
    }
    if (/MAX_MODEL_TOOL_ARGUMENT_BYTES\s*:\s*usize\s*=/.test(source) &&
        !/MAX_MODEL_TOOL_ARGUMENT_BYTES\s*:\s*usize\s*=\s*65_536\s*;/.test(source)) {
      violations.push('raw tool-argument limit differs');
    }
  }
  return violations;
}

const STAGE15_CALL_GRAPH_MAX_DEPTH = 16;
const STAGE15_TYPE_ALIAS_MAX_DEPTH = 16;

function stage15SimpleTypeAliases(source) {
  const aliases = new Map();
  for (const match of source.matchAll(
    /\btype\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^;={}]*)?=\s*([^;]+);/g,
  )) {
    aliases.set(match[1], match[2].trim());
  }
  return aliases;
}

function stage15ResolveTypeAliases(typeSource, aliases) {
  let resolved = typeSource;
  let depth = 0;
  for (; depth < STAGE15_TYPE_ALIAS_MAX_DEPTH; depth += 1) {
    let changed = false;
    for (const [name, value] of aliases) {
      const pattern = new RegExp(`\\b${name}\\b`, 'g');
      const next = resolved.replace(pattern, `(${value})`);
      if (next !== resolved) {
        resolved = next;
        changed = true;
      }
    }
    if (!changed) return { resolved, depthExceeded: false };
  }
  const depthExceeded = [...aliases.keys()].some((name) =>
    new RegExp(`\\b${name}\\b`).test(resolved),
  );
  return { resolved, depthExceeded };
}

function stage15TargetType(typeSource) {
  return /\b(?:ModelTarget(?:Snapshot|Collection|Catalog|Registry|Map|List|Set)?|ModelTargets|TargetCollection|TargetSnapshot|TargetCatalog|TargetRegistry)\b/.test(
    typeSource,
  );
}

function stage15TargetCollectionType(typeSource) {
  return stage15TargetType(typeSource) &&
    /\b(?:Vec|HashMap|BTreeMap|Map|HashSet|BTreeSet|Box|Arc|ModelTargetSnapshot|ModelTargets|TargetCollection|TargetSnapshot|Catalog|Registry)\b|\[\s*ModelTarget/.test(
      typeSource,
    );
}

function stage15CompactRust(source) {
  return source.replace(/\s+/g, '');
}

function stage15NamedBlockOrNull(source, pattern) {
  const match = pattern.exec(source);
  if (!match) return null;
  const opening = source.indexOf('{', match.index);
  if (opening === -1) return null;
  const closing = findMatchingDelimiter(source, opening, '{', '}');
  if (closing === -1) return null;
  return source.slice(match.index, closing + 1);
}

function stage15ModelSelectionContractViolations(source) {
  if (!/pub\s+struct\s+ModelTargetSnapshot\b/.test(source) ||
      !/pub\s+struct\s+ModelSelectionPolicy\b/.test(source)) return [];

  const violations = [];
  const aliases = stage15SimpleTypeAliases(source);
  const snapshot = stage15NamedBlockOrNull(source, /pub\s+struct\s+ModelTargetSnapshot\b/);
  const snapshotImpl = stage15NamedBlockOrNull(source, /impl\s+ModelTargetSnapshot\b/);
  const policyImpl = stage15NamedBlockOrNull(source, /impl\s+ModelSelectionPolicy\b/);
  if (stage15CompactRust(snapshot ?? '') !==
      'pubstructModelTargetSnapshot{default_target:ModelTargetId,targets:Box<[ModelTarget]>,}') {
    violations.push('published model-target storage is not the exact immutable Box<[ModelTarget]> shape');
  }
  if (!snapshotImpl || !policyImpl) {
    violations.push('model-target snapshot or selector implementation is absent');
    return violations;
  }

  const snapshotMethods = rustMethodNames(snapshotImpl);
  if (!equalStringArrays(snapshotMethods, [
    'try_new',
    'from_validated_config',
    'default_target',
    'targets',
    'target',
    'ordered_target_ids',
  ])) {
    violations.push('published model-target snapshot API differs from the read-only constructor contract');
  }
  if (/&\s*mut\s+self\b/.test(snapshotImpl) || /&\s*mut\s+ModelTargetSnapshot\b/.test(source)) {
    violations.push('published model-target snapshot exposes mutable access after construction');
  }
  const tryNew = rustFunctionBlocks(snapshotImpl).find((block) => block.name === 'try_new');
  if (!tryNew || !/mut\s+targets\s*:\s*Vec<ModelTarget>/.test(tryNew.parameters) ||
      !/targets\s*\.\s*sort_by\s*\(/.test(tryNew.body) ||
      !/targets\s*:\s*targets\s*\.\s*into_boxed_slice\s*\(\s*\)/.test(tryNew.body)) {
    violations.push('model-target constructor does not end mutable building at immutable publication');
  }

  for (const match of source.matchAll(/\bstruct\s+[A-Za-z_][A-Za-z0-9_]*[^;{]*\{/g)) {
    const opening = match.index + match[0].lastIndexOf('{');
    const closing = findMatchingDelimiter(source, opening, '{', '}');
    if (closing === -1) continue;
    const structure = source.slice(match.index, closing + 1);
    for (const field of structure.matchAll(/\b[A-Za-z_][A-Za-z0-9_]*\s*:\s*([^,\n}]+)/g)) {
      const resolved = stage15ResolveTypeAliases(field[1], aliases);
      if (resolved.depthExceeded) {
        violations.push('model-target storage alias exceeds the finite analysis bound');
        continue;
      }
      if (stage15TargetType(resolved.resolved) &&
          /\b(?:Vec|HashMap|BTreeMap|HashSet|BTreeSet|Mutex|RwLock|RefCell|UnsafeCell|Cell)\b/.test(
            resolved.resolved,
          )) {
        violations.push('mutable model-target collection is stored after construction');
      }
    }
  }
  for (const block of rustFunctionBlocks(source)) {
    const resolved = stage15ResolveTypeAliases(block.signature, aliases);
    if (resolved.depthExceeded && stage15TargetType(resolved.resolved)) {
      violations.push('model-target function signature exceeds the finite alias bound');
      continue;
    }
    const mutableTargetReference = /&\s*mut\s+[^,)]*(?:ModelTarget|\[\s*ModelTarget)/.test(
      resolved.resolved,
    );
    const ownedMutableBuilder = /mut\s+[A-Za-z_][A-Za-z0-9_]*\s*:\s*[^,)]*(?:Vec|BTreeMap|HashMap)[^,)]*ModelTarget/.test(
      resolved.resolved,
    );
    if (mutableTargetReference) {
      violations.push('mutable model-target storage escapes a constructor');
    }
    if (ownedMutableBuilder &&
        !/^(?:try_new|from_validated_config|build[A-Za-z0-9_]*|construct[A-Za-z0-9_]*)$/.test(block.name)) {
      violations.push('mutable model-target builder exists outside an approved constructor boundary');
    }
    if (/&\s*mut\s+[A-Za-z_][A-Za-z0-9_\s.]*\.\s*targets\b/.test(block.body) ||
        /\b[A-Za-z_][A-Za-z0-9_\s.]*\.\s*targets\s*=/.test(block.body)) {
      violations.push('published model-target storage has a mutable alias or whole-field assignment');
    }
  }

  const policyMethods = rustMethodNames(policyImpl);
  if (!equalStringArrays(policyMethods, ['new', 'snapshot', 'select', 'select_exact'])) {
    violations.push('selector API/topology differs from the exact-ID branch contract');
  }
  const policyFunctions = rustFunctionBlocks(policyImpl);
  const select = policyFunctions.find((block) => block.name === 'select');
  const selectExact = policyFunctions.find((block) => block.name === 'select_exact');
  const expectedSelectBody = `
    match explicit {
      Some(explicit_id) => self.select_exact(
        explicit_id,
        required,
        ModelSelectionReason::Explicit,
        ModelSelectionErrorKind::ExplicitTargetMissing,
        ModelSelectionErrorKind::ExplicitTargetDisabled,
        ModelSelectionErrorKind::ExplicitTargetIncapable,
      ),
      None => self.select_exact(
        self.snapshot.default_target(),
        required,
        ModelSelectionReason::ConfiguredDefault,
        ModelSelectionErrorKind::DefaultTargetMissing,
        ModelSelectionErrorKind::DefaultTargetDisabled,
        ModelSelectionErrorKind::DefaultTargetIncapable,
      ),
    }`;
  const expectedSelectExactBody = `
    let target = self
      .snapshot
      .target(target_id)
      .ok_or(ModelSelectionError(missing))?;
    if !target.enabled() {
      return Err(ModelSelectionError(disabled));
    }
    if !required.satisfied_by(target.reference().capabilities()) {
      return Err(ModelSelectionError(incapable));
    }
    Ok(ModelSelectionResult {
      selected_target: target.clone(),
      reason,
      considered_target_ids: self.snapshot.ordered_target_ids(),
      required_capabilities: required,
      target_configuration_version: target.reference().target_configuration_version(),
    })`;
  if (!select || stage15CompactRust(select.body) !== stage15CompactRust(expectedSelectBody)) {
    violations.push('selector explicit/default branches are not the frozen exact-ID topology');
  }
  if (!selectExact ||
      stage15CompactRust(selectExact.body) !== stage15CompactRust(expectedSelectExactBody) ||
      !/target_id\s*:\s*&\s*ModelTargetId/.test(selectExact.parameters)) {
    violations.push('selected-target success provenance is not the exact requested/default ID lookup');
  }
  return [...new Set(violations)];
}

function stage15ModelResponseContractViolations(source) {
  if (!/pub\s+struct\s+ModelResponseInput\b/.test(source) ||
      !/pub\s+struct\s+ModelResponse\b/.test(source)) return [];

  const violations = [];
  const input = stage15NamedBlockOrNull(source, /pub\s+struct\s+ModelResponseInput\b/);
  const response = stage15NamedBlockOrNull(source, /pub\s+struct\s+ModelResponse\b/);
  const responseImpl = stage15NamedBlockOrNull(source, /impl\s+ModelResponse\b/);
  if (!/pub\s+output_items\s*:\s*Vec<ModelOutputItem>/.test(input ?? '') ||
      !/(?:^|\n)\s*output_items\s*:\s*Vec<ModelOutputItem>/.test(response ?? '') ||
      /pub\s+output_items\s*:/.test(response ?? '')) {
    violations.push('canonical response output storage shape differs');
  }
  if (!responseImpl) {
    violations.push('canonical response implementation is absent');
    return violations;
  }
  const functions = rustFunctionBlocks(responseImpl);
  const tryNew = functions.find((block) => block.name === 'try_new');
  const supported = functions.find((block) => block.name === 'require_supported_semantics');
  if (!tryNew ||
      stage15CompactRust(tryNew.signature) !==
        'fntry_new(input:ModelResponseInput)->Result<Self,ModelContractError>') {
    violations.push('canonical response constructor signature differs from immutable input ownership');
  } else {
    const uses = [...tryNew.body.matchAll(/input\s*\.\s*output_items/g)].length;
    if (uses !== 3 ||
        !/input\s*\.\s*output_items\s*\.\s*len\s*\(\s*\)/.test(tryNew.body) ||
        !/for\s+item\s+in\s+&\s*input\s*\.\s*output_items/.test(tryNew.body) ||
        !/output_items\s*:\s*input\s*\.\s*output_items\s*,/.test(tryNew.body)) {
      violations.push('canonical output is not validated immutably and moved unchanged into ModelResponse');
    }
    if (/\blet\s+(?:mut\s+)?(?:\([^;=]*\)|[A-Za-z_][A-Za-z0-9_]*)[^;=]*\boutput_items\b[^;=]*=/.test(
      tryNew.body,
    ) || /&\s*mut[^;\n]*output_items|output_items[^;\n]*\.\s*iter_mut\s*\(/.test(tryNew.body)) {
      violations.push('canonical output construction is not append-free immutable pass-through');
    }
  }
  if (/&\s*mut\s+self\b/.test(responseImpl) ||
      /&\s*mut\s+(?:Vec\s*<\s*ModelOutputItem|\[\s*ModelOutputItem\s*\])/.test(source)) {
    violations.push('canonical output storage exposes a mutable alias');
  }
  if (!tryNew ||
      !/ModelOutputItem::UnknownProviderItem\s*\(_\)\s*=>\s*semantics\.unknown\s*=\s*true/.test(
        tryNew.body,
      )) {
    violations.push('one-input item conservation does not classify unknown output exhaustively');
  }
  if (!supported ||
      !/\.\s*any\s*\(\s*\|item\|\s*matches!\(item,\s*ModelOutputItem::UnknownProviderItem\(_\)\)\s*\)/s.test(
        supported.body,
      ) || !/ModelContractErrorKind::UnknownSemanticItem/.test(supported.body)) {
    violations.push('unknown canonical output does not fail supported-semantics validation');
  }
  return [...new Set(violations)];
}

function stage15DynamicRegistryMutation(source) {
  const aliases = stage15SimpleTypeAliases(source);
  const resolvedAliases = [...aliases.values()].map((value) =>
    stage15ResolveTypeAliases(value, aliases),
  );
  if (resolvedAliases.some(({ resolved, depthExceeded }) =>
    depthExceeded && stage15TargetType(resolved))) return true;
  if (resolvedAliases.some(({ resolved }) =>
    stage15TargetCollectionType(resolved) && /\b(?:Mutex|RwLock)\b/.test(resolved))) return true;

  const expandedSource = stage15ResolveTypeAliases(source, aliases);
  if (expandedSource.depthExceeded && stage15TargetType(expandedSource.resolved)) return true;
  if (
    /\b(?:Mutex|RwLock)\s*<[\s\S]{0,360}\b(?:ModelTarget|ModelTargets|TargetCollection|TargetSnapshot|TargetCatalog|TargetRegistry)\b/.test(
      expandedSource.resolved,
    )
  ) return true;

  const functions = rustFunctionBlocks(source);
  const inventory = stage14FunctionInventory(functions);
  const typedRoots = functions.filter((block) => {
    const signature = stage15ResolveTypeAliases(block.signature, aliases);
    return signature.depthExceeded || stage15TargetType(signature.resolved) ||
      /(?:target|catalog|snapshot|registry)/i.test(block.name) && /&mut\s+self/.test(block.parameters);
  });
  const reachable = stage15ReachableRustBlocks(typedRoots, inventory);
  if (reachable.depthExceeded) return true;

  return reachable.some((block) => {
    const expanded = stage15ResolveTypeAliases(block.source, aliases);
    if (expanded.depthExceeded) return true;
    const targetSignature = stage15TargetType(
      stage15ResolveTypeAliases(block.signature, aliases).resolved,
    );
    const mutableTargetParameters = [];
    for (const match of expanded.resolved.matchAll(
      /\b([A-Za-z_][A-Za-z0-9_]*)\s*:\s*&\s*mut\s+([^,){]+)/g,
    )) {
      if (stage15TargetCollectionType(match[2])) mutableTargetParameters.push(match[1]);
    }
    for (const receiver of mutableTargetParameters) {
      if (new RegExp(
        `\\b${receiver}\\s*\\.\\s*(?:insert|remove|replace|clear|append|extend|push|retain|swap|swap_remove)\\s*\\(`,
      ).test(expanded.resolved)) return true;
    }
    if (
      targetSignature && /&mut\s+self/.test(block.parameters) &&
      /\bself\s*\.\s*[A-Za-z_][A-Za-z0-9_]*\s*(?:=|\.\s*(?:insert|remove|replace|clear|append|extend|push|retain|swap|swap_remove)\s*\()/.test(
        block.body,
      )
    ) return true;
    if (
      targetSignature &&
      /\b(?:register|replace|remove|insert|update|set|swap|mutate)[A-Za-z0-9_]*\s*\(/i.test(
        block.signature,
      ) && /&mut\s+self|&\s*mut\s+/.test(block.parameters)
    ) return true;
    return /\.\s*write\s*\(\s*\)[\s\S]{0,220}\.\s*(?:insert|remove|replace|clear|extend|push)\s*\(/.test(
      expanded.resolved,
    );
  });
}

function stage15FallbackSelection(source) {
  if (/pub\s+struct\s+ModelSelectionPolicy\b/.test(source)) {
    return stage15ModelSelectionContractViolations(source).some((violation) =>
      /selector|selected-target/.test(violation),
    );
  }
  const functions = rustFunctionBlocks(source);
  const inventory = stage14FunctionInventory(functions);
  const selectRoots = functions.filter((block) => block.name === 'select');
  const reachable = stage15ReachableRustBlocks(selectRoots, inventory);
  if (reachable.depthExceeded) return true;
  return reachable.some((block) => stage15SelectorProvenanceViolation(block));
}

function stage15SelectorProvenanceViolation(block) {
  const body = block.body;
  if (/\.(?:or|or_else|unwrap_or|unwrap_or_else|map_or|map_or_else)\s*\(/.test(body)) {
    return true;
  }
  if (
    /\bmatch\s+[^\{;]*(?:lookup|resolve|selected|explicit|target)[^\{;]*\{[\s\S]{0,700}\b(?:Err\s*\([^)]*\)|None)\s*=>[\s\S]{0,240}(?:default|target|candidate|\.find\s*\(|\.first\s*\(|\.next\s*\(|Ok\s*\(|Some\s*\()/i.test(
      body,
    )
  ) return true;
  if (
    /if\s+let\s+(?:Err\s*\([^)]*\)|None)\s*=[\s\S]{0,500}(?:default_target|\.targets?\s*\(|\.find\s*\(|return\s+(?:Ok|Some)\s*\()/i.test(
      body,
    )
  ) return true;
  if (
    /if\s+[^\{]{0,220}(?:!\s*[^\{]*(?:enabled|capable|satisfied_by)|(?:disabled|incapable))[^\{]*\{[\s\S]{0,420}(?:default_target|[A-Za-z_][A-Za-z0-9_]*target\s*\(|\.targets?\s*\(|\.find\s*\(|return\s+(?:Ok|Some)\s*\()/i.test(
      body,
    )
  ) return true;
  if (
    /(?:for\s+\w+\s+in|while\s+[^\{]+)[\s\S]{0,600}(?:enabled|capable|satisfied_by)[\s\S]{0,260}(?:return\s+(?:Ok|Some)|break\s+\w+)/.test(
      body,
    )
  ) return true;

  for (const match of body.matchAll(/\.(?:find|find_map|first|next)\s*\(([^;]*)/g)) {
    const expression = match[0];
    const exactIdentityLookup = /(?:==|\.eq\s*\()[\s\S]{0,160}(?:requested|explicit|target_id|\bid\b)/.test(
      expression,
    ) && !/(?:enabled|capable|satisfied_by)/.test(expression);
    const diagnosticOnly = /(?:considered|diagnostic|inventory|ordered_target_ids)/i.test(block.name) &&
      !/Result\s*<[^>]*ModelTarget|Option\s*<[^>]*ModelTarget/.test(block.signature);
    if (!exactIdentityLookup && !diagnosticOnly) return true;
  }
  return false;
}

function stage15OutputReordering(source) {
  const functions = rustFunctionBlocks(source);
  const inventory = stage14FunctionInventory(functions);
  const reachable = stage15ReachableRustBlocks(stage15ProviderOutputRoots(functions), inventory);
  if (reachable.depthExceeded) return true;
  return reachable.some((block) => {
    if (block.name === 'canonicalize_json' || /\bserde_json::Value\b|\bValue::Object\b/.test(block.source)) {
      return false;
    }
    const anyReceiverOrderMutation = /\b[A-Za-z_][A-Za-z0-9_]*\s*\.\s*(?:sort|sort_by|sort_by_key|sort_unstable|sort_unstable_by|sort_unstable_by_key|reverse|rotate_left|rotate_right|swap|swap_remove|dedup|dedup_by|dedup_by_key|splice|split_off)\s*\(/;
    const outputReceiverDestructiveMutation = /\b(?:output_items|response_items|normalized_items|provider_items|outputs|items|values)\s*\.\s*(?:insert|remove|drain|truncate|clear)\s*\(/;
    const iteratorReorder = /\.\s*(?:rev|sorted|sorted_by|sorted_by_key)\s*\(\s*\)/;
    const splitOrRecombine = /\.\s*partition\s*\(|\b(?:texts?|tools?|known|unknown|recognized|discarded)[A-Za-z0-9_]*\s*\.\s*(?:extend|append)\s*\(|\.\s*chain\s*\(/;
    return [
      anyReceiverOrderMutation,
      outputReceiverDestructiveMutation,
      iteratorReorder,
      splitOrRecombine,
    ].some((pattern) => pattern.test(block.body));
  });
}

function stage15ProviderOutputRoots(functions) {
  return functions.filter((block) =>
    /\bModelOutputItem\b|\b(?:output_items|response_items|provider_items|normalized_items|outputs)\b/.test(
      `${block.signature}\n${block.body}`,
    ) || /(?:normalize|response|output)/i.test(block.name) && /\bitems?\b/.test(block.parameters),
  );
}

function stage15LocalCallCounts(body, inventory) {
  const calls = new Map();
  for (const name of inventory.keys()) {
    const pattern = new RegExp(`(?:\\.|::|(?<![A-Za-z0-9_]))${name}\\s*\\(`, 'g');
    const count = [...body.matchAll(pattern)].length;
    if (count > 0) calls.set(name, count);
  }
  return calls;
}

function stage15ReachableRustBlocks(roots, inventory) {
  const reachable = [];
  reachable.depthExceeded = false;
  const pending = roots.map((block) => ({ block, depth: 0 }));
  const seen = new Set();
  while (pending.length > 0) {
    const { block, depth } = pending.pop();
    if (seen.has(block)) continue;
    seen.add(block);
    reachable.push(block);
    const localCalls = stage15LocalCallCounts(block.body, inventory);
    if (depth >= STAGE15_CALL_GRAPH_MAX_DEPTH && localCalls.size > 0) {
      reachable.depthExceeded = true;
      continue;
    }
    for (const name of localCalls.keys()) {
      pending.push(...(inventory.get(name) ?? []).map((callee) => ({
        block: callee,
        depth: depth + 1,
      })));
    }
  }
  return reachable;
}

function stage15UnknownItemDropping(source) {
  const functions = rustFunctionBlocks(source);
  const inventory = stage14FunctionInventory(functions);
  const reachable = stage15ReachableRustBlocks(stage15ProviderOutputRoots(functions), inventory);
  if (reachable.depthExceeded) return true;
  return reachable.some((block) => {
    if (block.name === 'canonicalize_json' || /\bserde_json::Value\b|\bValue::Object\b/.test(block.source)) {
      return false;
    }
    const silentLossOperation = /\.\s*(?:filter|filter_map|retain|partition|flat_map|take_while|skip_while|drain)\s*\(/;
    const unknownDiscardArm = /(?:UnknownProviderItem|UnknownProviderEvent)[\s\S]{0,220}=>\s*(?:continue\b|None\b|Ok\s*\(\s*None\s*\)|Vec::new\s*\(\s*\)|\[\s*\]|\{\s*\})/;
    const swallowedUnknownError = /(?:UnknownProviderItem|UnknownProviderEvent)[\s\S]{0,240}\.\s*ok\s*\(\s*\)|\.\s*ok\s*\(\s*\)[\s\S]{0,240}(?:UnknownProviderItem|UnknownProviderEvent)/;
    const guardedPushWithoutConservation = /if\s+[^\{]{0,180}(?:known|recognized|supported|UnknownProvider)[^\{]*\{[^{}]{0,360}\.\s*push\s*\([^{}]*\)\s*;?\s*\}(?!\s*else)/i;
    return [
      silentLossOperation,
      unknownDiscardArm,
      swallowedUnknownError,
      guardedPushWithoutConservation,
    ].some((pattern) => pattern.test(block.body));
  });
}

function verifyStage15CheckerNegativeProbes() {
  let probeCount = 0;
  const cases = [
    ['Stage 17 gateway introduction', 'application/model_gateway.rs', 'struct ModelGateway;'],
    ['ModelGateway introduction', 'application/model_gateway.rs', 'struct ModelGateway;'],
    ['AgentLoop introduction', 'application/agent_loop.rs', 'async fn run_agent_loop() {}'],
    ['production WorkRunner', 'application/work_runner.rs', 'struct WorkRunner;'],
    ['OpenAI adapter', 'adapters/openai.rs', 'struct OpenAIAdapter;'],
    ['Reqwest use', 'adapters/provider.rs', 'fn send(client: reqwest::Client) {}'],
    ['provider StateStore access', 'ports/model_provider.rs', 'fn persist(store: &dyn StateStore) {}'],
    ['provider journal write', 'adapters/scripted_provider.rs', 'fn write() { append_journal_event(); }'],
    ['provider ToolExecutionService call', 'adapters/scripted_provider.rs', 'fn call(service: &ToolExecutionService) { service.execute_call(); }'],
    ['provider Workstation call', 'adapters/scripted_provider.rs', 'fn call(machine: &dyn Workstation) { machine.execute(); }'],
    ['snapshot insert mutation', 'application/model_selection.rs', 'fn mutate(snapshot: &mut ModelTargets, target: ModelTarget) { snapshot.insert(target); }'],
    ['replace target mutation', 'application/model_selection.rs', 'fn replace_target(snapshot: &mut ModelTargets, target: ModelTarget) { snapshot.replace(target); }'],
    ['runtime model registration', 'application/model_selection.rs', 'pub fn register_model_target(&mut self, target: ModelTarget) { self.targets.push(target); }'],
    ['mutex model catalog', 'application/model_selection.rs', 'struct MutableCatalog { targets: Arc<Mutex<ModelTargets>> }'],
    ['helper-mediated registry mutation', 'application/model_selection.rs', `
      fn update(snapshot: &mut ModelTargets, target: ModelTarget) { change(snapshot, target); }
      fn change(collection: &mut ModelTargets, target: ModelTarget) { collection.insert(target); }`],
    ['alias-hidden mutex target vector', 'application/model_selection.rs', `
      type RuntimeTargetCollection = Vec<ModelTarget>;
      type SharedRuntimeTargets = Arc<Mutex<RuntimeTargetCollection>>;
      struct RuntimeRegistry { targets: SharedRuntimeTargets }`],
    ['alias-chain rwlock target map', 'application/model_selection.rs', `
      type TargetRows = BTreeMap<ModelTargetId, ModelTarget>;
      type TargetGuard = RwLock<TargetRows>;
      type SharedRuntimeTargets = Arc<TargetGuard>;
      struct RuntimeRegistry { targets: SharedRuntimeTargets }`],
    ['helper mutates alias-hidden map after startup', 'application/model_selection.rs', `
      type TargetRows = BTreeMap<ModelTargetId, ModelTarget>;
      type SharedRuntimeTargets = Arc<Mutex<TargetRows>>;
      fn update_after_startup(targets: &SharedRuntimeTargets, id: ModelTargetId, target: ModelTarget) {
        mutate_targets(targets, id, target);
      }
      fn mutate_targets(targets: &SharedRuntimeTargets, id: ModelTargetId, target: ModelTarget) {
        targets.lock().unwrap().insert(id, target);
      }`],
    ['setter replaces whole target collection', 'application/model_selection.rs', `
      type RuntimeTargetCollection = Vec<ModelTarget>;
      struct RuntimeRegistry { targets: RuntimeTargetCollection }
      impl RuntimeRegistry {
        pub fn set_targets(&mut self, replacement: RuntimeTargetCollection) {
          self.targets = replacement;
        }
      }`],
    ['helper write-lock inserts target', 'application/model_selection.rs', `
      type RuntimeTargetCollection = BTreeMap<ModelTargetId, ModelTarget>;
      type SharedRuntimeTargets = Arc<RwLock<RuntimeTargetCollection>>;
      fn install(targets: &SharedRuntimeTargets, id: ModelTargetId, target: ModelTarget) {
        write_target(targets, id, target);
      }
      fn write_target(targets: &SharedRuntimeTargets, id: ModelTargetId, target: ModelTarget) {
        targets.write().unwrap().insert(id, target);
      }`],
    ['or-else capable fallback', 'application/model_selection.rs', 'pub fn select(&self) { self.selected().or_else(|| first_capable_target()); }'],
    ['loop fallback after default failure', 'application/model_selection.rs', 'pub fn select(&self) { for target in self.snapshot.targets() { if target.capable() { return Ok(target); } } }'],
    ['helper-mediated alternate selection', 'application/model_selection.rs', `
      pub fn select(&self) { pick(self.snapshot.targets()); }
      fn pick(candidates: &[ModelTarget]) -> Option<&ModelTarget> {
        candidates.iter().find(|candidate| candidate.capable())
      }`],
    ['unwrap-or secondary target', 'application/model_selection.rs', 'pub fn select(&self) { selected.unwrap_or(secondary_target); }'],
    ['explicit failure falls back to default', 'application/model_selection.rs', 'pub fn select(&self) { explicit_target.or_else(|| self.snapshot.default_target()); }'],
    ['incapable default falls back to enabled target', 'application/model_selection.rs', 'pub fn select(&self) { if default_incapable { first_capable_target() } }'],
    ['match explicit lookup failure chooses default', 'application/model_selection.rs', `
      pub fn select(&self) {
        let selected = match resolve_explicit() {
          Ok(target) => target,
          Err(_) => resolve_configured_default(),
        };
        Ok(selected)
      }`],
    ['unwrap-or configured default', 'application/model_selection.rs', `
      pub fn select(&self) {
        let selected = resolve_explicit().unwrap_or(resolve_configured_default());
        Ok(selected)
      }`],
    ['unwrap-or-else first capable', 'application/model_selection.rs', `
      pub fn select(&self) {
        let selected = resolve_explicit().unwrap_or_else(|| targets.iter().find(|target| target.capable()).unwrap());
        Ok(selected)
      }`],
    ['helper match fallback', 'application/model_selection.rs', `
      pub fn select(&self) { resolve_requested(explicit_target, configured_default) }
      fn resolve_requested(explicit_target: Option<ModelTarget>, configured_default: ModelTarget) -> ModelTarget {
        match explicit_target { Some(target) => target, None => configured_default }
      }`],
    ['iterator alternate target search', 'application/model_selection.rs', `
      pub fn select(&self) -> Option<&ModelTarget> {
        self.snapshot.targets().iter().find(|candidate| candidate.enabled() && candidate.capable())
      }`],
    ['default capability failure chooses another enabled target', 'application/model_selection.rs', `
      pub fn select(&self) -> Option<&ModelTarget> {
        let selected = self.snapshot.target(self.snapshot.default_target())?;
        if !selected.capable() {
          return self.snapshot.targets().iter().find(|candidate| candidate.enabled());
        }
        Some(selected)
      }`],
    ['explicit disabled chooses default target', 'application/model_selection.rs', `
      pub fn select(&self) -> Option<&ModelTarget> {
        let selected = self.snapshot.target(explicit_target)?;
        if !selected.enabled() { return self.snapshot.target(self.snapshot.default_target()); }
        Some(selected)
      }`],
    ['direct provider output sort', 'domain/model.rs', 'fn normalize(mut output_items: Vec<Item>) { output_items.sort(); }'],
    ['helper-mediated provider output sort', 'domain/model.rs', `
      fn normalize(mut output_items: Vec<Item>) { scramble(&mut output_items); }
      fn scramble<T: Ord>(values: &mut [T]) { values.sort(); }`],
    ['provider output reverse', 'domain/model.rs', 'fn normalize(mut output_items: Vec<Item>) { output_items.reverse(); }'],
    ['split and reconstruct provider output', 'domain/model.rs', 'fn normalize(output_items: Vec<Item>) { let (mut texts, mut tools) = output_items.partition(is_text); texts.extend(tools); }'],
    ['stable sort by output variant', 'domain/model.rs', 'fn normalize(mut output_items: Vec<Item>) { output_items.sort_by_key(Item::variant); }'],
    ['deduplicate provider output', 'domain/model.rs', 'fn normalize(mut output_items: Vec<Item>) { output_items.dedup(); }'],
    ['rotate provider output left', 'domain/model.rs', 'fn normalize(mut output_items: Vec<Item>) { output_items.rotate_left(1); }'],
    ['rotate provider output right', 'domain/model.rs', 'fn normalize(mut output_items: Vec<Item>) { output_items.rotate_right(1); }'],
    ['swap provider output items', 'domain/model.rs', 'fn normalize(mut output_items: Vec<Item>) { output_items.swap(0, 1); }'],
    ['swap-remove provider output item', 'domain/model.rs', 'fn normalize(mut output_items: Vec<Item>) { output_items.swap_remove(0); }'],
    ['remove and reinsert provider output item', 'domain/model.rs', `
      fn normalize(mut output_items: Vec<Item>) {
        let first = output_items.remove(0);
        output_items.insert(1, first);
      }`],
    ['partition text and tools then concatenate', 'domain/model.rs', `
      fn normalize(output_items: Vec<Item>) -> Vec<Item> {
        let (texts, tools): (Vec<_>, Vec<_>) = output_items.into_iter().partition(Item::is_text);
        texts.into_iter().chain(tools).collect()
      }`],
    ['helper returns reversed clone', 'domain/model.rs', `
      fn normalize(output_items: Vec<Item>) -> Vec<Item> { reversed_copy(output_items) }
      fn reversed_copy(mut values: Vec<Item>) -> Vec<Item> { values.reverse(); values }
    `],
    ['sort provider output by variant', 'domain/model.rs', `
      fn normalize(mut output_items: Vec<Item>) { output_items.sort_by(|left, right| left.variant().cmp(&right.variant())); }
    `],
    ['filter unknown provider item', 'domain/model.rs', 'fn normalize(items: Vec<ModelOutputItem>) { items.filter(|item| !matches!(item, UnknownProviderItem(_))); }'],
    ['filter-map unknown provider item', 'domain/model.rs', 'fn normalize(items: Vec<ModelOutputItem>) { items.filter_map(|item| match item { UnknownProviderItem(_) => None, known => Some(known) }); }'],
    ['continue on unknown provider item', 'domain/model.rs', 'fn normalize(items: Vec<ModelOutputItem>) { for item in items { match item { UnknownProviderItem(_) => continue, _ => emit(item) } } }'],
    ['retain known provider items', 'domain/model.rs', 'fn normalize(mut items: Vec<ModelOutputItem>) { items.retain(|item| !matches!(item, UnknownProviderItem(_))); }'],
    ['helper-mediated unknown drop', 'domain/model.rs', `
      fn normalize(items: Vec<ModelOutputItem>) { items.filter_map(project); }
      fn project(item: ModelOutputItem) -> Option<ModelOutputItem> {
        match item { UnknownProviderItem(_) => None, known => Some(known) }
      }`],
    ['partition and discard unknown half', 'domain/model.rs', `
      fn normalize(items: Vec<ModelOutputItem>) -> Vec<ModelOutputItem> {
        let (known, discarded): (Vec<_>, Vec<_>) = items.into_iter().partition(is_known);
        known
      }`],
    ['flat-map unknown to empty', 'domain/model.rs', `
      fn normalize(items: Vec<ModelOutputItem>) -> Vec<ModelOutputItem> {
        items.into_iter().flat_map(|item| match item {
          UnknownProviderItem(_) => Vec::new(),
          known => vec![known],
        }).collect()
      }`],
    ['match unknown continues', 'domain/model.rs', `
      fn normalize(items: Vec<ModelOutputItem>) -> Vec<ModelOutputItem> {
        let mut output = Vec::new();
        for item in items {
          match item { UnknownProviderItem(_) => continue, known => output.push(known) }
        }
        output
      }`],
    ['helper returns only known provider items', 'domain/model.rs', `
      fn normalize(items: Vec<ModelOutputItem>) -> Vec<ModelOutputItem> { only_known(items) }
      fn only_known(items: Vec<ModelOutputItem>) -> Vec<ModelOutputItem> {
        items.into_iter().filter(|item| is_known(item)).collect()
      }`],
    ['guarded known push has no unknown branch', 'domain/model.rs', `
      fn normalize(items: Vec<ModelOutputItem>) -> Vec<ModelOutputItem> {
        let mut output = Vec::new();
        for item in items { if is_known(&item) { output.push(item); } }
        output
      }`],
    ['filter-map recognized provider items', 'domain/model.rs', `
      fn normalize(items: Vec<ModelOutputItem>) -> Vec<ModelOutputItem> {
        items.into_iter().filter_map(recognize).collect()
      }`],
    ['retain recognized provider items', 'domain/model.rs', `
      fn normalize(mut items: Vec<ModelOutputItem>) -> Vec<ModelOutputItem> {
        items.retain(is_known);
        items
      }`],
    ['more than 64 output items', 'domain/model.rs', 'const MAX_MODEL_OUTPUT_ITEMS: usize = 65;'],
    ['more than 64 KiB arguments', 'domain/model.rs', 'const MAX_MODEL_TOOL_ARGUMENT_BYTES: usize = 65_537;'],
    ['parallel tool calls true', 'domain/model.rs', 'fn request() { parallel_tool_calls = true; }'],
  ];
  for (const [label, path, source] of cases) {
    assert(
      stage15ProductionViolations({ path, source }).length > 0,
      `checker negative probe was not rejected: ${label}`,
    );
    probeCount += 1;
  }

  const modelRoute = `
    Router::new()
      .route("/health/live", get(liveness))
      .route("/health/ready", get(readiness))
      .route("/bootstrap", get(bootstrap))
      .route("/conversations/{conversation_id}/messages", post(message))
      .route("/work-items/{work_id}/cancel", post(cancel))
      .route("/events", get(events))
      .route("/models/invoke", post(invoke));`;
  expectStructuralRejection('public model endpoint', () => verifyStage11RouteInventory(modelRoute));
  probeCount += 1;

  assert(
    stage15MigrationViolations(['0001_core.sql', '0002_journal.sql', '0003_model.sql', '0004_stage15.sql']).length === 1,
    'checker negative probe was not rejected: migration 0004',
  );
  probeCount += 1;

  const readinessProbe = 'fn compose(health: Health) { health.mark_ready(); start_scheduler(); }';
  assert(
    stage15ReadinessViolations(readinessProbe).length === 2,
    'checker negative probe was not rejected: readiness promotion',
  );
  probeCount += 1;

  const falsePositiveCases = [
    ['constructor-local mutable builder', 'application/model_selection.rs', `
      struct ModelTargetSnapshot { targets: Box<[ModelTarget]> }
      fn build(config: Config) -> ModelTargetSnapshot {
        let mut targets = Vec::new();
        for item in config.targets() { targets.push(make_target(item)); }
        targets.sort_by(target_id_order);
        ModelTargetSnapshot { targets: targets.into_boxed_slice() }
      }`],
    ['considered target ID projection', 'application/model_selection.rs', `
      fn ordered_target_ids(&self) -> Vec<ModelTargetId> {
        self.targets.iter().map(|target| target.id().clone()).collect()
      }`],
    ['deterministic model target inventory sort', 'application/model_selection.rs', `
      fn build(mut targets: Vec<ModelTarget>) -> Box<[ModelTarget]> {
        targets.sort_by(|left, right| left.id().cmp(right.id()));
        targets.into_boxed_slice()
      }`],
    ['constructor-local target validation map', 'application/model_selection.rs', `
      fn validate(targets: &[ModelTarget]) -> Result<(), Error> {
        let mut by_id = BTreeMap::new();
        for target in targets { by_id.insert(target.id(), target); }
        Ok(())
      }`],
    ['immutable arc target slice', 'application/model_selection.rs', `
      type ImmutableTargets = Arc<[ModelTarget]>;
      struct ModelTargetSnapshot { targets: ImmutableTargets }
      fn publish(targets: Vec<ModelTarget>) -> ModelTargetSnapshot {
        ModelTargetSnapshot { targets: Arc::from(targets) }
      }`],
    ['diagnostic target inventory iteration', 'application/model_selection.rs', `
      pub fn select(&self) -> Result<&ModelTarget, Error> {
        record_diagnostic_ids(self.snapshot.targets());
        exact_target(self.snapshot.targets(), requested).ok_or(Error)
      }
      fn record_diagnostic_ids(targets: &[ModelTarget]) {
        for target in targets { record(target.id()); }
      }
      fn exact_target<'a>(targets: &'a [ModelTarget], requested: &ModelTargetId) -> Option<&'a ModelTarget> {
        targets.iter().find(|target| target.id() == requested)
      }`],
    ['exact target lookup helper', 'application/model_selection.rs', `
      pub fn select(&self) -> Result<&ModelTarget, Error> {
        exact_target(self.snapshot.targets(), requested).ok_or(Error)
      }
      fn exact_target<'a>(targets: &'a [ModelTarget], requested: &ModelTargetId) -> Option<&'a ModelTarget> {
        targets.iter().find(|target| target.id() == requested)
      }`],
    ['harmless immutable helper alias', 'application/model_selection.rs', `
      type ImmutableTargetSlice = Arc<[ModelTarget]>;
      fn target_count(targets: &ImmutableTargetSlice) -> usize { targets.len() }`],
    ['canonical JSON key sorting', 'domain/model.rs', `
      fn canonicalize_json_object(mut keys: Vec<(String, Value)>) -> Value {
        keys.sort_by(|left, right| left.0.cmp(&right.0));
        Value::Object(keys.into_iter().collect())
      }`],
    ['unrelated output fixture sorting', 'domain/model.rs', `
      fn sort_fixture_rows(mut rows: Vec<FixtureRow>) -> Vec<FixtureRow> {
        rows.sort_by_key(FixtureRow::ordinal);
        rows
      }`],
    ['model target inventory filtering', 'application/model_selection.rs', `
      fn enabled_target_ids(targets: &[ModelTarget]) -> Vec<ModelTargetId> {
        targets.iter().filter(|target| target.enabled()).map(|target| target.id().clone()).collect()
      }`],
    ['test-only output diagnostics', 'domain/model.rs', `
      #[cfg(test)]
      mod diagnostics {
        fn unknown_rows(items: Vec<ModelOutputItem>) -> Vec<ModelOutputItem> {
          items.into_iter().filter(|item| matches!(item, UnknownProviderItem(_))).collect()
        }
      }`],
    ['unrelated fixture filtering', 'domain/model.rs', `
      fn select_fixture_rows(rows: Vec<FixtureRow>) -> Vec<FixtureRow> {
        rows.into_iter().filter(|row| row.enabled()).collect()
      }`],
  ];
  for (const [label, path, source] of falsePositiveCases) {
    assert(
      stage15ProductionViolations({ path, source }).length === 0,
      `checker false-positive probe was rejected: ${label}`,
    );
  }
  const compiled = verifyStage15CompilationGatedProbes();
  return {
    negativeProbeCount: probeCount + compiled.negativeProbeCount,
    retainedStructuralProbeCount: probeCount,
    compilationGatedNegativeProbeCount: compiled.negativeProbeCount,
    compilationGatedCaseCount: compiled.negativeProbeCount + compiled.controlCount,
    builtInCompilationGatedProbeCount: compiled.builtInCount,
    novelChallengeMutationCount: compiled.novelCount,
    falsePositiveCompilationGatedControlCount: compiled.controlCount,
  };
}

function stage15ReplaceOnce(source, before, after, label) {
  const parts = source.split(before);
  assert(parts.length === 2, `Stage 15 probe mutation anchor differs: ${label}`);
  return `${parts[0]}${after}${parts[1]}`;
}

function stage15AppendProductionHelper(source, helper) {
  return `${source.trimEnd()}\n\n${helper.trim()}\n`;
}

const STAGE15_EXACT_LOOKUP = `        let target = self
            .snapshot
            .target(target_id)
            .ok_or(ModelSelectionError(missing))?;`;

const STAGE15_EXPLICIT_SELECTOR_ARM = `            Some(explicit_id) => self.select_exact(
                explicit_id,
                required,
                ModelSelectionReason::Explicit,
                ModelSelectionErrorKind::ExplicitTargetMissing,
                ModelSelectionErrorKind::ExplicitTargetDisabled,
                ModelSelectionErrorKind::ExplicitTargetIncapable,
            ),`;

function stage15MutateExactLookup(source, replacement, label) {
  return stage15ReplaceOnce(source, STAGE15_EXACT_LOOKUP, replacement, label);
}

function stage15MutateResponseBeforeConstruction(source, statement, outputExpression = null) {
  let mutated = stage15ReplaceOnce(
    source,
    '        let response = Self {',
    `${statement.trimEnd()}\n        let response = Self {`,
    'ModelResponse construction',
  );
  if (outputExpression !== null) {
    mutated = stage15ReplaceOnce(
      mutated,
      '            output_items: input.output_items,',
      `            output_items: ${outputExpression},`,
      'ModelResponse output move',
    );
  }
  return mutated;
}

function stage15MutableResponseInput(source) {
  return stage15ReplaceOnce(
    source,
    'pub fn try_new(input: ModelResponseInput) -> Result<Self, ModelContractError> {',
    'pub fn try_new(mut input: ModelResponseInput) -> Result<Self, ModelContractError> {',
    'mutable ModelResponse input',
  );
}

function stage15DynamicProbeDefinitions() {
  return [
    {
      label: 'neutral helper whole-catalog assignment',
      mutate(source) {
        let mutated = stage15ReplaceOnce(
          source,
          '    targets: Box<[ModelTarget]>,',
          '    targets: Vec<ModelTarget>,',
          this.label,
        );
        mutated = stage15ReplaceOnce(
          mutated,
          '            targets: targets.into_boxed_slice(),',
          '            targets,',
          this.label,
        );
        return stage15AppendProductionHelper(mutated, `
          impl ModelTargetSnapshot {
              fn publish_catalog_epoch(&mut self, replacement: Vec<ModelTarget>) {
                  transfer_entire_catalog(&mut self.targets, replacement);
              }
          }
          fn transfer_entire_catalog(targets: &mut Vec<ModelTarget>, replacement: Vec<ModelTarget>) {
              *targets = replacement;
          }`);
      },
    },
    {
      label: 'mem::replace catalog storage',
      mutate(source) {
        return stage15AppendProductionHelper(source, `
          impl ModelTargetSnapshot {
              fn publish_replacement(&mut self, replacement: Vec<ModelTarget>) {
                  let _old = std::mem::replace(&mut self.targets, replacement.into_boxed_slice());
              }
          }`);
      },
    },
    {
      label: 'alias-hidden wrapper setter',
      mutate(source) {
        let mutated = stage15ReplaceOnce(
          source,
          '#[derive(Debug)]\npub struct ModelTargetSnapshot {',
          'type PublishedCatalog = Vec<ModelTarget>;\n\n#[derive(Debug)]\npub struct ModelTargetSnapshot {',
          this.label,
        );
        mutated = stage15ReplaceOnce(
          mutated,
          '    targets: Box<[ModelTarget]>,',
          '    targets: PublishedCatalog,',
          this.label,
        );
        mutated = stage15ReplaceOnce(
          mutated,
          '            targets: targets.into_boxed_slice(),',
          '            targets,',
          this.label,
        );
        return stage15AppendProductionHelper(mutated, `
          impl ModelTargetSnapshot {
              fn publish_epoch(&mut self, replacement: PublishedCatalog) {
                  self.targets = replacement;
              }
          }`);
      },
    },
    {
      label: 'mutable published storage type',
      mutate(source) {
        return stage15ReplaceOnce(
          stage15ReplaceOnce(
            source,
            '    targets: Box<[ModelTarget]>,',
            '    targets: Vec<ModelTarget>,',
            this.label,
          ),
          '            targets: targets.into_boxed_slice(),',
          '            targets,',
          this.label,
        );
      },
    },
  ];
}

function stage15SelectorProbeDefinitions() {
  return [
    {
      label: 'Option plus get-zero fallback',
      mutate: (source) => stage15MutateExactLookup(source, `        let target = self
            .snapshot
            .target(target_id)
            .or_else(|| self.snapshot.targets().get(0))
            .ok_or(ModelSelectionError(missing))?;`, 'get-zero fallback'),
    },
    {
      label: 'first-target fallback',
      mutate: (source) => stage15MutateExactLookup(source, `        let target = self
            .snapshot
            .target(target_id)
            .or_else(|| self.snapshot.targets().first())
            .ok_or(ModelSelectionError(missing))?;`, 'first fallback'),
    },
    {
      label: 'map values-next fallback',
      mutate: (source) => stage15MutateExactLookup(source, `        let inventory: std::collections::BTreeMap<&ModelTargetId, &ModelTarget> = self
            .snapshot
            .targets()
            .iter()
            .map(|target| (target.reference().model_target_id(), target))
            .collect();
        let target = self
            .snapshot
            .target(target_id)
            .or_else(|| inventory.values().next().copied())
            .ok_or(ModelSelectionError(missing))?;`, 'values-next fallback'),
    },
    {
      label: 'neutral helper returns last target',
      mutate(source) {
        return stage15AppendProductionHelper(
          stage15MutateExactLookup(source, `        let target = self
            .snapshot
            .target(target_id)
            .or_else(|| candidate(self.snapshot.targets()))
            .ok_or(ModelSelectionError(missing))?;`, this.label),
          `fn candidate(targets: &[ModelTarget]) -> Option<&ModelTarget> { targets.last() }`,
        );
      },
    },
    {
      label: 'explicit failure selects configured default',
      mutate(source) {
        return stage15ReplaceOnce(source, STAGE15_EXPLICIT_SELECTOR_ARM, `            Some(explicit_id) => self
                .select_exact(
                    explicit_id,
                    required,
                    ModelSelectionReason::Explicit,
                    ModelSelectionErrorKind::ExplicitTargetMissing,
                    ModelSelectionErrorKind::ExplicitTargetDisabled,
                    ModelSelectionErrorKind::ExplicitTargetIncapable,
                )
                .or_else(|_| {
                    self.select_exact(
                        self.snapshot.default_target(),
                        required,
                        ModelSelectionReason::ConfiguredDefault,
                        ModelSelectionErrorKind::DefaultTargetMissing,
                        ModelSelectionErrorKind::DefaultTargetDisabled,
                        ModelSelectionErrorKind::DefaultTargetIncapable,
                    )
                }),`, this.label);
      },
    },
    {
      label: 'default failure selects arbitrary capable target',
      mutate: (source) => stage15MutateExactLookup(source, `        let target = self
            .snapshot
            .target(target_id)
            .or_else(|| {
                (reason == ModelSelectionReason::ConfiguredDefault)
                    .then(|| self.snapshot.targets().iter().find(|target| {
                        target.enabled() && required.satisfied_by(target.reference().capabilities())
                    }))
                    .flatten()
            })
            .ok_or(ModelSelectionError(missing))?;`, 'default arbitrary fallback'),
    },
  ];
}

function stage15OutputProbeDefinitions() {
  const mutableInput = (source, statement) => stage15MutateResponseBeforeConstruction(
    stage15MutableResponseInput(source),
    statement,
  );
  return [
    {
      label: 'as_mut_slice swap',
      mutate: (source) => mutableInput(source, `        if input.output_items.len() > 1 {
            input.output_items.as_mut_slice().swap(0, 1);
        }`),
    },
    {
      label: 'mutable slice rotate',
      mutate: (source) => mutableInput(source, `        if input.output_items.len() > 1 {
            let slice = &mut input.output_items[..];
            slice.rotate_left(1);
        }`),
    },
    {
      label: 'arbitrary helper receives mutable output Vec',
      mutate(source) {
        return stage15AppendProductionHelper(
          mutableInput(source, '        mutate_canonical_output(&mut input.output_items);'),
          `fn mutate_canonical_output(items: &mut Vec<ModelOutputItem>) {
              if items.len() > 1 { items.reverse(); }
          }`,
        );
      },
    },
    {
      label: 'remove and reinsert output item',
      mutate: (source) => mutableInput(source, `        if input.output_items.len() > 1 {
            let first = input.output_items.remove(0);
            input.output_items.insert(1, first);
        }`),
    },
    {
      label: 'reverse iterator reconstruction',
      mutate: (source) => stage15MutateResponseBeforeConstruction(
        source,
        `        let output_items = input.output_items.into_iter().rev().collect();`,
        'output_items',
      ),
    },
  ];
}

function stage15UnknownProbeDefinitions() {
  return [
    {
      label: 'fold drops unknown provider item',
      mutate: (source) => stage15MutateResponseBeforeConstruction(source, `        let output_items = input.output_items.into_iter().fold(
            Vec::new(),
            |mut acc, item| {
                if !matches!(item, ModelOutputItem::UnknownProviderItem(_)) {
                    acc.push(item);
                }
                acc
            },
        );`, 'output_items'),
    },
    {
      label: 'partition discards unknown half',
      mutate: (source) => stage15MutateResponseBeforeConstruction(source, `        let (output_items, _discarded): (Vec<_>, Vec<_>) = input
            .output_items
            .into_iter()
            .partition(|item| !matches!(item, ModelOutputItem::UnknownProviderItem(_)));`, 'output_items'),
    },
    {
      label: 'filter_map drops unknown item',
      mutate: (source) => stage15MutateResponseBeforeConstruction(source, `        let output_items = input
            .output_items
            .into_iter()
            .filter_map(|item| match item {
                ModelOutputItem::UnknownProviderItem(_) => None,
                known => Some(known),
            })
            .collect();`, 'output_items'),
    },
    {
      label: 'helper returns recognized subset',
      mutate(source) {
        return stage15AppendProductionHelper(
          stage15MutateResponseBeforeConstruction(
            source,
            '        let output_items = only_supported_items(input.output_items);',
            'output_items',
          ),
          `fn only_supported_items(items: Vec<ModelOutputItem>) -> Vec<ModelOutputItem> {
              items
                  .into_iter()
                  .filter(|item| !matches!(item, ModelOutputItem::UnknownProviderItem(_)))
                  .collect()
          }`,
        );
      },
    },
    {
      label: 'unknown match arm continues',
      mutate: (source) => stage15MutateResponseBeforeConstruction(source, `        let mut output_items = Vec::new();
        for item in input.output_items {
            if matches!(item, ModelOutputItem::UnknownProviderItem(_)) {
                continue;
            }
            output_items.push(item);
        }`, 'output_items'),
    },
  ];
}

function stage15NovelProbeDefinitions() {
  return [
    {
      className: 'dynamic registry',
      label: 'mem::swap immutable catalog field',
      path: 'backend/src/application/model_selection.rs',
      mutate: (source) => stage15AppendProductionHelper(source, `
        impl ModelTargetSnapshot {
            fn exchange_epoch(&mut self, replacement: Vec<ModelTarget>) {
                let mut replacement = replacement.into_boxed_slice();
                std::mem::swap(&mut self.targets, &mut replacement);
            }
        }`),
    },
    {
      className: 'dynamic registry',
      label: 'mem::take immutable catalog field',
      path: 'backend/src/application/model_selection.rs',
      mutate: (source) => stage15AppendProductionHelper(source, `
        impl ModelTargetSnapshot {
            fn empty_epoch(&mut self) {
                let _old = std::mem::take(&mut self.targets);
            }
        }`),
    },
    {
      className: 'selector provenance',
      label: 'slice index one fallback',
      path: 'backend/src/application/model_selection.rs',
      mutate: (source) => stage15MutateExactLookup(source, `        let target = self
            .snapshot
            .target(target_id)
            .or_else(|| self.snapshot.targets().get(1))
            .ok_or(ModelSelectionError(missing))?;`, 'slice index fallback'),
    },
    {
      className: 'selector provenance',
      label: 'neutral nth helper fallback',
      path: 'backend/src/application/model_selection.rs',
      mutate(source) {
        return stage15AppendProductionHelper(
          stage15MutateExactLookup(source, `        let target = self
            .snapshot
            .target(target_id)
            .or_else(|| later_candidate(self.snapshot.targets()))
            .ok_or(ModelSelectionError(missing))?;`, this.label),
          `fn later_candidate(targets: &[ModelTarget]) -> Option<&ModelTarget> {
              targets.iter().nth(1)
          }`,
        );
      },
    },
    {
      className: 'output order',
      label: 'subslice rotate-right',
      path: 'backend/src/domain/model.rs',
      mutate: (source) => stage15MutateResponseBeforeConstruction(source, `        let mut output_items = input.output_items;
        if output_items.len() > 1 {
            output_items[0..2].rotate_right(1);
        }`, 'output_items'),
    },
    {
      className: 'output order',
      label: 'pop last and insert first',
      path: 'backend/src/domain/model.rs',
      mutate: (source) => stage15MutateResponseBeforeConstruction(source, `        let mut output_items = input.output_items;
        if output_items.len() > 1 {
            let last = output_items.pop().expect("length checked");
            output_items.insert(0, last);
        }`, 'output_items'),
    },
    {
      className: 'unknown conservation',
      label: 'scan terminates at unknown item',
      path: 'backend/src/domain/model.rs',
      mutate: (source) => stage15MutateResponseBeforeConstruction(source, `        let output_items = input
            .output_items
            .into_iter()
            .scan((), |(), item| match item {
                ModelOutputItem::UnknownProviderItem(_) => None,
                known => Some(known),
            })
            .collect();`, 'output_items'),
    },
    {
      className: 'unknown conservation',
      label: 'take_while truncates at unknown item',
      path: 'backend/src/domain/model.rs',
      mutate: (source) => stage15MutateResponseBeforeConstruction(source, `        let output_items = input
            .output_items
            .into_iter()
            .take_while(|item| !matches!(item, ModelOutputItem::UnknownProviderItem(_)))
            .collect();`, 'output_items'),
    },
  ];
}

function stage15ControlDefinitions() {
  return [
    {
      label: 'mutable constructor builder',
      path: 'backend/src/application/model_selection.rs',
      mutate: (source) => stage15AppendProductionHelper(source, `
        fn build_control_catalog(mut targets: Vec<ModelTarget>) -> Box<[ModelTarget]> {
            targets.sort_by(|left, right| left.reference().model_target_id().cmp(
                right.reference().model_target_id(),
            ));
            targets.into_boxed_slice()
        }`),
    },
    {
      label: 'considered target ID iteration',
      path: 'backend/src/application/model_selection.rs',
      mutate: (source) => stage15AppendProductionHelper(source, `
        fn diagnostic_control_ids(snapshot: &ModelTargetSnapshot) -> Vec<ModelTargetId> {
            snapshot
                .targets()
                .iter()
                .map(|target| target.reference().model_target_id().clone())
                .collect()
        }`),
    },
    {
      label: 'model-target sorting',
      path: 'backend/src/domain/model.rs',
      mutate: (source) => stage15AppendProductionHelper(source, `
        fn sort_model_target_control(mut targets: Vec<ModelTarget>) -> Vec<ModelTarget> {
            targets.sort_by(|left, right| {
                left.reference().model_target_id().cmp(right.reference().model_target_id())
            });
            targets
        }`),
    },
    {
      label: 'canonical JSON key sorting',
      path: 'backend/src/domain/model.rs',
      mutate: (source) => stage15AppendProductionHelper(source, `
        fn canonicalize_json_control(mut keys: Vec<String>) -> Vec<String> {
            keys.sort();
            keys
        }`),
    },
    {
      label: 'unrelated fixture filtering',
      path: 'backend/src/domain/model.rs',
      mutate: (source) => stage15AppendProductionHelper(source, `
        fn select_fixture_control(rows: Vec<u64>) -> Vec<u64> {
            rows.into_iter().filter(|row| *row > 0).collect()
        }`),
    },
  ];
}

function stage15CompilationProbeDefinitions() {
  const selectionPath = 'backend/src/application/model_selection.rs';
  const modelPath = 'backend/src/domain/model.rs';
  const builtIn = [
    ...stage15DynamicProbeDefinitions().map((probe) => ({
      ...probe,
      className: 'dynamic registry',
      path: selectionPath,
    })),
    ...stage15SelectorProbeDefinitions().map((probe) => ({
      ...probe,
      className: 'selector provenance',
      path: selectionPath,
    })),
    ...stage15OutputProbeDefinitions().map((probe) => ({
      ...probe,
      className: 'output order',
      path: modelPath,
    })),
    ...stage15UnknownProbeDefinitions().map((probe) => ({
      ...probe,
      className: 'unknown conservation',
      path: modelPath,
    })),
  ];
  return { builtIn, novel: stage15NovelProbeDefinitions(), controls: stage15ControlDefinitions() };
}

function stage15CopyProbeRepository(destination) {
  cpSync(repositoryRoot, destination, {
    recursive: true,
    filter(source) {
      const path = relative(repositoryRoot, source);
      return path === '' || !/^(?:\.git|target)(?:\/|$)/.test(path);
    },
  });
}

function stage15RunCompilationCase(probeRepository, targetDirectory, probe, expectRejection) {
  const path = join(probeRepository, probe.path);
  const original = readFileSync(path, 'utf8');
  const mutated = probe.mutate(original);
  assert(mutated !== original, `Stage 15 compilation probe did not mutate source: ${probe.label}`);
  writeFileSync(path, mutated);
  try {
    const compile = spawnSync(
      'cargo',
      ['check', '--locked', '--workspace', '--all-targets'],
      {
        cwd: probeRepository,
        encoding: 'utf8',
        env: { ...process.env, CARGO_TARGET_DIR: targetDirectory },
        maxBuffer: 16 * 1024 * 1024,
      },
    );
    assert(
      compile.status === 0,
      `Stage 15 compilation-gated probe did not compile: ${probe.label}: ${
        compile.stderr.trim() || compile.stdout.trim() || `exit status ${compile.status}`
      }`,
    );
    const checker = spawnSync(
      process.execPath,
      ['scripts/check-repository.mjs', '--stage15-probe-only'],
      { cwd: probeRepository, encoding: 'utf8', maxBuffer: 4 * 1024 * 1024 },
    );
    if (expectRejection) {
      assert(
        checker.status !== 0 && /Repository invariant failed:/.test(checker.stderr),
        `compiling forbidden mutation was not rejected by the Stage 15 checker: ${probe.label}`,
      );
    } else {
      assert(
        checker.status === 0,
        `compiling legitimate control was rejected by the Stage 15 checker: ${probe.label}: ${
          checker.stderr.trim() || checker.stdout.trim() || `exit status ${checker.status}`
        }`,
      );
    }
  } finally {
    writeFileSync(path, original);
  }
}

function verifyStage15CompilationGatedProbes() {
  const definitions = stage15CompilationProbeDefinitions();
  assert(definitions.builtIn.length === 20, 'Stage 15 built-in compilation probe inventory differs');
  assert(definitions.novel.length === 8, 'Stage 15 novel mutation inventory differs');
  assert(definitions.controls.length === 5, 'Stage 15 compilation-gated control inventory differs');
  const temporaryRoot = mkdtempSync(join(tmpdir(), 'craxii-stage15-probes-'));
  const probeRepository = join(temporaryRoot, 'repository');
  const targetDirectory = join(temporaryRoot, 'target');
  try {
    stage15CopyProbeRepository(probeRepository);
    for (const probe of [...definitions.builtIn, ...definitions.novel]) {
      stage15RunCompilationCase(probeRepository, targetDirectory, probe, true);
    }
    for (const control of definitions.controls) {
      stage15RunCompilationCase(probeRepository, targetDirectory, control, false);
    }
  } finally {
    rmSync(temporaryRoot, { recursive: true, force: true });
  }
  return {
    negativeProbeCount: definitions.builtIn.length + definitions.novel.length,
    builtInCount: definitions.builtIn.length,
    novelCount: definitions.novel.length,
    controlCount: definitions.controls.length,
  };
}

function verifyStage15ProbeRepository() {
  const files = [
    ['application/model_selection.rs', 'backend/src/application/model_selection.rs'],
    ['domain/model.rs', 'backend/src/domain/model.rs'],
  ];
  const violations = files.flatMap(([checkerPath, repositoryPath]) =>
    stage15ProductionViolations({
      path: checkerPath,
      source: readFileSync(join(repositoryRoot, repositoryPath), 'utf8'),
    }).map((violation) => `${checkerPath}: ${violation}`),
  );
  assert(violations.length === 0, `Stage 15 probe boundary differs: ${violations.join(', ')}`);
}

function stage15MigrationViolations(names) {
  return names.filter((name) => /^000(?:[4-9]|[1-9][0-9]+)_.*\.sql$/.test(name));
}

function stage15ReadinessViolations(source) {
  const violations = [];
  if (/mark_ready\s*\(/.test(source)) violations.push('readiness promotion');
  if (/start_scheduler\s*\(/.test(source)) violations.push('scheduler activation');
  return violations;
}

function verifyStage15CanonicalModelStructure(rustRoot, productionFiles) {
  const expectedModules = [
    'adapters/scripted_provider.rs',
    'application/model_selection.rs',
    'domain/model.rs',
    'ports/model_provider.rs',
  ];
  const actualModules = productionFiles
    .map((file) => file.path)
    .filter((path) => /(?:^|\/)(?:model|model_selection|model_provider|scripted_provider)\.rs$/.test(path));
  assert(
    equalStringArrays(sortedStrings(actualModules), expectedModules),
    `Stage 15 production module allowlist differs: ${actualModules.join(', ')}`,
  );
  for (const file of productionFiles) {
    const violations = stage15ProductionViolations(file);
    assert(
      violations.length === 0,
      `Stage 15 boundary differs in ${file.path}: ${violations.join(', ')}`,
    );
  }
  const stage17Leaks = stage17PlusImplementationLeaks(productionFiles);
  assert(stage17Leaks.length === 0, `Stage 17+ implementation is forbidden: ${stage17Leaks.join(', ')}`);

  const model = readFileSync(join(rustRoot, 'domain', 'model.rs'), 'utf8');
  const selection = readFileSync(join(rustRoot, 'application', 'model_selection.rs'), 'utf8');
  const provider = readFileSync(join(rustRoot, 'ports', 'model_provider.rs'), 'utf8');
  const providerContract = readFileSync(
    join(rustRoot, 'ports', 'model_provider', 'provider_contract.rs'),
    'utf8',
  );
  const scripted = readFileSync(join(rustRoot, 'adapters', 'scripted_provider.rs'), 'utf8');
  const bootstrap = readFileSync(join(rustRoot, 'bootstrap', 'startup.rs'), 'utf8');
  const cargoManifest = readFileSync(join(repositoryRoot, 'backend', 'Cargo.toml'), 'utf8');

  assert(
    /MAX_MODEL_OUTPUT_ITEMS:\s*usize\s*=\s*64;/.test(model) &&
      /MAX_MODEL_TOOL_ARGUMENT_BYTES:\s*usize\s*=\s*65_536;/.test(model) &&
      /MAX_NORMALIZED_MODEL_RESPONSE_BYTES:\s*usize\s*=\s*262_144;/.test(model),
    'Stage 15 canonical output limits differ',
  );
  for (const variant of [
    'Text', 'ToolCall', 'StructuredData', 'Refusal', 'ReasoningSummary',
    'ProviderOpaque', 'UnknownProviderItem',
  ]) {
    assert(new RegExp(`\\b${variant}\\b`).test(extractRustNamedBlock(model, /pub\s+enum\s+ModelOutputItem\b/, 'ModelOutputItem')), `Stage 15 output variant is absent: ${variant}`);
  }
  assert(
    /pub\s+const\s+fn\s+parallel_tool_calls\([^)]*\)\s*->\s*bool\s*\{\s*false\s*\}/s.test(model) &&
      /"parallel_tool_calls":\s*false/.test(model),
    'Stage 15 request must freeze parallel_tool_calls=false',
  );
  assert(
    /pub\s+struct\s+ModelUsage\b/.test(model) &&
      /cached_input_tokens\s*>\s*input_tokens/.test(model) &&
      /reasoning_tokens\s*>\s*output_tokens/.test(model) &&
      /total_tokens\s*!=\s*calculated/.test(model),
    'Stage 15 usage validation is incomplete',
  );
  assert(
    /UnknownProviderItem/.test(model) && /require_supported_semantics/.test(model) &&
      /UnknownSemanticItem/.test(model),
    'Stage 15 unknown semantic item is not retained and rejected',
  );
  assert(
    /canonical_json_bytes/.test(model) && /Sha256Digest::hash_bytes/.test(model) &&
      /keys\.sort_by/.test(model),
    'Stage 15 canonical JSON/SHA-256 contract is incomplete',
  );
  assert(
    !/chain_of_thought|hidden_reasoning|private_reasoning/i.test(model) &&
      !/\b(?:OpenAI|ResponsesApi|SseEvent)\b/.test(model),
    'Stage 15 canonical model module contains hidden reasoning or provider wire types',
  );

  assert(
    /targets\.sort_by/.test(selection) && /targets:\s*Box<\[ModelTarget\]>/.test(selection) &&
      !/pub\s+fn\s+(?:insert|register|remove|update)/.test(selection),
    'Stage 15 target snapshot is not immutable and deterministically ordered',
  );
  assert(
    /match\s+explicit\s*\{[\s\S]*Some\(explicit_id\)\s*=>\s*self\.select_exact\s*\(/.test(selection) &&
      /None\s*=>\s*self\.select_exact\s*\([\s\S]*self\.snapshot\.default_target\(\)/.test(selection) &&
      /ExplicitTargetMissing/.test(selection) && /DefaultTargetMissing/.test(selection) &&
      !/fallback/i.test(stripRustComments(withoutRustTestModules(selection))),
    'Stage 15 exact explicit/default no-fallback selection differs',
  );
  assert(
    /ConfigFingerprint/.test(readFileSync(join(rustRoot, 'bootstrap', 'config', 'fingerprint.rs'), 'utf8')) &&
      /semantic_target_changes_alter_the_single_global_config_fingerprint/.test(selection),
    'Stage 15 does not prove the existing global config fingerprint binds target semantics',
  );

  assert(
    /pub\s+trait\s+ModelProvider:\s*Send\s*\+\s*Sync/.test(provider) &&
      /pub\s+trait\s+ModelProviderStream:\s*Send/.test(provider) &&
      /pub\s+trait\s+TokenEstimator:\s*Send\s*\+\s*Sync/.test(provider),
    'Stage 15 provider/stream/estimator ports are absent or not object-safe',
  );
  for (const category of [
    'Authentication', 'Authorization', 'InvalidRequest', 'UnknownModel', 'RateLimited',
    'TemporarilyUnavailable', 'TransportBeforeResponse', 'TransportAfterPossibleProcessing',
    'TimeoutBeforeOutput', 'TimeoutAfterOutput', 'MalformedResponse',
    'MalformedCompletedToolArguments', 'OutputTooLarge', 'UnsupportedResponseItem',
    'ContextError', 'SafetyRefusal', 'Cancelled', 'ProviderOutcomeUnknown',
    'InternalProviderError',
  ]) {
    assert(new RegExp(`\\b${category}\\b`).test(provider), `Stage 15 provider error category is absent: ${category}`);
  }
  assert(
    /MAX_PROVIDER_ATTEMPTS:\s*u32\s*=\s*3/.test(provider) &&
      /Duration::from_millis\(250\)/.test(provider) &&
      /Duration::from_secs\(5\)/.test(provider) &&
      /Duration::from_secs\(30\)/.test(provider) &&
      /Duration::from_secs\(5\s*\*\s*60\)/.test(provider) &&
      /Duration::from_secs\(60\)/.test(provider),
    'Stage 15 retry/backoff/deadline constants differ',
  );

  assert(
    /impl\s+ModelProvider\s+for\s+ScriptedProvider/.test(scripted) &&
      /ScriptedStep::AwaitRelease/.test(scripted) && /ScriptMismatch/.test(scripted) &&
      /impl\s+TokenEstimator\s+for\s+ScriptedTokenEstimator/.test(scripted) &&
      /clock:\s*Arc<dyn Clock>/.test(scripted) &&
      /expected:\s*Box<\[ScriptedEstimate\]>/.test(scripted) &&
      !/expected:\s*Mutex<VecDeque<ScriptedEstimate>>/.test(scripted),
    'Stage 15 deterministic scripted provider/estimator is incomplete',
  );
  assert(
    /trait\s+ModelProviderContractFixture/.test(providerContract) &&
      /assert_model_provider_contract/.test(providerContract) &&
      /Arc<dyn ModelProvider>/.test(providerContract) &&
      !/ScriptedProvider|ScriptedStream|ScriptedProgram|captures\s*\(/.test(providerContract),
    'Stage 15 provider contract suite is not reusable through the public provider port',
  );
  for (const testName of [
    'scripted_text_only_completion_is_ordered_and_captured_once',
    'scripted_one_tool_call_preserves_identity_name_and_arguments',
    'scripted_text_then_tool_call_preserves_provider_order',
    'scripted_multiple_tool_calls_preserve_exact_ordinals',
    'scripted_refusal_is_semantic_output_not_transport_failure',
    'scripted_structured_data_preserves_canonical_json',
    'scripted_reasoning_summary_exposes_only_summary_delta',
    'scripted_opaque_continuation_retains_provider_hash_and_type',
    'scripted_transient_pre_output_failure_is_retry_eligible',
    'scripted_failure_after_semantic_output_is_never_retryable',
    'scripted_malformed_tool_arguments_are_retained_then_fail_closed',
    'scripted_oversized_tool_arguments_are_rejected_before_emission',
    'scripted_duplicate_provider_tool_ids_fail_closed_without_item_drop',
    'scripted_timeout_before_output_is_retry_eligible',
    'scripted_timeout_after_output_is_not_retryable',
    'scripted_cancellation_uses_barrier_and_records_observation_without_sleep',
    'scripted_unknown_provider_item_is_retained_and_rejected_semantically',
    'scripted_request_hash_mismatch_fails_deterministically_before_stream',
    'scripted_machine_inspection_answer_fixture_is_deterministic',
    'scripted_stream_preserves_all_events_and_rejects_post_terminal_ordering',
    'response_terminal_consistency_matrix_fails_closed',
    'response_terminal_mixed_combinations_make_contradictions_explicit',
    'scripted_complete_multi_tool_lifecycle_preserves_order_without_execution',
    'scripted_program_rejects_every_post_terminal_residue_before_emission',
    'scripted_emitted_provider_error_capture_preserves_terminal_certainty',
    'scripted_overall_deadline_expires_before_first_event_without_sleep',
    'scripted_idle_timeout_classification_uses_shared_semantic_predicate',
    'scripted_timeout_threshold_completion_and_cancellation_precedence_are_frozen',
    'scripted_token_estimator_is_immutable_by_canonical_input_identity',
    'reusable_model_provider_contract_suite_passes_via_public_port_only',
  ]) {
    assert(
      new RegExp(`(?:async\\s+)?fn\\s+${testName}\\s*\\(`).test(`${model}\n${scripted}`),
      `Stage 15 permanent provider test is absent: ${testName}`,
    );
  }

  assert(
    /ModelTargetSnapshot::from_validated_config\(config\.models\(\)\)/.test(bootstrap) &&
      /ModelSelectionPolicy::new\(model_targets\)/.test(bootstrap) &&
      !/ScriptedProvider/.test(bootstrap),
    'Stage 15 bootstrap must compose only the immutable target snapshot and selector',
  );
  assert(!/^reqwest\s*=/m.test(cargoManifest), 'Reqwest must remain absent through Stage 15');
  assert(
    !/ContextManifestId::generate|ContextManifest::(?:new|try_new)/.test(`${model}\n${selection}\n${provider}\n${scripted}`),
    'Stage 16 context manifest construction is forbidden in Stage 15',
  );
  assert(
    stage15ReadinessViolations(bootstrap).length === 0,
    'Stage 15 must not activate Scheduler/WorkRunner or promote readiness',
  );
  return verifyStage15CheckerNegativeProbes();
}

function stage16ContextMutationViolations(path, source) {
  const production = stripRustComments(withoutRustTestModules(source));
  const violations = [];
  const applicationContext = path === 'application/context_assembler.rs';
  const sqliteContext = path === 'adapters/sqlite/context_source_store.rs';
  if (applicationContext) {
    for (const [label, pattern] of [
      ['provider invocation', /\bModelGateway\b|\.invoke_model\s*\(|\.send_request\s*\(/],
      ['tool execution invocation', /\bToolExecutionService\b|\.execute_call\s*\(/],
      ['live Workstation invocation', /\bdyn\s+Workstation\b|Arc<dyn\s+Workstation>|Workstation::|\.inspect_execution\s*\(|\.read_file\s*\(/],
      ['selector or reselection', /\bModelSelectionPolicy\b|\.select_model\s*\(|\.reselect\s*\(/],
      ['mutating StateStore', /\bModelStateStore\b|\bStateStore\b|\.persist_context_manifest\s*\(|\.begin_model_invocation\s*\(/],
      ['content truncation to fit', /\.truncate\s*\(|\.split_at\s*\(|\btruncate_to_fit\b/],
      ['history subset to fit', /\b(?:history|sources|messages)\b[^;\n]{0,180}\.take\s*\(/],
      ['tool removal to fit', /\b(?:tools|tool_definitions)\b[^;\n]{0,180}\.(?:retain|remove|truncate)\s*\(/],
      ['requested output reduction', /requested_output[^;\n]{0,120}(?:saturating_sub|checked_sub|-=|\/\s*2)/],
      ['alternate estimator fallback', /(?:estimate|estimator)[^;\n]{0,180}(?:or_else|unwrap_or_else|fallback)/i],
      ['HashMap canonical ordering', /HashMap[^;\n]{0,160}(?:sources|history|items)|(?:sources|history|items)[^;\n]{0,160}HashMap/],
      ['timestamp in request hash', /(?:request|manifest)_sha256[^;\n]{0,200}(?:created_at|utc_now)|(?:created_at|utc_now)[^;\n]{0,200}(?:request|manifest)_sha256/],
      ['draft model output inclusion', /(?:Streaming|Draft|Partial)[^=]{0,100}=>[^;]{0,160}(?:render|push|ModelInputItem)/],
      ['provider-unknown assistant rendering', /UnknownProviderItem[^=]{0,100}=>[^;]{0,160}(?:prior_assistant|ModelInputRole::Assistant)/],
      ['unknown tool outcome as ordinary result', /OutcomeUnknown[^=]{0,100}=>[^;]{0,180}(?:ToolResult|result_success)/],
      ['standalone manifest persistence', /\.persist_context_manifest\s*\(|\.insert_context_manifest\s*\(/],
      ['mutable prepared manifest', /&mut\s+PreparedContextManifest|prepared_manifest\s*:\s*&mut/],
      ['incomplete request hashing', /request_sha256\s*=.*;[\s\S]{0,240}(?:instructions|tool_definitions)\s*\.(?:push|extend)/],
    ]) {
      if (pattern.test(production)) violations.push(label);
    }
    violations.push(...stage16AssemblerReachabilityViolations(production));
    violations.push(...stage16FinalRequestTopologyViolations(production));
    violations.push(...stage16OutcomeUnknownTopologyViolations(production));
  }
  if (sqliteContext) {
    if (/FROM\s+work_items\s+w\b/i.test(production) && !/w\.conversation_id\s*=\s*\?/i.test(production)) {
      violations.push('missing conversation predicate');
    }
    if (/prior_[A-Za-z0-9_]*[\s\S]{0,500}conversation_work_ordinal\s*<=\s*\?/i.test(production)) {
      violations.push('future ordinal leakage');
    }
    if (/load_prior_[A-Za-z0-9_]*[\s\S]{0,700}FROM\s+work_items\b/i.test(production) &&
        !/conversation_work_ordinal\s*<\s*\?/i.test(production)) {
      violations.push('missing ordinal cutoff');
    }
    if (/ORDER\s+BY\s+(?:[^;]{0,120})?(?:created_at|committed_at|recorded_at)|latest[_ ]message|ORDER\s+BY[^;]*DESC\s+LIMIT\s+1/i.test(production)) {
      violations.push('timestamp or latest-message frontier');
    }
    if (/SELECT[^;]+FROM\s+work_items\s+w\b[^;]+conversation_work_ordinal\s*<\s*\?/is.test(production) &&
        !/ORDER\s+BY/i.test(production)) {
      violations.push('missing deterministic ORDER BY');
    }
    if (/load_exact_trigger[\s\S]{0,900}FROM\s+messages(?![\s\S]{0,500}work_item_inputs)/i.test(production)) {
      violations.push('trigger not loaded through work input relation');
    }
    violations.push(...stage16PriorQueryShapeViolations(production));
  }
  if (/\bsqlx\b/.test(production) && !path.startsWith('adapters/sqlite/')) {
    violations.push('SQLx outside adapter');
  }
  if (/\b(?:reqwest|OpenAI(?:Client|Adapter)?|EventSource|Sse)\b/.test(production)) {
    violations.push('provider transport introduced');
  }
  if (/\b(?:struct|trait|impl)\s+ModelGateway\b/.test(production)) {
    violations.push('ModelGateway introduced');
  }
  if (/\bAgentLoop\b|fn\s+run_agent_loop\s*\(/.test(production)) {
    violations.push('AgentLoop introduced');
  }
  if (/\b(?:struct|impl)\s+(?:Real)?WorkRunner\b/.test(production)) {
    violations.push('production WorkRunner introduced');
  }
  if (path === 'bootstrap/startup.rs' && /(?:\blive_ready\b|mark_ready\s*\()/.test(production)) {
    violations.push('readiness promoted');
  }
  return [...new Set(violations)];
}

function stage16PriorQueryShapeViolations(source) {
  const violations = [];
  for (const name of ['load_prior_works', 'load_prior_messages', 'load_prior_assistant_messages']) {
    if (!new RegExp(`\\bfn\\s+${name}\\b`).test(source)) continue;
    const block = extractRustFunction(source, name);
    if (!/WHERE\s+w\.conversation_id\s*=\s*\?\s+AND\s+w\.conversation_work_ordinal\s*<\s*\?/i.test(block)) {
      violations.push(`${name} does not use the frozen strict prior-work ordinal predicate`);
    }
    if (/conversation_work_ordinal\s*(?:<=|=|>=|>)\s*\?|conversation_work_ordinal\s+BETWEEN/i.test(block)) {
      violations.push(`${name} broadens the active ordinal frontier`);
    }
    if (!/\.bind\s*\(\s*conversation_id\.to_string\s*\(\s*\)\s*\)\s*\.bind\s*\(\s*active_ordinal\.get\s*\(\s*\)\s*\)/s.test(block)) {
      violations.push(`${name} does not bind the exact active ordinal cutoff`);
    }
    if (/\.bind\s*\(\s*active_ordinal\.get\s*\(\s*\)\s*(?:\+|-|\.saturating_add|\.checked_add)/s.test(block)) {
      violations.push(`${name} offsets the active ordinal cutoff`);
    }
    if (/\.(?:filter|filter_map|retain)\s*\([^)]*ordinal/s.test(block) &&
        !/conversation_work_ordinal\s*<\s*\?/i.test(block)) {
      violations.push(`${name} performs application-side prior-work frontier filtering`);
    }
  }
  return violations;
}

function stage16AssemblerReachabilityViolations(source) {
  if (!/\bstruct\s+ContextAssembler\b/.test(source)) return [];
  const functions = rustFunctionBlocks(source);
  const inventory = stage14FunctionInventory(functions);
  const roots = functions.filter((block) =>
    ['assemble', 'assemble_snapshot', 'verify_reconstruction', 'verify_exact_reconstruction',
      'render_model_and_tool_trace', 'render_model_output', 'render_exact_source',
      'render_tool_result'].includes(block.name));
  const reachable = stage15ReachableRustBlocks(roots, inventory);
  const violations = [];
  if (reachable.depthExceeded) {
    violations.push('ContextAssembler helper call graph exceeds the finite analysis bound');
  }
  const aliases = stage15SimpleTypeAliases(source);
  for (const block of reachable) {
    const expanded = stage15ResolveTypeAliases(block.source, aliases);
    if (expanded.depthExceeded) {
      violations.push('ContextAssembler helper type alias exceeds the finite analysis bound');
      continue;
    }
    const value = expanded.resolved;
    if (/\bModelSelectionPolicy\b|\bModelTargetSnapshot\b/.test(value) ||
        /\b(?:selector|selection_policy|target_catalog|target_registry|target_snapshot)\b[\s\S]{0,180}\.\s*(?:select|target|default_target|targets)\s*\(/i.test(value) ||
        /considered_target_ids\s*\(\s*\)[\s\S]{0,100}\.\s*(?:first|last|get|find|next)\s*\(/.test(value) ||
        /\b(?:fallback|alternate|default)[A-Za-z0-9_]*target\b/i.test(value)) {
      violations.push('selector, catalog lookup, default, or alternate target is reachable from ContextAssembler');
    }

    const shortening = /\.\s*(?:truncate|split_off|drain|remove|pop|retain|take|take_while|skip|skip_while)\s*\(|\.\s*split_at\s*\(|\b(?:bytes|items|request|sources|history|tool_definitions|ordered_input_items)\s*\[[^\]\n]*\.\.[^\]\n]*\]/;
    const outputReduction = /requested_output[^;\n]{0,180}(?:saturating_sub|checked_sub|-=|\/=|=\s*[^;\n]*-)|reserved_output[^;\n]{0,180}(?:saturating_sub|checked_sub|-=|\/=)/;
    const budgetFiltering = /(?:budget|limit|fit|token|byte)[\s\S]{0,220}\.(?:filter|filter_map|retain)\s*\(|\.(?:filter|filter_map|retain)\s*\([^;]{0,220}(?:budget|limit|fit|token|byte)/i;
    if (shortening.test(block.body) || outputReduction.test(block.body) || budgetFiltering.test(block.body)) {
      violations.push('semantic request truncation is reachable from ContextAssembler');
    }
  }
  return violations;
}

function stage16SemanticValueEscapeViolations(block, valueName, allowedCalls) {
  const violations = [];
  if (new RegExp(`\\blet\\s+mut\\s+${valueName}\\b|&\\s*mut\\s+${valueName}\\b`).test(block)) {
    violations.push(`${valueName} has a mutable alias`);
  }
  if (new RegExp(`\\b${valueName}\\s*\\[[^\\]]*\\.\\.[^\\]]*\\]`).test(block)) {
    violations.push(`${valueName} is sliced or indexed`);
  }
  for (const match of block.matchAll(
    new RegExp(`(?<![A-Za-z0-9_.:])([A-Za-z_][A-Za-z0-9_]*)\\s*(?:::\\s*<[^;(){}>]+>)?\\s*\\([^(){};]*\\b${valueName}\\b[^(){};]*\\)`, 'g'),
  )) {
    if (!allowedCalls.has(match[1])) {
      violations.push(`${valueName} escapes to ${match[1]}`);
    }
  }
  return violations;
}

function stage16FinalRequestTopologyViolations(source) {
  if (!/\bstruct\s+ContextAssembler\b/.test(source)) return [];
  const violations = [];
  const functions = rustFunctionBlocks(source);
  const constructors = functions.filter((block) => block.name === 'construct_final_model_request');
  const modelRequestConstructors = source.match(/\bModelRequest::try_new\s*\(/g) ?? [];
  if (constructors.length !== 1 || modelRequestConstructors.length !== 1) {
    return ['final ModelRequest must have one exclusive construction function'];
  }
  const constructor = constructors[0];
  const constructorAliases = stage15ResolveTypeAliases(
    constructor.source,
    stage15SimpleTypeAliases(source),
  );
  if (constructorAliases.depthExceeded) {
    violations.push('final ModelRequest constructor type aliases exceed the finite analysis bound');
  }
  const constructed = constructorAliases.resolved.replace(/\(\s*(Model(?:InputItem|ToolDefinition|TextPart))\s*\)/g, '$1');
  for (const [label, pattern] of [
    ['input items are not copied from the complete frozen canonical input', /ordered_input_items:\s*input\.canonical_input_items\.to_vec\s*\(\s*\)/],
    ['instructions are not copied from the complete canonical instruction snapshot', /instructions:\s*input\.canonical_instructions\.to_vec\s*\(\s*\)/],
    ['tools are not copied from the complete Stage 14 projection', /tool_definitions:\s*input\.canonical_tool_definitions\.to_vec\s*\(\s*\)/],
    ['requested output does not come directly from the selected target', /requested_output_limit:\s*input\.target\.requested_output_tokens\s*\(\s*\)/],
    ['provider options do not come directly from the selected target', /provider_native_options:\s*input\.target\.provider_native_options\s*\(\s*\)/],
    ['final input equality guard is absent', /final_request\.ordered_input_items\s*\(\s*\)\s*!=\s*input\.canonical_input_items/],
    ['final tool equality guard is absent', /final_request\.tool_definitions\s*\(\s*\)\s*!=\s*input\.canonical_tool_definitions/],
    ['final requested-output equality guard is absent', /final_request\.requested_output_limit\s*\(\s*\)\s*!=\s*input\.target\.requested_output_tokens\s*\(\s*\)/],
    ['final tool fingerprint guard is absent', /model_toolset_fingerprint\s*\(\s*final_request\.tool_definitions\s*\(\s*\)\s*\)\s*!=\s*input\.expected_toolset_fingerprint/],
  ]) {
    if (!pattern.test(constructed)) violations.push(label);
  }
  if (/\b(?:mut\s+)?(?:ordered_input_items|tool_definitions|requested_output_limit)\b\s*=/.test(constructor.body) ||
      /&\s*mut\s+input\.(?:canonical_input_items|canonical_tool_definitions)/.test(constructor.body)) {
    violations.push('final ModelRequest semantic fields are reconstructed or mutably escaped');
  }

  const assembly = functions.find((block) => block.name === 'assemble_snapshot');
  const reconstruction = functions.find((block) => block.name === 'verify_exact_reconstruction');
  if (!assembly || !reconstruction) return [...violations, 'final request construction roots are absent'];
  for (const [label, block, patterns] of [
    ['assembly', assembly.source, [
      /let\s+canonical_input_items\s*=\s*builder\.freeze_canonical_input_items\s*\(\s*\)\s*\?\s*;/,
      /let\s+requested_output\s*=\s*selected\.requested_output_tokens\s*\(\s*\)\s*;/,
      /let\s+final_request\s*=\s*construct_final_model_request\s*\(\s*FinalModelRequestInput\s*\{[\s\S]*?target:\s*selected\s*,[\s\S]*?canonical_input_items:\s*canonical_input_items\.as_ref\s*\(\s*\)\s*,[\s\S]*?canonical_tool_definitions:\s*&tool_definitions\s*,[\s\S]*?expected_toolset_fingerprint:\s*self\.tool_registry\.model_projection_fingerprint\s*\(\s*\)/,
      /let\s+canonical_request_bytes\s*=\s*final_request\.canonical_bytes\s*\(\s*\)\s*;/,
      /let\s+request_byte_count\s*=\s*u64::try_from\s*\(\s*canonical_request_bytes\.len\s*\(\s*\)\s*\)/,
      /validate_request_byte_limit\s*\(\s*request_byte_count\s*,\s*MAX_CANONICAL_MODEL_REQUEST_BYTES\s*\)/,
      /complete_request_units\s*\(\s*&final_request\s*,\s*request_byte_count\s*\)/,
      /let\s+rendered_request_sha256\s*=\s*final_request\.canonical_sha256\s*\(\s*\)\s*;/,
      /ordered_input_items:\s*canonical_input_items\.clone\s*\(\s*\)\s*,/,
      /tool_definitions:\s*tool_definitions\.clone\s*\(\s*\)\.into_boxed_slice\s*\(\s*\)\s*,/,
      /reserved_output_tokens:\s*requested_output_tokens\s*,/,
      /request:\s*final_request\s*,/,
    ]],
    ['reconstruction', reconstruction.source, [
      /let\s+canonical_input_items\s*=\s*builder\.freeze_canonical_input_items\s*\(\s*\)\s*\?\s*;/,
      /let\s+final_request\s*=\s*construct_final_model_request\s*\(\s*FinalModelRequestInput\s*\{[\s\S]*?target\s*,[\s\S]*?canonical_input_items:\s*canonical_input_items\.as_ref\s*\(\s*\)\s*,[\s\S]*?canonical_tool_definitions:\s*&tool_definitions\s*,[\s\S]*?expected_toolset_fingerprint:\s*manifest\.toolset_fingerprint/,
      /let\s+canonical_request_bytes\s*=\s*final_request\.canonical_bytes\s*\(\s*\)\s*;/,
      /canonical_request_bytes\s*!=\s*prepared\.request\.canonical_bytes\s*\(\s*\)/,
      /final_request\.canonical_sha256\s*\(\s*\)\s*!=\s*manifest\.rendered_request_sha256/,
    ]],
  ]) {
    for (const pattern of patterns) {
      if (!pattern.test(block)) violations.push(`${label} final request conservation topology differs`);
    }
  }

  const assemblyFrozen = assembly.source.slice(assembly.source.indexOf('let canonical_input_items'));
  const reconstructionFrozen = reconstruction.source.slice(
    reconstruction.source.indexOf('let canonical_input_items'),
  );
  violations.push(...stage16SemanticValueEscapeViolations(
    assemblyFrozen,
    'canonical_input_items',
    new Set(['construct_final_model_request']),
  ));
  violations.push(...stage16SemanticValueEscapeViolations(
    reconstructionFrozen,
    'canonical_input_items',
    new Set(['construct_final_model_request']),
  ));
  violations.push(...stage16SemanticValueEscapeViolations(
    assembly.source,
    'tool_definitions',
    new Set(['model_toolset_fingerprint', 'render_tool_sources', 'construct_final_model_request']),
  ));
  violations.push(...stage16SemanticValueEscapeViolations(
    reconstruction.source,
    'tool_definitions',
    new Set(['project_model_tool_definitions', 'render_exact_source', 'construct_final_model_request']),
  ));
  if ((source.match(/\bconstruct_final_model_request\s*\(/g) ?? []).length !== 3) {
    violations.push('final ModelRequest constructor call inventory differs');
  }
  return [...new Set(violations)];
}

function stage16OutcomeUnknownTopologyViolations(source) {
  if (!/\bfn\s+render_tool_result\b/.test(source)) return [];
  const render = rustFunctionBlocks(source).find((block) => block.name === 'render_tool_result');
  if (!render) return ['tool-result renderer topology is absent'];
  const marker = /ToolExecutionState::OutcomeUnknown\s*=>\s*\{/.exec(render.body);
  if (!marker) return ['outcome_unknown mapping arm is absent'];
  const opening = render.body.indexOf('{', marker.index);
  const closing = findMatchingDelimiter(render.body, opening, '{', '}');
  if (closing === -1) return ['outcome_unknown mapping arm is unbalanced'];
  const arm = render.body.slice(opening + 1, closing);
  const returned = /\(\s*ContextSourceKind::SyntheticOutcomeUnknown\s*,\s*"synthetic_tool_outcome_unknown"\s*,([\s\S]+)\)\s*$/.exec(
    arm.trim(),
  );
  const functions = rustFunctionBlocks(source);
  const inventory = stage14FunctionInventory(functions);
  const called = [];
  const returnedExpression = returned?.[1] ?? '';
  for (const name of stage15LocalCallCounts(returnedExpression, inventory).keys()) {
    called.push(...(inventory.get(name) ?? []));
  }
  const reachable = stage15ReachableRustBlocks(called, inventory);
  const topology = [returnedExpression, ...reachable.map((block) => block.source)].join('\n');
  const aliases = stage15ResolveTypeAliases(topology, stage15SimpleTypeAliases(source));
  const expandedTopology = aliases.resolved
    .replace(/\(\s*(ModelInputItem)\s*\)/g, '$1')
    .replace(/\(\s*(Result\s*<[^;{}]+>)\s*\)/g, '$1');
  const violations = [];
  if (reachable.depthExceeded) violations.push('outcome_unknown helper call graph exceeds the finite analysis bound');
  if (aliases.depthExceeded) violations.push('outcome_unknown helper type aliases exceed the finite analysis bound');
  if (!returned || !/ModelInputItem::synthetic_runtime_status\s*\(/.test(expandedTopology)) {
    violations.push('outcome_unknown is not mapped through the synthetic uncertainty representation');
  }
  const safeSemanticMacros = new Set(['json', 'format', 'vec', 'matches']);
  for (const match of expandedTopology.matchAll(/\b([A-Za-z_][A-Za-z0-9_]*)\s*!\s*[({[]/g)) {
    const locallyDefined = new RegExp(`\\bmacro_rules\\s*!\\s*${match[1]}\\b`).test(source);
    if (!safeSemanticMacros.has(match[1]) || locallyDefined) {
      violations.push('outcome_unknown reaches an unresolved semantic macro expansion');
    }
  }
  if (/ModelInputItem\s*::\s*(?:ToolResult|tool_result)\b|\bToolResult\s*::|\btool_result\s*\(|result_success|result_failure/.test(expandedTopology)) {
    violations.push('outcome_unknown reaches an ordinary tool result');
  }
  return violations;
}

function verifyStage16CheckerProbes() {
  const cases = [
    ['missing conversation predicate', 'adapters/sqlite/context_source_store.rs', 'fn load_prior() { sql!("SELECT * FROM work_items w ORDER BY w.work_id"); }'],
    ['future ordinal leakage', 'adapters/sqlite/context_source_store.rs', 'fn load_prior_rows() { sql!("SELECT * FROM work_items w WHERE w.conversation_id = ? AND w.conversation_work_ordinal <= ? ORDER BY w.work_id"); }'],
    ['missing ordinal cutoff', 'adapters/sqlite/context_source_store.rs', 'fn load_prior_rows() { sql!("SELECT * FROM work_items w WHERE w.conversation_id = ? ORDER BY w.work_id"); }'],
    ['timestamp latest message', 'adapters/sqlite/context_source_store.rs', 'fn query() { sql!("SELECT * FROM messages ORDER BY committed_at DESC LIMIT 1"); }'],
    ['missing deterministic order', 'adapters/sqlite/context_source_store.rs', 'fn load_prior_rows() { sql!("SELECT * FROM work_items w WHERE w.conversation_id = ? AND w.conversation_work_ordinal < ?"); }'],
    ['provider invocation', 'application/context_assembler.rs', 'fn assemble(&self) { self.invoke_model(); }'],
    ['ToolExecutionService invocation', 'application/context_assembler.rs', 'fn assemble(service: &ToolExecutionService) { service.execute_call(); }'],
    ['Workstation invocation', 'application/context_assembler.rs', 'fn assemble(machine: &dyn Workstation) { machine.read_file(todo!()); }'],
    ['selector invocation', 'application/context_assembler.rs', 'fn assemble(policy: &ModelSelectionPolicy) { policy.select_model(); }'],
    ['SQLx application use', 'application/context_assembler.rs', 'fn assemble(pool: sqlx::SqlitePool) {}'],
    ['content truncate', 'application/context_assembler.rs', 'fn fit(text: &mut String) { text.truncate(12); }'],
    ['content split', 'application/context_assembler.rs', 'fn fit(text: &str) { text.split_at(12); }'],
    ['history take', 'application/context_assembler.rs', 'fn fit(history: Vec<Item>) { history.into_iter().take(2); }'],
    ['tool removal', 'application/context_assembler.rs', 'fn fit(tool_definitions: &mut Vec<Tool>) { tool_definitions.retain(Tool::small); }'],
    ['requested output lowering', 'application/context_assembler.rs', 'fn fit(mut requested_output: u64) { requested_output -= 1; }'],
    ['alternate estimator', 'application/context_assembler.rs', 'fn estimate(primary: Result<E, X>) { primary.or_else(fallback_estimator); }'],
    ['HashMap source order', 'application/context_assembler.rs', 'fn order(sources: HashMap<Id, Source>) { for source in sources {} }'],
    ['timestamp request hash', 'application/context_assembler.rs', 'fn hash(created_at: Time) { let request_sha256 = digest(created_at); }'],
    ['draft output', 'application/context_assembler.rs', 'fn render(state: State) { match state { Streaming => render(ModelInputItem::text()), _ => {} } }'],
    ['unknown provider assistant', 'application/context_assembler.rs', 'fn render(item: Item) { match item { UnknownProviderItem(x) => ModelInputRole::Assistant, _ => todo!() } }'],
    ['unknown tool as result', 'application/context_assembler.rs', 'fn render(state: State) { match state { OutcomeUnknown => ToolResult::success(), _ => todo!() } }'],
    ['standalone manifest persistence', 'application/context_assembler.rs', 'fn assemble(store: &Store) { store.persist_context_manifest(); }'],
    ['mutable prepared manifest', 'application/context_assembler.rs', 'fn rewrite(value: &mut PreparedContextManifest) {}'],
    ['early request hash', 'application/context_assembler.rs', 'fn build() { let request_sha256 = hash(request); instructions.push(x); tool_definitions.extend(y); }'],
    ['ModelGateway', 'ports/model_gateway.rs', 'pub trait ModelGateway {}'],
    ['AgentLoop', 'application/agent_loop.rs', 'struct AgentLoop; fn run_agent_loop() {}'],
    ['production WorkRunner', 'application/work_runner.rs', 'struct WorkRunner;'],
    ['Reqwest', 'adapters/openai.rs', 'fn call(client: reqwest::Client) {}'],
    ['OpenAI adapter', 'adapters/openai.rs', 'struct OpenAIAdapter;'],
    ['SSE', 'adapters/openai.rs', 'fn stream(event: Sse) {}'],
    ['readiness promotion', 'bootstrap/startup.rs', 'fn start() { mark_ready(); let state = live_ready; }'],
  ];
  for (const [label, path, source] of cases) {
    assert(
      stage16ContextMutationViolations(path, source).length > 0,
      `Stage 16 checker negative probe was not rejected: ${label}`,
    );
  }
  const controls = [
    ['single unique lookup', 'adapters/sqlite/context_source_store.rs', 'fn load_one() { sql!("SELECT * FROM work_items WHERE work_id = ?"); }'],
    ['diagnostic sorting', 'application/diagnostics.rs', 'fn diagnostic(mut rows: Vec<Row>) { rows.sort_by_key(Row::created_at); }'],
    ['durable byte projection', 'application/context_assembler.rs', 'fn render(result: DurableResult) { let returned_inline = result.returned_inline; }'],
    ['immutable source iteration', 'application/context_assembler.rs', 'fn render(sources: &[Source]) { for source in sources.iter() { inspect(source); } }'],
    ['constructor-local builder', 'application/context_assembler.rs', 'fn build() { let mut sources = Vec::new(); sources.push(source); ContextPackage::new(sources); }'],
    ['metadata clock', 'application/context_assembler.rs', 'fn metadata(clock: &Clock) { let created_at = clock.utc_now(); store_metadata(created_at); }'],
  ];
  for (const [label, path, source] of controls) {
    assert(
      stage16ContextMutationViolations(path, source).length === 0,
      `Stage 16 checker false-positive control was rejected: ${label}`,
    );
  }
  return { negativeProbeCount: cases.length, falsePositiveControlCount: controls.length };
}

function verifyStage16ContextStructure(rustRoot, productionFiles) {
  const contextPath = join(rustRoot, 'application', 'context_assembler.rs');
  const portPath = join(rustRoot, 'ports', 'context_source_store.rs');
  const sqlitePath = join(rustRoot, 'adapters', 'sqlite', 'context_source_store.rs');
  for (const path of [contextPath, portPath, sqlitePath]) {
    assert(existsSync(path), `Stage 16 required module is absent: ${relative(rustRoot, path)}`);
  }
  const context = readFileSync(contextPath, 'utf8');
  const productionContext = stripRustComments(withoutRustTestModules(context));
  const port = readFileSync(portPath, 'utf8');
  const sqlite = stripRustComments(withoutRustTestModules(readFileSync(sqlitePath, 'utf8')));
  for (const file of productionFiles) {
    const violations = stage16ContextMutationViolations(file.path, file.source);
    assert(violations.length === 0, `Stage 16 boundary differs in ${file.path}: ${violations.join(', ')}`);
  }
  assert(/pub\s+struct\s+ContextAssembler\b/.test(productionContext), 'Stage 16 ContextAssembler is absent');
  assert(/selection:\s*&ModelSelectionResult/.test(productionContext), 'ContextAssembler must receive an immutable ModelSelectionResult');
  assert(!/\bModelSelectionPolicy\b/.test(productionContext), 'ContextAssembler must not own selection policy');
  assert(/Arc<dyn\s+ContextSourceStore>/.test(productionContext) && /Arc<dyn\s+TokenEstimator>/.test(productionContext), 'ContextAssembler narrow dependencies differ');
  assert(/MAX_CANONICAL_MODEL_REQUEST_BYTES:\s*u64\s*=\s*16_777_216/.test(context), 'Stage 16 request byte ceiling differs');
  assert(/context_limit_exceeded/.test(readFileSync(join(rustRoot, 'domain', 'error.rs'), 'utf8')), 'context_limit_exceeded code is absent');
  assert(/HistoricalReasoningSummary/.test(readFileSync(join(rustRoot, 'domain', 'model.rs'), 'utf8')), 'provider-neutral reasoning summary input is absent');
  assert(/trait\s+ContextSourceStore\b/.test(port) && !/\bsqlx\b/.test(port), 'ContextSourceStore is not a narrow SQLx-free port');
  assert(/fn\s+reload_context_sources\s*\(/.test(port) && /ContextReconstructionRequest/.test(port),
    'exact manifest-source reconstruction reload API is absent');
  assert(/\.inner\.pool\.begin\s*\(\s*\)/.test(sqlite), 'SQLite context read must begin one transaction');
  assert(sqlite.indexOf('SELECT max(journal_offset) FROM journal_events') < sqlite.indexOf('load_active_work'), 'SQLite snapshot frontier must be established before eligibility reads');
  assert(/conversation_work_ordinal\s*<\s*\?/.test(sqlite), 'prior context query lacks strict ordinal cutoff');
  assert((sqlite.match(/w\.conversation_id\s*=\s*\?/g) ?? []).length >= 4, 'context history queries do not consistently bind conversation ID');
  assert((sqlite.match(/ORDER\s+BY/g) ?? []).length >= 4, 'canonical multi-row context queries lack deterministic ORDER BY');
  assert(/work_item_inputs[\s\S]{0,500}relationship\s*=\s*'trigger'[\s\S]{0,200}ordinal_within_work\s*=\s*1/.test(sqlite), 'active trigger is not loaded by exact Work input relationship');
  assert(!/ORDER\s+BY[^;]*(?:created_at|committed_at|recorded_at)/i.test(sqlite), 'wall-clock order is forbidden for causal history');
  assert(/load_continuation_boundaries\s*\(&mut transaction, conversation_id, active_ordinal\)/.test(sqlite) &&
      /provider_outcome_unknown/.test(sqlite) && /outcome_unknown/.test(sqlite),
    'durable continuation barrier facts are not loaded inside the eligibility transaction');
  const reconstruction = extractRustFunction(productionContext, 'verify_reconstruction');
  assert(/reload_context_sources\s*\(/.test(reconstruction) &&
      !/load_context_eligibility_snapshot\s*\(/.test(reconstruction),
    'reconstruction must reload exact manifest sources instead of current eligibility');
  assert(/ModelRequest::try_new[\s\S]{0,700}instructions:[\s\S]{0,300}tool_definitions:[\s\S]{0,500}provider_native_options:/.test(productionContext), 'complete provider-neutral request is not constructed before hashing');
  const assembly = extractRustFunction(productionContext, 'assemble_snapshot');
  const byteCeiling = assembly.indexOf('validate_request_byte_limit');
  const estimatorIdentity = assembly.indexOf('self.estimator.identity()');
  const estimatorCall = assembly.indexOf('.estimator\n            .estimate');
  assert(byteCeiling !== -1 && estimatorIdentity !== -1 && estimatorCall !== -1 &&
      byteCeiling < estimatorIdentity && byteCeiling < estimatorCall,
    'request byte ceiling must be enforced before estimator identity/call work');
  assert(/let\s+rendered_request_sha256\s*=\s*final_request\.canonical_sha256\s*\(\s*\)/.test(productionContext), 'authoritative request hash is not derived from complete ModelRequest');
  const manifestHash = extractRustFunction(productionContext, 'semantic_manifest_hash');
  assert(!/created_at|utc_now/.test(manifestHash), 'created_at leaked into semantic manifest hash');
  assert(!/persist_context_manifest|insert_context_manifest/.test(productionContext), 'Stage 16 independently persists successful manifests');
  assert(/reserved_output_tokens:\s*requested_output_tokens/.test(productionContext), 'reserved output is not the selected requested output');
  assert(/omitted_source_count:\s*0/.test(productionContext), 'V0 mandatory history omission count must remain zero');
  assert(/ContextManifestId::generate\s*\(\s*\)/.test(productionContext) && /LogicalInvocationId::generate\s*\(\s*\)/.test(productionContext), 'Stage 16 immutable UUIDv7 IDs are absent');
  assert(stage17PlusImplementationLeaks(productionFiles).length === 0, 'Stage 17+ implementation crossed the Stage 16 boundary');
  const probes = verifyStage16CheckerProbes();
  const compilation = verifyStage16CompilationGatedProbes();
  return {
    structuralNegativeProbeCount: probes.negativeProbeCount,
    structuralFalsePositiveControlCount: probes.falsePositiveControlCount,
    negativeProbeCount: probes.negativeProbeCount + compilation.negativeProbeCount,
    falsePositiveControlCount: probes.falsePositiveControlCount + compilation.controlCount,
    compilationGatedNegativeProbeCount: compilation.negativeProbeCount,
    compilationGatedFalsePositiveControlCount: compilation.controlCount,
  };
}

function verifyStage16ProbeRepository() {
  const paths = [
    'backend/src/application/context_assembler.rs',
    'backend/src/ports/context_source_store.rs',
    'backend/src/adapters/sqlite/context_source_store.rs',
  ];
  const violations = paths.flatMap((path) => stage16ContextMutationViolations(
    relative(join(repositoryRoot, 'backend', 'src'), join(repositoryRoot, path)),
    readFileSync(join(repositoryRoot, path), 'utf8'),
  ));
  assert(violations.length === 0, `Stage 16 probe boundary differs: ${violations.join(', ')}`);
}

function stage16MutateFunction(source, name, mutate) {
  const block = extractRustFunction(source, name);
  const changed = mutate(block);
  assert(changed !== block, `Stage 16 probe did not mutate Rust function ${name}`);
  return source.replace(block, changed);
}

function stage16InjectAssembleStatement(source, statement) {
  return stage16MutateFunction(source, 'assemble', (block) => block.replace(
    /\{\s*let snapshot = self/,
    `{\n        ${statement}\n        let snapshot = self`,
  ));
}

function stage16AppendReachableHelper(source, helper, call) {
  return stage15AppendProductionHelper(stage16InjectAssembleStatement(source, call), helper);
}

function stage16ReplaceOutcomeUnknownArm(source, replacement) {
  return stage16MutateFunction(source, 'render_tool_result', (block) => {
    const marker = /ToolExecutionState::OutcomeUnknown\s*=>\s*\{/.exec(block);
    assert(marker, 'Stage 16 outcome_unknown mutation anchor differs');
    const opening = block.indexOf('{', marker.index);
    const closing = findMatchingDelimiter(block, opening, '{', '}');
    assert(closing !== -1, 'Stage 16 outcome_unknown mutation arm is unbalanced');
    return `${block.slice(0, opening)}{${replacement}}${block.slice(closing + 1)}`;
  });
}

function stage16CompilationProbeDefinitions() {
  const applicationPath = 'backend/src/application/context_assembler.rs';
  const sqlitePath = 'backend/src/adapters/sqlite/context_source_store.rs';
  const directUnknownResult = `
        let projection = json!({"result_kind": "failure", "outcome": "unknown"});
        (
            ContextSourceKind::ObservedToolResult,
            "observed_tool_result",
            ModelInputItem::tool_result(call_id, projection).map_err(contract_error)?,
        )
      `;
  return {
    negatives: [
      {
        label: 'mutable prepared manifest',
        path: applicationPath,
        mutate: (source) => stage15AppendProductionHelper(source, `
          fn stage16_forbidden_manifest_mutation(value: &mut PreparedContextManifest) {
              value.omitted_source_count = 1;
          }`),
      },
      {
        label: 'future cutoff less-than becomes less-than-or-equal',
        path: sqlitePath,
        mutate: (source) => stage16MutateFunction(source, 'load_prior_works', (block) =>
          block.replace('w.conversation_work_ordinal < ?', 'w.conversation_work_ordinal <= ?')),
      },
      {
        label: 'future cutoff predicate removed',
        path: sqlitePath,
        mutate: (source) => stage16MutateFunction(source, 'load_prior_works', (block) =>
          block
            .replace(' AND w.conversation_work_ordinal < ?', '')
            .replace('\n    .bind(active_ordinal.get())', '')),
      },
      {
        label: 'future cutoff bound to active ordinal plus one',
        path: sqlitePath,
        mutate: (source) => stage16MutateFunction(source, 'load_prior_works', (block) =>
          block.replace('.bind(active_ordinal.get())', '.bind(active_ordinal.get() + 1)')),
      },
      {
        label: 'broad same-conversation query followed by application filtering',
        path: sqlitePath,
        mutate: (source) => stage16MutateFunction(source, 'load_prior_works', (block) =>
          block
            .replace(' AND w.conversation_work_ordinal < ?', '')
            .replace('\n    .bind(active_ordinal.get())', '')
            .replace(
              'rows.iter().map(decode_work_source).collect()',
              `let decoded = rows.iter().map(decode_work_source).collect::<Result<Vec<_>, _>>()?;
    Ok(decoded
        .into_iter()
        .filter(|work| work.ordinal < active_ordinal)
        .collect())`,
            )),
      },
      {
        label: 'direct selector call reachable from assembler',
        path: applicationPath,
        mutate: (source) => stage16InjectAssembleStatement(source, `
          if let Some(policy) = Option::<&crate::application::model_selection::ModelSelectionPolicy>::None {
              let _ = policy.select(None, selection.required_capabilities());
          }`),
      },
      {
        label: 'neutral helper wraps selector',
        path: applicationPath,
        mutate: (source) => stage16AppendReachableHelper(source, `
          fn stage16_route(
              policy: &crate::application::model_selection::ModelSelectionPolicy,
              selection: &ModelSelectionResult,
          ) {
              let _ = policy.select(None, selection.required_capabilities());
          }`, `
          if let Some(policy) = Option::<&crate::application::model_selection::ModelSelectionPolicy>::None {
              stage16_route(policy, selection);
          }`),
      },
      {
        label: 'type alias hides selection policy',
        path: applicationPath,
        mutate: (source) => stage16AppendReachableHelper(source, `
          type Stage16RouteEngine = crate::application::model_selection::ModelSelectionPolicy;
          fn stage16_route_alias(policy: &Stage16RouteEngine, selection: &ModelSelectionResult) {
              let _ = policy.select(None, selection.required_capabilities());
          }`, `
          if let Some(policy) = Option::<&Stage16RouteEngine>::None {
              stage16_route_alias(policy, selection);
          }`),
      },
      {
        label: 'helper chooses alternate considered target',
        path: applicationPath,
        mutate: (source) => stage16AppendReachableHelper(source, `
          fn stage16_alternate(selection: &ModelSelectionResult) -> Option<&crate::domain::ModelTargetId> {
              selection.considered_target_ids().last()
          }`, 'let _ = stage16_alternate(selection);'),
      },
      {
        label: 'neutral helper returns first N canonical inputs',
        path: applicationPath,
        mutate: (source) => stage16AppendReachableHelper(source, `
          fn stage16_first_n<T>(values: Vec<T>, count: usize) -> Vec<T> {
              values.into_iter().take(count).collect()
          }`, 'let _ = stage16_first_n(Vec::<ModelInputItem>::new(), 1);'),
      },
      {
        label: 'request byte iterator take',
        path: applicationPath,
        mutate: (source) => stage16AppendReachableHelper(source, `
          fn stage16_byte_take(bytes: &[u8], byte_budget: usize) -> Vec<u8> {
              bytes.iter().copied().take(byte_budget).collect()
          }`, 'let _ = stage16_byte_take(&[], 0);'),
      },
      {
        label: 'string or byte prefix slicing helper',
        path: applicationPath,
        mutate: (source) => stage16AppendReachableHelper(source, `
          fn stage16_byte_prefix(bytes: &[u8], byte_budget: usize) -> &[u8] {
              &bytes[..byte_budget.min(bytes.len())]
          }`, 'let _ = stage16_byte_prefix(&[], 0);'),
      },
      {
        label: 'helper removes last input item',
        path: applicationPath,
        mutate: (source) => stage16AppendReachableHelper(source, `
          fn stage16_remove_last(mut items: Vec<ModelInputItem>) -> Vec<ModelInputItem> {
              items.pop();
              items
          }`, 'let _ = stage16_remove_last(Vec::new());'),
      },
      {
        label: 'helper removes tool definition',
        path: applicationPath,
        mutate: (source) => stage16AppendReachableHelper(source, `
          fn stage16_remove_tool(mut tool_definitions: Vec<ModelToolDefinition>) -> Vec<ModelToolDefinition> {
              if !tool_definitions.is_empty() { tool_definitions.remove(0); }
              tool_definitions
          }`, 'let _ = stage16_remove_tool(Vec::new());'),
      },
      {
        label: 'helper lowers requested output reserve',
        path: applicationPath,
        mutate: (source) => stage16AppendReachableHelper(source, `
          fn stage16_lower_output(requested_output: u64) -> u64 {
              requested_output.saturating_sub(1)
          }`, 'let _ = stage16_lower_output(1);'),
      },
      {
        label: 'direct outcome_unknown to ordinary ToolResult',
        path: applicationPath,
        mutate: (source) => stage16ReplaceOutcomeUnknownArm(source, directUnknownResult),
      },
      {
        label: 'neutral helper maps outcome_unknown to ToolResult',
        path: applicationPath,
        mutate: (source) => stage15AppendProductionHelper(
          stage16ReplaceOutcomeUnknownArm(source, `
            (
                ContextSourceKind::ObservedToolResult,
                "observed_tool_result",
                stage16_unknown_item(call_id)?,
            )`), `
          fn stage16_unknown_item(call_id: ModelToolCallId) -> Result<ModelInputItem, ContextAssemblyError> {
              ModelInputItem::tool_result(call_id, json!({"outcome": "unknown"})).map_err(contract_error)
          }`),
      },
      {
        label: 'outcome_unknown becomes fake failed result',
        path: applicationPath,
        mutate: (source) => stage16ReplaceOutcomeUnknownArm(source, directUnknownResult.replace(
          '"outcome": "unknown"', '"error": "unknown", "result_kind": "failure"')),
      },
      {
        label: 'alias helper maps outcome_unknown to ordinary result',
        path: applicationPath,
        mutate: (source) => stage15AppendProductionHelper(
          stage16ReplaceOutcomeUnknownArm(source, `
            (
                ContextSourceKind::ObservedToolResult,
                "observed_tool_result",
                stage16_alias_unknown(call_id)?,
            )`), `
          type Stage16UnknownRendered = ModelInputItem;
          fn stage16_alias_unknown(call_id: ModelToolCallId) -> Result<Stage16UnknownRendered, ContextAssemblyError> {
              ModelInputItem::tool_result(call_id, json!({"result_kind": "failure"})).map_err(contract_error)
          }`),
      },
      {
        label: 'previous bypass A uses a neutral helper to remove the final canonical input',
        path: applicationPath,
        mutate: (source) => stage15AppendProductionHelper(
          stage16MutateFunction(source, 'assemble_snapshot', (block) => block.replace(
            'let canonical_input_items = builder.freeze_canonical_input_items()?;',
            `let canonical_input_items = builder.freeze_canonical_input_items()?;
        let canonical_input_items = stage16_shape_a(canonical_input_items);`,
          )), `
          fn stage16_shape_a(values: Box<[ModelInputItem]>) -> Box<[ModelInputItem]> {
              let mut owned = values.into_vec();
              let final_index = owned.len() - 1;
              owned.swap_remove(final_index);
              owned.into_boxed_slice()
          }`),
      },
      {
        label: 'previous bypass B checks a neutral byte prefix instead of the final request',
        path: applicationPath,
        mutate: (source) => stage15AppendProductionHelper(
          stage16MutateFunction(source, 'assemble_snapshot', (block) => block.replace(
            'let request_byte_count = u64::try_from(canonical_request_bytes.len())',
            'let request_byte_count = u64::try_from(stage16_shape_b(&canonical_request_bytes).len())',
          )), `
          fn stage16_shape_b(bytes: &[u8]) -> &[u8] {
              &bytes[..bytes.len().saturating_sub(1)]
          }`),
      },
      {
        label: 'previous bypass C removes one real tool with swap_remove',
        path: applicationPath,
        mutate: (source) => stage15AppendProductionHelper(
          stage16MutateFunction(source, 'assemble_snapshot', (block) => block.replace(
            'let toolset_fingerprint = model_toolset_fingerprint(&tool_definitions);',
            `let tool_definitions = stage16_shape_c(tool_definitions);
        let toolset_fingerprint = model_toolset_fingerprint(&tool_definitions);`,
          )), `
          fn stage16_shape_c(mut values: Vec<ModelToolDefinition>) -> Vec<ModelToolDefinition> {
              values.swap_remove(0);
              values
          }`),
      },
      {
        label: 'previous bypass D lowers the selected requested output by one',
        path: applicationPath,
        mutate: (source) => stage15AppendProductionHelper(
          stage16MutateFunction(source, 'construct_final_model_request', (block) => block.replace(
            'requested_output_limit: input.target.requested_output_tokens(),',
            'requested_output_limit: stage16_shape_d(input.target.requested_output_tokens()),',
          )), `
          fn stage16_shape_d(limit: crate::domain::TokenCount) -> crate::domain::TokenCount {
              crate::domain::TokenCount::try_new(limit.get() - 1).expect("positive configured limit")
          }`),
      },
      {
        label: 'novel input slice copies all but the final canonical item',
        path: applicationPath,
        mutate: (source) => stage15AppendProductionHelper(
          stage16MutateFunction(source, 'assemble_snapshot', (block) => block.replace(
            'let canonical_input_items = builder.freeze_canonical_input_items()?;',
            `let canonical_input_items = builder.freeze_canonical_input_items()?;
        let canonical_input_items = stage16_shape_e(canonical_input_items);`,
          )), `
          fn stage16_shape_e(values: Box<[ModelInputItem]>) -> Box<[ModelInputItem]> {
              values[..values.len() - 1].to_vec().into_boxed_slice()
          }`),
      },
      {
        label: 'novel neutral manual loop stops before the final canonical item',
        path: applicationPath,
        mutate: (source) => stage15AppendProductionHelper(
          stage16MutateFunction(source, 'assemble_snapshot', (block) => block.replace(
            'let canonical_input_items = builder.freeze_canonical_input_items()?;',
            `let canonical_input_items = builder.freeze_canonical_input_items()?;
        let canonical_input_items = stage16_shape_f(canonical_input_items);`,
          )), `
          fn stage16_shape_f(values: Box<[ModelInputItem]>) -> Box<[ModelInputItem]> {
              let mut copied = Vec::new();
              let stopping_point = values.len().saturating_sub(1);
              let mut position = 0;
              while position < stopping_point {
                  copied.push(values[position].clone());
                  position += 1;
              }
              copied.into_boxed_slice()
          }`),
      },
      {
        label: 'novel manual tool projection skips one known definition',
        path: applicationPath,
        mutate: (source) => stage15AppendProductionHelper(
          stage16MutateFunction(source, 'assemble_snapshot', (block) => block.replace(
            'let toolset_fingerprint = model_toolset_fingerprint(&tool_definitions);',
            `let tool_definitions = stage16_shape_g(tool_definitions);
        let toolset_fingerprint = model_toolset_fingerprint(&tool_definitions);`,
          )), `
          fn stage16_shape_g(values: Vec<ModelToolDefinition>) -> Vec<ModelToolDefinition> {
              let mut copied = Vec::new();
              for value in values {
                  if value.name().as_str() != "read_file" {
                      copied.push(value);
                  }
              }
              copied
          }`),
      },
      {
        label: 'novel output helper uses saturating arithmetic on a neutral parameter',
        path: applicationPath,
        mutate: (source) => stage15AppendProductionHelper(
          stage16MutateFunction(source, 'construct_final_model_request', (block) => block.replace(
            'requested_output_limit: input.target.requested_output_tokens(),',
            'requested_output_limit: stage16_shape_h(input.target.requested_output_tokens()),',
          )), `
          fn stage16_shape_h(value: crate::domain::TokenCount) -> crate::domain::TokenCount {
              crate::domain::TokenCount::try_new(value.get().saturating_sub(1))
                  .expect("positive configured limit")
          }`),
      },
      {
        label: 'novel byte helper copies a shorter vector before the actual gate',
        path: applicationPath,
        mutate: (source) => stage15AppendProductionHelper(
          stage16MutateFunction(source, 'assemble_snapshot', (block) => block.replace(
            'let request_byte_count = u64::try_from(canonical_request_bytes.len())',
            `let gate_material = stage16_shape_i(&canonical_request_bytes);
        let request_byte_count = u64::try_from(gate_material.len())`,
          )), `
          fn stage16_shape_i(bytes: &[u8]) -> Vec<u8> {
              let mut copied = Vec::new();
              let stopping_point = bytes.len().saturating_sub(1);
              let mut position = 0;
              while position < stopping_point {
                  copied.push(bytes[position]);
                  position += 1;
              }
              copied
          }`),
      },
      {
        label: 'previous outcome_unknown alias helper returns a failed ToolResult while synthetic is unused',
        path: applicationPath,
        mutate: (source) => stage15AppendProductionHelper(
          stage16ReplaceOutcomeUnknownArm(source, `
            let unused = ModelInputItem::synthetic_runtime_status(
                "tool_outcome_unknown",
                json!({"outcome": "unknown"}),
            ).map_err(contract_error)?;
            drop(unused);
            (
                ContextSourceKind::ObservedToolResult,
                "observed_tool_result",
                stage16_shape_j(call_id)?,
            )`), `
          type Stage16ShapeJ = ModelInputItem;
          fn stage16_shape_j(call_id: ModelToolCallId) -> Result<Stage16ShapeJ, ContextAssemblyError> {
              Stage16ShapeJ::tool_result(call_id, json!({"result_kind": "failure"}))
                  .map_err(contract_error)
          }`),
      },
      {
        label: 'novel outcome_unknown helper returns a successful ToolResult',
        path: applicationPath,
        mutate: (source) => stage15AppendProductionHelper(
          stage16ReplaceOutcomeUnknownArm(source, `
            (
                ContextSourceKind::ObservedToolResult,
                "observed_tool_result",
                stage16_shape_k(call_id)?,
            )`), `
          fn stage16_shape_k(call_id: ModelToolCallId) -> Result<ModelInputItem, ContextAssemblyError> {
              ModelInputItem::tool_result(call_id, json!({"result_kind": "success", "value": true}))
                  .map_err(contract_error)
          }`),
      },
      {
        label: 'novel outcome_unknown wrapper contains an ordinary ToolResult',
        path: applicationPath,
        mutate: (source) => stage15AppendProductionHelper(
          stage16ReplaceOutcomeUnknownArm(source, `
            (
                ContextSourceKind::ObservedToolResult,
                "observed_tool_result",
                stage16_shape_l(call_id)?.value,
            )`), `
          struct Stage16ShapeL { value: ModelInputItem }
          fn stage16_shape_l(call_id: ModelToolCallId) -> Result<Stage16ShapeL, ContextAssemblyError> {
              Ok(Stage16ShapeL {
                  value: ModelInputItem::tool_result(call_id, json!({"result_kind": "failure"}))
                      .map_err(contract_error)?,
              })
          }`),
      },
      {
        label: 'novel outcome_unknown reaches ToolResult through two neutral helper layers',
        path: applicationPath,
        mutate: (source) => stage15AppendProductionHelper(
          stage16ReplaceOutcomeUnknownArm(source, `
            (
                ContextSourceKind::ObservedToolResult,
                "observed_tool_result",
                stage16_shape_m1(call_id)?,
            )`), `
          fn stage16_shape_m1(call_id: ModelToolCallId) -> Result<ModelInputItem, ContextAssemblyError> {
              stage16_shape_m2(call_id)
          }
          fn stage16_shape_m2(call_id: ModelToolCallId) -> Result<ModelInputItem, ContextAssemblyError> {
              ModelInputItem::tool_result(call_id, json!({"result_kind": "success"}))
                  .map_err(contract_error)
          }`),
      },
      {
        label: 'novel outcome_unknown constructs ToolResult then drops a separate synthetic marker',
        path: applicationPath,
        mutate: (source) => stage16ReplaceOutcomeUnknownArm(source, `
            let ordinary = ModelInputItem::tool_result(
                call_id,
                json!({"result_kind": "failure"}),
            ).map_err(contract_error)?;
            let synthetic = ModelInputItem::synthetic_runtime_status(
                "tool_outcome_unknown",
                json!({"outcome": "unknown"}),
            ).map_err(contract_error)?;
            drop(synthetic);
            (
                ContextSourceKind::ObservedToolResult,
                "observed_tool_result",
                ordinary,
            )`),
      },
    ],
    controls: [
      {
        label: 'constructor-local mutable source builder',
        path: applicationPath,
        mutate: (source) => stage15AppendProductionHelper(source, `
          fn stage16_control_source_builder(mut values: Vec<String>) -> Box<[String]> {
              values.push(String::from("source"));
              values.into_boxed_slice()
          }`),
      },
      {
        label: 'created-at metadata value',
        path: applicationPath,
        mutate: (source) => stage15AppendProductionHelper(source, `
          fn stage16_control_created_at_metadata(created_at: UtcTimestamp) -> UtcTimestamp {
              created_at
          }`),
      },
      {
        label: 'immutable selected target inspection',
        path: applicationPath,
        mutate: (source) => stage16AppendReachableHelper(source, `
          fn stage16_control_selected(selection: &ModelSelectionResult) -> &crate::domain::ModelTarget {
              selection.selected_target()
          }`, 'let _ = stage16_control_selected(selection);'),
      },
      {
        label: 'request byte inspection without truncation',
        path: applicationPath,
        mutate: (source) => stage16AppendReachableHelper(source, `
          fn stage16_control_request_bytes(bytes: &[u8]) -> usize { bytes.len() }`,
          'let _ = stage16_control_request_bytes(&[]);'),
      },
      {
        label: 'ordinary definite failed result remains ToolResult',
        path: applicationPath,
        mutate: (source) => stage15AppendProductionHelper(source, `
          fn stage16_control_definite_failure(call_id: ModelToolCallId) -> Result<ModelInputItem, ContextAssemblyError> {
              ModelInputItem::tool_result(call_id, json!({"result_kind": "failure"})).map_err(contract_error)
          }`),
      },
      {
        label: 'diagnostics-only take outside semantic assembly',
        path: applicationPath,
        mutate: (source) => stage15AppendProductionHelper(source, `
          fn stage16_control_diagnostic_sample(values: Vec<u64>) -> Vec<u64> {
              values.into_iter().take(2).collect()
          }`),
      },
      {
        label: 'immutable iteration over the frozen final input',
        path: applicationPath,
        mutate: (source) => stage16MutateFunction(source, 'assemble_snapshot', (block) => block.replace(
          'let canonical_input_items = builder.freeze_canonical_input_items()?;',
          `let canonical_input_items = builder.freeze_canonical_input_items()?;
        let _canonical_item_count = canonical_input_items.iter().count();`,
        )),
      },
      {
        label: 'exact immutable move of the complete frozen input',
        path: applicationPath,
        mutate: (source) => stage16MutateFunction(source, 'assemble_snapshot', (block) => block.replace(
          'let canonical_input_items = builder.freeze_canonical_input_items()?;',
          `let canonical_input_items = builder.freeze_canonical_input_items()?;
        let canonical_input_items = canonical_input_items;`,
        )),
      },
      {
        label: 'immutable inspection of the complete Stage 14 tool projection',
        path: applicationPath,
        mutate: (source) => stage16MutateFunction(source, 'assemble_snapshot', (block) => block.replace(
          'let toolset_fingerprint = model_toolset_fingerprint(&tool_definitions);',
          `let _projected_tool_count = tool_definitions.iter().count();
        let toolset_fingerprint = model_toolset_fingerprint(&tool_definitions);`,
        )),
      },
      {
        label: 'exact configured requested output is immutably copied',
        path: applicationPath,
        mutate: (source) => stage16MutateFunction(source, 'assemble_snapshot', (block) => block.replace(
          'let requested_output = selected.requested_output_tokens();',
          `let requested_output = selected.requested_output_tokens();
        let _configured_requested_output = requested_output;`,
        )),
      },
      {
        label: 'immutable complete final request byte inspection',
        path: applicationPath,
        mutate: (source) => stage16MutateFunction(source, 'assemble_snapshot', (block) => block.replace(
          'let canonical_request_bytes = final_request.canonical_bytes();',
          `let canonical_request_bytes = final_request.canonical_bytes();
        let _diagnostic_request_byte_count = canonical_request_bytes.len();`,
        )),
      },
      {
        label: 'definite completed tool evidence remains an ordinary ToolResult',
        path: applicationPath,
        mutate: (source) => stage16MutateFunction(source, 'render_tool_result', (block) => block.replace(
          'ToolExecutionState::Completed => {',
          `ToolExecutionState::Completed => {
            let _definite_observed_state = ToolExecutionState::Completed;`,
        )),
      },
      {
        label: 'outcome_unknown direct synthetic uncertainty remains accepted',
        path: applicationPath,
        mutate: (source) => stage16MutateFunction(source, 'render_tool_result', (block) => block.replace(
          'ToolExecutionState::OutcomeUnknown => {',
          `ToolExecutionState::OutcomeUnknown => {
            let _durable_unknown_state = ToolExecutionState::OutcomeUnknown;`,
        )),
      },
      {
        label: 'outcome_unknown synthetic uncertainty helper chain remains accepted',
        path: applicationPath,
        mutate: (source) => stage15AppendProductionHelper(
          stage16MutateFunction(source, 'render_tool_result', (block) => block.replace(
            `ModelInputItem::synthetic_runtime_status("tool_outcome_unknown", details)
                    .map_err(contract_error)?`,
            'stage16_control_synthetic(details)?',
          )), `
          fn stage16_control_synthetic(details: Value) -> Result<ModelInputItem, ContextAssemblyError> {
              stage16_control_synthetic_inner(details)
          }
          fn stage16_control_synthetic_inner(details: Value) -> Result<ModelInputItem, ContextAssemblyError> {
              ModelInputItem::synthetic_runtime_status("tool_outcome_unknown", details)
                  .map_err(contract_error)
          }`),
      },
      {
        label: 'unrelated ToolResult fixture outside Stage 16 outcome mapping',
        path: applicationPath,
        mutate: (source) => `${source.trimEnd()}

#[cfg(test)]
fn stage16_control_fixture(call_id: ModelToolCallId) -> Result<ModelInputItem, ContextAssemblyError> {
    ModelInputItem::tool_result(call_id, json!({"fixture": true})).map_err(contract_error)
}
`,
      },
    ],
  };
}

function stage16RunCompilationCase(probeRepository, targetDirectory, probe, expectRejection) {
  const path = join(probeRepository, probe.path);
  const original = readFileSync(path, 'utf8');
  const mutated = probe.mutate(original);
  assert(mutated !== original, `Stage 16 compilation probe did not mutate source: ${probe.label}`);
  writeFileSync(path, mutated);
  try {
    const compile = spawnSync('cargo', ['check', '--locked', '--workspace', '--all-targets'], {
      cwd: probeRepository,
      encoding: 'utf8',
      env: { ...process.env, CARGO_TARGET_DIR: targetDirectory },
      maxBuffer: 16 * 1024 * 1024,
    });
    assert(
      compile.status === 0,
      `Stage 16 compilation-gated probe did not compile: ${probe.label}: ${
        compile.stderr.trim() || compile.stdout.trim() || `exit status ${compile.status}`
      }`,
    );
    const checker = spawnSync(process.execPath, ['scripts/check-repository.mjs', '--stage16-probe-only'], {
      cwd: probeRepository,
      encoding: 'utf8',
      maxBuffer: 4 * 1024 * 1024,
    });
    if (expectRejection) {
      assert(
        checker.status !== 0 && /Repository invariant failed:/.test(checker.stderr),
        `compiling forbidden mutation was not rejected by the Stage 16 checker: ${probe.label}`,
      );
    } else {
      assert(
        checker.status === 0,
        `compiling legitimate control was rejected by the Stage 16 checker: ${probe.label}: ${
          checker.stderr.trim() || checker.stdout.trim() || `exit status ${checker.status}`
        }`,
      );
    }
  } finally {
    writeFileSync(path, original);
  }
}

function verifyStage16CompilationGatedProbes() {
  const definitions = stage16CompilationProbeDefinitions();
  assert(definitions.negatives.length === 33, 'Stage 16 compilation-gated negative inventory differs');
  assert(definitions.controls.length === 15, 'Stage 16 compilation-gated control inventory differs');
  const temporaryRoot = mkdtempSync(join(tmpdir(), 'craxii-stage16-probes-'));
  const probeRepository = join(temporaryRoot, 'repository');
  const targetDirectory = join(temporaryRoot, 'target');
  try {
    stage15CopyProbeRepository(probeRepository);
    for (const probe of definitions.negatives) {
      stage16RunCompilationCase(probeRepository, targetDirectory, probe, true);
    }
    for (const control of definitions.controls) {
      stage16RunCompilationCase(probeRepository, targetDirectory, control, false);
    }
  } finally {
    rmSync(temporaryRoot, { recursive: true, force: true });
  }
  return { negativeProbeCount: definitions.negatives.length, controlCount: definitions.controls.length };
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

function verifyStage13Boundaries() {
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
    .map((path) => stripRustComments(withoutRustTestModules(readFileSync(path, 'utf8'))))
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
    .filter((path) =>
      withoutRustTestModules(readFileSync(path, 'utf8')).includes(
        'FinalizedArtifact::from_durable_publication(',
      ),
    )
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
  const productionImplementationFiles = rustFiles
    .filter((path) => !/_tests\.rs$/.test(path))
    .map((path) => ({
      path: relative(rustRoot, path),
      source: readFileSync(path, 'utf8'),
    }));
  verifyStage13WorkstationStructure(rustRoot, productionImplementationFiles);
  const stage13CheckerNegativeProbeCount = verifyStage13CheckerNegativeProbes();
  const stage14CheckerNegativeProbeCount = verifyStage14ToolStructure(
    rustRoot,
    productionImplementationFiles,
  );
  const stage15Checker = verifyStage15CanonicalModelStructure(
    rustRoot,
    productionImplementationFiles,
  );
  const stage16Checker = verifyStage16ContextStructure(
    rustRoot,
    productionImplementationFiles,
  );
  const stage15CheckerNegativeProbeCount = stage15Checker.negativeProbeCount;
  const checkerNegativeProbeCount =
    stage13CheckerNegativeProbeCount + stage14CheckerNegativeProbeCount +
    stage15CheckerNegativeProbeCount + stage16Checker.negativeProbeCount;

  assert(
    /^tokio\s*=\s*\{[^\n]*features\s*=\s*\["io-util", "macros", "net", "process", "rt-multi-thread", "signal", "sync", "time"\][^\n]*\}$/m.test(cargoManifest) &&
      /^nix\s*=\s*\{[^\n]*features\s*=\s*\["fs", "process", "signal"\][^\n]*\}$/m.test(cargoManifest),
    'Stage 13 Tokio process and nix process/signal feature sets differ',
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
    stage13CheckerNegativeProbeCount,
    stage14CheckerNegativeProbeCount,
    stage15CheckerNegativeProbeCount,
    stage15RetainedStructuralProbeCount: stage15Checker.retainedStructuralProbeCount,
    stage15CompilationGatedNegativeProbeCount:
      stage15Checker.compilationGatedNegativeProbeCount,
    stage15CompilationGatedCaseCount: stage15Checker.compilationGatedCaseCount,
    stage15BuiltInCompilationGatedProbeCount:
      stage15Checker.builtInCompilationGatedProbeCount,
    stage15NovelChallengeMutationCount: stage15Checker.novelChallengeMutationCount,
    stage15FalsePositiveCompilationGatedControlCount:
      stage15Checker.falsePositiveCompilationGatedControlCount,
    stage16CheckerNegativeProbeCount: stage16Checker.negativeProbeCount,
    stage16CheckerFalsePositiveControlCount: stage16Checker.falsePositiveControlCount,
    stage16CompilationGatedNegativeProbeCount:
      stage16Checker.compilationGatedNegativeProbeCount,
    stage16CompilationGatedFalsePositiveControlCount:
      stage16Checker.compilationGatedFalsePositiveControlCount,
  };
}

try {
  if (process.argv[2] === '--stage15-probe-only') {
    verifyStage15ProbeRepository();
    console.log('Stage 15 structural invariants passed.');
  } else if (process.argv[2] === '--stage16-probe-only') {
    verifyStage16ProbeRepository();
    console.log('Stage 16 structural invariants passed.');
  } else if (process.argv[2] === '--stage16-static-probes-only') {
    const probes = verifyStage16CheckerProbes();
    assert(probes.negativeProbeCount === 31 && probes.falsePositiveControlCount === 6,
      'Stage 16 static probe summary differs');
    console.log('Stage 16 static checker probes passed.');
  } else if (process.argv[2] === '--stage16-compilation-probes-only') {
    const probes = verifyStage16CompilationGatedProbes();
    assert(probes.negativeProbeCount === 33 && probes.controlCount === 15,
      'Stage 16 compilation-gated probe summary differs');
    console.log('Stage 16 compilation-gated checker probes passed: 33 negative, 15 controls.');
  } else {
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
  const stage15 = verifyStage13Boundaries();

  assert(
    directDependencyCount > 0 &&
      stage15.stage13CheckerNegativeProbeCount === 19 &&
      stage15.stage14CheckerNegativeProbeCount === 30 &&
      stage15.stage15RetainedStructuralProbeCount === 65 &&
      stage15.stage15BuiltInCompilationGatedProbeCount === 20 &&
      stage15.stage15CompilationGatedNegativeProbeCount === 28 &&
      stage15.stage15CompilationGatedCaseCount === 33 &&
      stage15.stage15NovelChallengeMutationCount === 8 &&
      stage15.stage15FalsePositiveCompilationGatedControlCount === 5 &&
      stage15.stage15CheckerNegativeProbeCount === 93 &&
      stage15.stage16CheckerNegativeProbeCount === 64 &&
      stage15.stage16CheckerFalsePositiveControlCount === 21 &&
      stage15.stage16CompilationGatedNegativeProbeCount === 33 &&
      stage15.stage16CompilationGatedFalsePositiveControlCount === 15 &&
      stage15.checkerNegativeProbeCount === 206,
    'checker summary evidence is incomplete',
  );
  console.log('Stage 13 retained checker probes: 19.');
  console.log('Stage 14 retained checker probes: 30.');
  console.log('Stage 15 retained structural probes: 65.');
  console.log('Stage 15 compilation-gated negative probes: 28 (20 built-in, 8 novel).');
  console.log('Stage 15 false-positive compilation-gated controls: 5.');
  console.log('Stage 15 checker negative probes passed: 93 (142 total retained).');
  console.log('Stage 16 checker negative probes passed: 64 (31 structural, 33 compilation-gated).');
  console.log('Stage 16 checker positive controls passed: 21 (6 structural, 15 compilation-gated).');
  console.log('Stage 16 compilation-gated negative probes passed: 33.');
  console.log('Stage 16 compilation-gated positive controls passed: 15.');
  console.log('Stage 16 structural invariants passed.');
  }
} catch (error) {
  console.error(`Repository invariant failed: ${error.message}`);
  process.exitCode = 1;
}
