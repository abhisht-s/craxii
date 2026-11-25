import { createHash } from 'node:crypto';
import { lstatSync,readdirSync,readFileSync } from 'node:fs';
import { resolve,join } from 'node:path';
import { fileURLToPath } from 'node:url';
// Frozen initial release: wire changes require a new bundle directory and preserved old vectors.
export const frozenBundleDigest='5800dbae670b0cff7eb4c9e7b82572ccf81866d63afb0ca0f36490aa54b7ecdb';
export const schemaNames=['accounting','authentication','compute','error','execution-profile','fixture','lifecycle','model','negotiation','primitives','result','status','work'];
export const fixtureNames=['admission-cancel','authority-lifecycle','budgets-model','canonical','envelopes','legacy-regression','limits','negotiation','neutral-policy-lineage','primitives','raw-invalid','result-delivery','signatures'];
export const expectedFiles=['README.md','SEMANTICS.md','compatibility.md',...schemaNames.map(n=>`schemas/${n}.schema.json`),...fixtureNames.map(n=>`fixtures/${n}.json`)].sort();
export const sha=bytes=>createHash('sha256').update(bytes).digest('hex');
export function canonical(v){if(v===null||typeof v!=='object')return JSON.stringify(v);if(Array.isArray(v))return '['+v.map(canonical).join(',')+']';return '{'+Object.keys(v).sort().map(k=>JSON.stringify(k)+':'+canonical(v[k])).join(',')+'}';}
function assert(ok){if(!ok)throw new Error('contract_bundle_invalid');}
export function checkBundle(root){
  const paths=[];
  function walk(dir,prefix=''){assert(lstatSync(dir).isDirectory()&&!lstatSync(dir).isSymbolicLink());for(const name of readdirSync(dir).sort()){assert(/^[A-Za-z0-9._-]+$/.test(name)&&name!=='.'&&name!=='..');const p=join(dir,name),stat=lstatSync(p),rel=prefix+name;assert(!stat.isSymbolicLink());if(stat.isDirectory())walk(p,rel+'/');else{assert(stat.isFile()&&stat.nlink===1);paths.push(rel);}}}
  walk(root);assert(JSON.stringify(paths.sort())===JSON.stringify([...expectedFiles,'manifest.json'].sort()));
  const bytes=readFileSync(join(root,'manifest.json'));const manifest=JSON.parse(bytes);
  assert(Object.keys(manifest).sort().join(',')==='bundle_digest,bundle_version,files');assert(manifest.bundle_version==='1.0.0');
  const files=expectedFiles.map(path=>{const bytes=readFileSync(join(root,path));assert(bytes.length>0&&bytes.at(-1)===10&&!bytes.includes(13));const text=new TextDecoder('utf-8',{fatal:true}).decode(bytes);assert(!text.includes('/Users/')&&!text.includes('craxii-internal'));return {path,bytes:bytes.length,sha256:sha(bytes)};});
  assert(JSON.stringify(manifest.files)===JSON.stringify(files));const digest=sha(Buffer.from(canonical({bundle_version:'1.0.0',files})));assert(manifest.bundle_digest===digest&&digest===frozenBundleDigest);assert(bytes.toString('utf8')===JSON.stringify({bundle_version:'1.0.0',files,bundle_digest:digest},null,2)+'\n');
  const ids=new Set(schemaNames.map(n=>`urn:craxii:distribution:1.0.0:${n}`));
  function refs(v){if(!v||typeof v!=='object')return;if(v.$ref)assert(ids.has(v.$ref.split('#')[0])&&/^#\/definitions\/[A-Za-z0-9]+$/.test('#'+v.$ref.split('#')[1]));for(const x of Object.values(v))refs(x);}
  for(const name of schemaNames){const schema=JSON.parse(readFileSync(join(root,`schemas/${name}.schema.json`)));assert(schema.$schema==='http://json-schema.org/draft-07/schema#'&&schema.$id===`urn:craxii:distribution:1.0.0:${name}`);refs(schema);}
  for(const name of fixtureNames){const f=JSON.parse(readFileSync(join(root,`fixtures/${name}.json`)));assert(f.fixture_version===1&&f.group===name&&Array.isArray(f.cases)&&f.cases.length>0);const ids=new Set();for(const c of f.cases){assert(/^[a-z][a-z0-9_.-]{0,127}$/.test(c.case_id)&&!ids.has(c.case_id));ids.add(c.case_id);}}
  return manifest;
}
const invoked=process.argv[1]&&resolve(process.argv[1])===fileURLToPath(import.meta.url);
if(invoked){checkBundle(process.argv[2]??'contracts/distribution/1.0.0');console.log('Distribution bundle integrity passed');}
