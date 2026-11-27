//! Tests: programs of their own, beside a program, that call its functions
//! (and the host's), `run` it with variables, and `expect` what they get. A
//! test passes if it runs to the end.

mod common;

use codyscript_core::debug::{self, Action, Breakpoint};
use codyscript_core::{CompiledProgram, Error, ErrorCode, RunOptions, RunResult, Value, value};
use common::*;

/// A fee, at most 300, and tests of it.
fn fees() -> CompiledProgram {
    engine()
        .compile(&value!({
            "ext": { "math": ["add"] },
            "fn": {
                "fee": { "do": [
                    { "set": { "fee": { "x": "amount * rate" } } },
                    { "if": { "x": "fee > 300" }, "do": [{ "ret": 300 }] },
                    { "ret": { "ref": "fee" } }
                ] }
            },
            "tests": {
                "a fee is the rate's share": { "do": [
                    { "expect": { "call": { "fee": { "amount": 1000, "rate": 0.03 } } }, "eq": 30 }
                ] },
                "fees stop at 300": { "do": [
                    { "expect": { "call": { "fee": { "amount": 20000, "rate": 0.03 } } }, "eq": 301, "tag": "cap" }
                ] },
                "the program charges its fee": { "do": [
                    { "expect": { "run": { "Amount": 100 } }, "eq": 3 }
                ] },
                "adds with the host's function": { "do": [
                    { "expect": { "x": "1 + 1 == 2" } },
                    { "expect": { "call": { "math.add": { "a": 1, "b": 2 } } }, "eq": 3 }
                ] }
            },
            "call": { "fee": { "amount": { "ref": "Amount" }, "rate": 0.03 } }
        }))
        .unwrap()
}

fn run_test(program: &CompiledProgram, name: &str) -> Result<RunResult, Error> {
    program.test(name).unwrap().run_blocking(RunOptions::new())
}

/// What `expect` says when `actual` isn't `expected`.
fn mismatch(actual: Value, expected: Value) -> String {
    let program = value!({ "tests": { "t": { "do": [
        { "expect": { "val": actual }, "eq": { "val": expected } }
    ] } }, "do": [] });
    let program = engine().compile(&program).unwrap();
    let error = failure(run_test(&program, "t"));
    assert_eq!(error.code(), ErrorCode::ExpectationFailed);
    error.message().split(" at /").next().unwrap().to_owned()
}

#[test]
fn a_programs_tests_are_listed_in_order_and_found_by_name() {
    let program = fees();
    let names: Vec<&str> = program.tests().map(|t| t.name()).collect();
    assert_eq!(
        names,
        [
            "a fee is the rate's share",
            "fees stop at 300",
            "the program charges its fee",
            "adds with the host's function"
        ]
    );
    assert!(program.test("nope").is_none());
    // A program without tests has none.
    let none = engine().compile(&value!(1)).unwrap();
    assert_eq!(none.tests().len(), 0);
}

#[test]
fn a_test_passes_if_it_runs_to_the_end_and_fails_at_the_expectation_that_doesnt_hold() {
    let program = fees();
    for name in [
        "a fee is the rate's share",
        "the program charges its fee",
        "adds with the host's function",
    ] {
        assert!(run_test(&program, name).is_ok(), "{name}");
    }
    let error = failure(run_test(&program, "fees stop at 300"));
    assert_eq!(error.code(), ErrorCode::ExpectationFailed);
    assert_eq!(
        error.message(),
        "Expected 301, got 300 at /tests/fees stop at 300/do/0 (tag cap)"
    );
    assert_eq!(error.path(), "/tests/fees stop at 300/do/0");
    assert_eq!(error.tag(), Some("cap"));
    // Running the program is untouched by its tests.
    let charged = fees()
        .run_blocking(with_vars(value!({ "Amount": 100 })))
        .unwrap();
    assert_eq!(charged.value, 3);
}

#[test]
fn expect_compares_values_as_values_and_takes_only_booleans_on_its_own() {
    // Numbers by value; objects field by field, in any order.
    let same = value!({ "tests": { "t": { "do": [
        { "expect": 2, "eq": 2.0 },
        { "expect": { "val": { "a": 1, "b": [1, 2] } }, "eq": { "val": { "b": [1, 2], "a": 1.0 } } },
        { "expect": null, "eq": null }
    ] } }, "do": [] });
    assert!(run_test(&engine().compile(&same).unwrap(), "t").is_ok());
    // Lists item by item, in order; values written out.
    assert_eq!(
        mismatch(value!([2, 1]), value!([1, 2])),
        "Expected [1, 2], got [2, 1]"
    );
    assert_eq!(
        mismatch(value!({ "a": "x\"y" }), value!(null)),
        r#"Expected null, got {"a": "x\"y"}"#
    );
    let long: Vec<Value> = (0..100).map(Value::from).collect();
    let shown = mismatch(Value::Array(long), value!(1));
    assert!(shown.ends_with('…'), "{shown}");
    assert!(shown.chars().count() < 150, "{shown}");
    // On its own, `true` or `false`, nothing else.
    let falsy = value!({ "tests": { "t": { "do": [{ "expect": { "x": "1 > 2" } }] } }, "do": [] });
    let error = failure(run_test(&engine().compile(&falsy).unwrap(), "t"));
    assert_eq!(error.code(), ErrorCode::ExpectationFailed);
    assert_eq!(error.message(), "Expected true, got false at /tests/t/do/0");
    let number = value!({ "tests": { "t": { "do": [{ "expect": 1 }] } }, "do": [] });
    let error = failure(run_test(&engine().compile(&number).unwrap(), "t"));
    assert_eq!(error.code(), ErrorCode::TypeError);
    assert_eq!(
        error.message(),
        r#""expect" needs a boolean, or "eq" to compare with, got number at /tests/t/do/0/expect"#
    );
}

#[test]
fn run_runs_the_program_with_only_the_variables_it_names() {
    let program = engine()
        .compile(&value!({
            "tests": {
                "doubles": { "do": [
                    { "set": { "secret": 1 } },
                    { "expect": { "run": { "n": 21 } }, "eq": 42 }
                ] },
                "can't see the test's variables": { "do": [
                    { "set": { "n": 1 } },
                    { "ret": { "run": {} } }
                ] }
            },
            "x": "n * 2"
        }))
        .unwrap();
    assert_eq!(
        run_test(&program, "doubles").unwrap().variables,
        vars(value!({ "secret": 1 }))
    );
    let error = failure(run_test(&program, "can't see the test's variables"));
    assert_eq!(error.code(), ErrorCode::UndefinedVariable);
    assert_eq!(error.path(), "/x");
}

#[test]
fn a_test_takes_the_hosts_variables_and_a_failing_expect_fails_a_program_too() {
    // A test's body is an input, like a function's: an expect needs a block.
    let bare = value!({
        "tests": { "sees the host's": { "expect": { "ref": "seed" }, "eq": 7 } },
        "do": []
    });
    assert_eq!(
        compile_error(&engine(), &bare).path(),
        "/tests/sees the host's"
    );
    let program = engine()
        .compile(&value!({
            "tests": { "sees the host's": { "do": [{ "expect": { "ref": "seed" }, "eq": 7 }] } },
            "do": [{ "expect": { "x": "total >= 0" }, "tag": "positive" }]
        }))
        .unwrap();
    let test = program.test("sees the host's").unwrap();
    assert!(test.run_blocking(with_vars(value!({ "seed": 7 }))).is_ok());
    let error = failure(program.run_blocking(with_vars(value!({ "total": -1 }))));
    assert_eq!(error.code(), ErrorCode::ExpectationFailed);
    assert_eq!(error.tag(), Some("positive"));
}

#[test]
fn a_test_is_debugged_like_a_program_with_its_runs_frames() {
    let program = engine()
        .compile(&value!({
            "tests": { "t": { "do": [
                { "set": { "got": { "run": { "n": 2 } } } },
                { "expect": { "ref": "got" }, "eq": 4 }
            ] } },
            "do": [{ "ret": { "x": "n * 2", "tag": "double" } }]
        }))
        .unwrap();
    // A test's steps are listed, and say whose they are.
    let steps = program.steps();
    let in_test: Vec<&str> = steps
        .iter()
        .filter(|s| s.test == Some("t"))
        .map(|s| s.path)
        .collect();
    assert_eq!(
        in_test,
        [
            "/tests/t/do/0",
            "/tests/t/do/0/set/got",
            "/tests/t/do/0/set/got/run/n",
            "/tests/t/do/1",
            "/tests/t/do/1/expect",
            "/tests/t/do/1/eq"
        ]
    );
    assert_eq!(
        steps
            .iter()
            .find(|s| s.path == "/tests/t/do/1")
            .unwrap()
            .kind
            .as_str(),
        "expect"
    );
    // Paused in the program the test runs: its frame is the program's,
    // called from the test's `run`, with the variables it was given.
    let mut seen = Vec::new();
    let debugger = debug::from_fn(|pause| {
        let frames = pause.frames();
        seen.push((
            pause.step().path.to_owned(),
            frames.len(),
            frames.last().and_then(|f| f.call).map(str::to_owned),
            pause.variable("n").cloned(),
        ));
        Action::Continue
    });
    let options = RunOptions::new().with_debugger([Breakpoint::tag("double")], debugger);
    assert!(program.test("t").unwrap().run_blocking(options).is_ok());
    assert_eq!(
        seen,
        [(
            "/do/0/ret".to_owned(),
            2,
            Some("/tests/t/do/0/set/got".to_owned()),
            Some(value!(2))
        )]
    );
}

#[test]
fn a_debugged_test_ends_as_a_plain_one_does() {
    let program = engine()
        .compile(&value!({
            "types": { "Pair": { "list": "number" } },
            "fn": { "pair": { "list": [{ "ref": "n" }, { "x": "n + 1" }] } },
            "tests": { "t": { "do": [
                { "set": { "got": { "run": { "n": 2 } } } },
                { "expect": { "is": { "ref": "got" }, "type": "Pair" } },
                { "expect": { "problems": { "ref": "got" }, "type": "Pair" }, "eq": { "val": [] } },
                { "expect": { "ref": "got" }, "eq": { "val": [2, 3] } }
            ] } },
            "call": { "pair": { "n": { "ref": "n" } } }
        }))
        .unwrap();
    let test = program.test("t").unwrap();
    let plain = test.run_blocking(RunOptions::new()).unwrap();
    // From the first step, a step at a time: the same result, and steps.
    let first = program
        .steps()
        .iter()
        .find(|s| s.test.is_some())
        .unwrap()
        .path;
    let mut pauses = 0;
    let debugger = debug::from_fn(|_| {
        pauses += 1;
        Action::StepInto
    });
    let options = RunOptions::new().with_debugger([Breakpoint::path(first)], debugger);
    assert_eq!(test.run_blocking(options).unwrap(), plain);
    assert!(pauses > 10, "{pauses}");
}
