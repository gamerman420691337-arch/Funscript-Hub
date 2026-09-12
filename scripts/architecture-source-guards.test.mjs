import assert from "node:assert/strict";
import test from "node:test";
import { hasForbiddenClientRuntimeReference } from "./architecture-source-guards.mjs";

const allowed = [
  "project_package_import::PACKAGE_IMPORT_NOTICE",
  "PreparedPackageImport::open(path)",
  "crate::project_package_import::validate_import_handshake(request)",
  "teleport::advance()",
  "transport::connect()",
  "my_pulsar_engine::State",
  "pulsar_engine_adapter::State",
  "serialport2::new()",
  "MockButtplugDispatcher",
  "HandyDispatcherMock",
  "r#teleport::advance()",
  "r#HandyDispatcherMock",
  "_ort::Session",
  "\u03B1ort::Session",
  "ort\u03B1::Session",
  "x\u0301ort::Session",
  "\u03B1HandyDispatcher",
  "HandyDispatcher\u0301",
];

const forbidden = [
  "ort::Session",
  "::ort::Session",
  "crate::ort::Session",
  "pulsar_engine::State",
  "pulsar_engine :: State",
  "serialport::new()",
  "serialport\n::new()",
  "r#ort::Session",
  "vendor::r#pulsar_engine::State",
  "ButtplugDispatcher",
  "drivers::HandyDispatcher::new()",
  "Option<r#HandyDispatcher>",
  "\u03B1::ort::Session",
  "\u03B1::r#HandyDispatcher",
  "project_package_import::open(); teleport::advance(); ort::Session",
  "PreparedPackageImport::open(path); r#serialport :: new()",
];

for (const source of allowed) {
  test("allows unrelated identifier: " + JSON.stringify(source), () => {
    assert.equal(hasForbiddenClientRuntimeReference(source), false);
  });
}
for (const source of forbidden) {
  test("rejects native reference: " + JSON.stringify(source), () => {
    assert.equal(hasForbiddenClientRuntimeReference(source), true);
  });
}

test("repeated checks do not retain regular-expression cursor state", () => {
  for (let i = 0; i < 4; i += 1) {
    assert.equal(hasForbiddenClientRuntimeReference("ort::Session"), true);
    assert.equal(hasForbiddenClientRuntimeReference("project_package_import::open"), false);
  }
});

test("lexical guard remains conservative in comments and strings", () => {
  assert.equal(hasForbiddenClientRuntimeReference("// ort::Session"), true);
  assert.equal(hasForbiddenClientRuntimeReference('let name = "HandyDispatcher";'), true);
});
