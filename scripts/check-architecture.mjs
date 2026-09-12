#!/usr/bin/env node
// Structural architecture gate. Behavioral and qualification gates remain separate.
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { execFileSync } from "node:child_process";
import { hasForbiddenClientRuntimeReference } from "./architecture-source-guards.mjs";

const root = path.resolve(path.dirname(new URL(import.meta.url).pathname), "..");
const metadata = JSON.parse(execFileSync("cargo", ["metadata", "--format-version", "1", "--no-deps", "--manifest-path", path.join(root,"Cargo.toml")], { encoding:"utf8" }));
const expected = ["pulsar-core","pulsar-protocol","pulsar-engine","pulsar-clients"];
const packages = new Map(metadata.packages.map(p => [p.name,p]));
const failures=[];
const checks=[];
function check(name, condition, detail="") {
  checks.push({name, result:condition?"PASS":"FAIL", detail});
  if(!condition) failures.push(name);
}
for(const name of expected) check("workspace:"+name,packages.has(name));
const allLibraries = metadata.packages.filter(p=>p.targets.some(t=>t.kind.includes("lib"))).map(p=>p.name).sort();
check("exactly-four-product-libraries",JSON.stringify(allLibraries)===JSON.stringify([...expected].sort()),allLibraries.join(","));
const banned = {
 "pulsar-core": ["pulsar-protocol","pulsar-engine","pulsar-clients","eframe","egui","ort","tokio","rusqlite","ureq","reqwest","serialport","wasmtime"],
 "pulsar-protocol": ["pulsar-engine","pulsar-clients","eframe","ort","rusqlite"],
 "pulsar-engine": ["pulsar-clients","eframe","egui"],
 "pulsar-clients": ["pulsar-engine","ort","rusqlite","serialport","wasmtime"],
};
for(const [name, forbidden] of Object.entries(banned)) {
 const pkg=packages.get(name);
 if(!pkg) continue;
 const violations=pkg.dependencies.filter(d=>forbidden.includes(d.name)&&d.kind!=="dev").map(d=>d.name);
 check("dependency-direction:"+name,violations.length===0,violations.join(","));
}
function rustFiles(dir) {
 if(!fs.existsSync(dir)) return [];
 return fs.readdirSync(dir,{withFileTypes:true}).flatMap(e=>e.isDirectory()?rustFiles(path.join(dir,e.name)):e.name.endsWith(".rs")?[path.join(dir,e.name)]:[]);
}
const coreRoot=path.join(root,"crates/pulsar-core/src");
const coreLib=fs.existsSync(path.join(coreRoot,"lib.rs"))?fs.readFileSync(path.join(coreRoot,"lib.rs"),"utf8"):"";
check("core-forbids-unsafe",/#!\[forbid\(unsafe_code\)\]/.test(coreLib));
for(const file of rustFiles(coreRoot)) {
 const text=fs.readFileSync(file,"utf8");
 const violations=[...text.matchAll(/\bstd::(?:fs|net|process|thread)::/g)].map(m=>m[0]);
 check("core-no-effects:"+path.relative(root,file),violations.length===0,violations.join(","));
}
const entry=fs.readFileSync(path.join(root,"src/main.rs"),"utf8");
check("composition-no-legacy-modules",!/^\s*(?:pub\s+)?mod\s+(audio|batch|funscript|gui|kinematics|neural|plugin|signal|stash|sync|tracking|video)\s*;/m.test(entry));
for(const pkg of expected) for(const file of rustFiles(path.join(root,"crates",pkg,"src"))) {
 const text=fs.readFileSync(file,"utf8");
 check("no-legacy-path-import:"+path.relative(root,file),!/#\[path\s*=\s*"[^"]*(?:legacy|\.\.\/\.\.\/\.\.\/src)/.test(text));
 if(pkg==="pulsar-clients") check("no-client-native-runtime:"+path.relative(root,file),!hasForbiddenClientRuntimeReference(text));
}
const productionFiles=[path.join(root,"Cargo.toml"),path.join(root,"Cargo.lock"),path.join(root,"src/main.rs"),...expected.flatMap(pkg=>[path.join(root,"crates",pkg,"Cargo.toml"),...rustFiles(path.join(root,"crates",pkg,"src"))])].filter(f=>fs.existsSync(f)).sort();
const identity=crypto.createHash("sha256");
for(const file of productionFiles) identity.update(path.relative(root,file)).update("\0").update(fs.readFileSync(file)).update("\0");
const report={gate:"phase-a-structure",generated_at:new Date().toISOString(),source_sha256:identity.digest("hex"),checks,passed:failures.length===0,limitations:["Static direct-dependency/source checks only; not proof of transitive runtime behavior.","No neural accuracy, OS/platform, sandbox or physical qualification claim."]};
process.stdout.write(JSON.stringify(report,null,2)+"\n");
process.exitCode=failures.length?1:0;
