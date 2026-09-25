use q0s_format::q0lang::{compiler, parser, runtime::Runtime};
use q0vm::{Chunk, Value, Vm, DEFAULT_INSTRUCTION_BUDGET};

fn run(source: &str) -> Runtime {
    let mut runtime = Runtime::new();
    let report = runtime.execute_source(source);
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    runtime
}

#[test]
fn range_has_an_ast_and_survives_bytecode_roundtrip() {
    let source = "total = 0\nfor i in range(1, 6, 2) {\n  total = total + i\n}\n";
    let program = parser::parse(source);
    assert!(program.diagnostics.is_empty());
    assert!(
        matches!(&program.statements[1], parser::Statement::ForRange { name, body, .. }
        if name == "i" && body.statement_lines == [3])
    );
    assert_eq!(program.statement_lines, [1, 2]);
    let compiled = compiler::compile(&program);
    assert!(!compiled.fatal);
    assert!(compiled.diagnostics.is_empty());
    let bytes = compiled.chunk.encode().unwrap();
    assert_eq!(&bytes[4..6], &2_u16.to_le_bytes());
    let decoded = Chunk::decode(&bytes).unwrap();
    assert_eq!(decoded, compiled.chunk);
    let mut vm = Vm::new();
    let report = vm.execute(&decoded, DEFAULT_INSTRUCTION_BUDGET);
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    assert_eq!(vm.state.vars["total"], Value::Number(9.0));
    assert_eq!(vm.state.vars["i"], Value::Number(5.0));
    assert_eq!(vm.state.vars.len(), 2, "iteration state stays private");
}

#[test]
fn defaults_descending_fractional_and_empty_ranges() {
    let runtime = run(r#"total = 0
for i in range(4) {
  total = total + i
}
for j in range(4, 1, -1) {
  total = total + j
}
for k in range(0, 1, 0.25) {
  total = total + k
}
kept = 99
for kept in range(5, 2) {
  total = -1000
}
for absent in range(0) {
  total = -1000
}
for also_absent in range(1, 4, -1) {
  total = -1000
}
"#);
    assert_eq!(runtime.vars["total"], Value::Number(16.5));
    assert_eq!(runtime.vars["kept"], Value::Number(99.0));
    assert!(!runtime.vars.contains_key("absent"));
    assert!(!runtime.vars.contains_key("also_absent"));
}

#[test]
fn bounds_run_once_in_order_and_body_cannot_change_progression() {
    let runtime = run(r#"trace.value = 0
pb func bound digit {
  trace.value = trace.value * 10 + digit
  return digit
}
total = 0
for i in range(bound(1), bound(5), bound(2)) {
  total = total + i
  i = 100
}
"#);
    assert_eq!(runtime.vars["trace.value"], Value::Number(152.0));
    assert_eq!(runtime.vars["total"], Value::Number(4.0));
    assert_eq!(runtime.vars["i"], Value::Number(100.0));
}

#[test]
fn break_and_continue_target_the_innermost_loop() {
    let runtime = run(r#"total = 0
for i in range(5) {
  if i == 1 {
    continue
  }
  if i == 4 {
    break
  }
  for j in range(4) {
    if j == 1 {
      continue
    }
    if j == 3 {
      break
    }
    total = total + i * 10 + j
  }
}
x = 0
while true {
  x = x + 1
  if x < 3 {
    continue
  }
  break
}
"#);
    assert_eq!(runtime.vars["total"], Value::Number(106.0));
    assert_eq!(runtime.vars["x"], Value::Number(3.0));
}

#[test]
fn function_ranges_are_local_and_return_cleans_nested_iteration_state() {
    let runtime = run(r#"pb func first limit {
  for i in range(limit) {
    for j in range(10) {
      return i + j + limit
    }
  }
  return -1
}
total = 0
for i in range(4) {
  total = total + first(i)
}
"#);
    assert_eq!(runtime.vars["total"], Value::Number(5.0));
    assert_eq!(runtime.vars["i"], Value::Number(3.0));
    assert!(!runtime.vars.contains_key("j"));
    assert!(!runtime.vars.contains_key("limit"));
}

#[test]
fn recursive_functions_have_independent_range_state() {
    let runtime = run(r#"pb func sum_depth n {
  total = 0
  for i in range(n) {
    total = total + 1 + sum_depth(i)
  }
  return total
}
result = sum_depth(4)
"#);
    assert_eq!(runtime.vars["result"], Value::Number(15.0));
    assert!(!runtime.vars.contains_key("i"));
}

#[test]
fn loop_control_outside_a_loop_or_across_a_function_boundary_is_fatal() {
    for source in [
        "break\n",
        "continue\n",
        "for i in range(1) {\n  pb func bad {\n    break\n  }\n}\n",
        "while true {\n  pb func bad {\n    continue\n  }\n}\n",
    ] {
        let compiled = compiler::compile_source(source);
        assert!(compiled.fatal, "{source}");
        assert!(compiled
            .diagnostics
            .iter()
            .any(|d| d.message.contains("same function")));
        let mut runtime = Runtime::new();
        let report = runtime.execute_source(source);
        assert!(!report.diagnostics.is_empty());
        assert!(runtime.vars.is_empty());
    }
}

#[test]
fn malformed_range_headers_have_actionable_line_diagnostics() {
    for header in [
        "for i in range() {",
        "for i in range(1, 2, 3, 4) {",
        "for player.x in range(4) {",
        "for i range(4) {",
        "for i in other(4) {",
    ] {
        let parsed = parser::parse(&format!("\n{header}\n  leaked = 1\n}}\n"));
        assert!(
            parsed.statements.is_empty(),
            "invalid loop body escaped: {header}"
        );
        assert!(
            parsed
                .diagnostics
                .iter()
                .any(|d| d.line == 2 && d.message.contains("for")),
            "{header}: {:?}",
            parsed.diagnostics
        );
    }
    for control in ["break 2", "continue outer"] {
        let parsed = parser::parse(control);
        assert!(parsed.diagnostics[0]
            .message
            .contains("does not take arguments"));
    }
}

#[test]
fn invalid_dynamic_bounds_and_nonprogressing_steps_stop_with_source_lines() {
    for (range, expected) in [
        ("range(0, 10, 0)", "step must not be zero"),
        ("range(0, 10, -0)", "step must not be zero"),
        ("range(\"no\", 10)", "start must be a finite number"),
        ("range(false)", "stop must be a finite number"),
        ("range(0, 10, unknown)", "step must be a finite number"),
        ("range(0, 10 ^ 400)", "stop must be a finite number"),
        ("range(10 ^ 20, 10 ^ 21, 1)", "too small to advance"),
    ] {
        let mut runtime = Runtime::new();
        let report = runtime.execute_source(&format!(
            "\nfor i in {range} {{\n  continue\n}}\nafter = true\n"
        ));
        assert!(
            report
                .diagnostics
                .iter()
                .any(|d| d.line == 2 && d.message.contains(expected)),
            "{range}: {:?}",
            report.diagnostics
        );
        assert!(!runtime.vars.contains_key("after"));
    }
}

#[test]
fn overflow_exhausts_finite_ranges_without_exposing_infinity() {
    let runtime = run(
        "count = 0\nfor i in range(10 ^ 308, 1.7 * 10 ^ 308, 10 ^ 308) {\n  count = count + 1\n}\n",
    );
    assert_eq!(runtime.vars["count"], Value::Number(1.0));
    assert!(matches!(runtime.vars["i"], Value::Number(n) if n.is_finite()));
}

#[test]
fn continuing_loops_obey_the_shared_instruction_budget() {
    for source in [
        "for i in range(10 ^ 20) {\n  continue\n}\n",
        "while true {\n  continue\n}\n",
    ] {
        let compiled = compiler::compile_source(source);
        assert!(compiled.diagnostics.is_empty());
        let mut vm = Vm::new();
        let report = vm.execute(&compiled.chunk, 60);
        assert_eq!(report.instructions_executed, 60);
        assert!(report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("budget exceeded")));
    }
}
