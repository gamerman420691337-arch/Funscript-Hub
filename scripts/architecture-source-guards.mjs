// Conservative lexical tripwire; Cargo dependency checks remain separate.
// This does not parse Rust comments, strings, use trees, macros, or aliases.
const FORBIDDEN_CLIENT_RUNTIME_REFERENCE =
  /(?:^|[^\p{XID_Continue}])(?:r#)?(?:(?:pulsar_engine|ort|serialport)\s*::|(?:ButtplugDispatcher|HandyDispatcher)(?!\p{XID_Continue}))/u;

export function hasForbiddenClientRuntimeReference(source) {
  return FORBIDDEN_CLIENT_RUNTIME_REFERENCE.test(source);
}
