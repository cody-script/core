//! Debugging: where a run pauses, what a pause shows, and how each action
//! goes on.

mod common;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use codyscript_core::debug::{self, Action, Breakpoint, Debugger, NodeKind, Pause, Reason};
use codyscript_core::executor::BoxFuture;
use codyscript_core::{
    CompiledProgram, Engine, Error, ErrorCode, Map, RunOptions, RunResult, Signal, Value, value,
};
use common::*;

use Action::{Continue, StepInto, StepOver, Stop};

/// One pause, as the tests check it.
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    reason: Reason,
    path: String,
    /// The variables the step sees, innermost scope winning.
    variables: Map,
    /// Calls running: the frames after the program's.
    depth: usize,
}

fn seen(pause: &Pause<'_>) -> Seen {
    let mut variables = Map::new();
    for scope in pause.scopes() {
        for (name, value) in scope {
            variables.insert(name, value.clone());
        }
    }
    Seen {
        reason: pause.reason(),
        path: pause.step().path.to_owned(),
        variables,
        depth: pause.frames().len() - 1,
    }
}

/// Debugs `program` at `breakpoints`, answering each pause with the next of
/// `actions` (then `Continue`): what it saw at each pause, and the outcome.
fn debug(
    program: &CompiledProgram,
    variables: Value,
    breakpoints: &[&str],
    actions: &[Action],
) -> (Vec<Seen>, Result<RunResult, Error>) {
    let mut log = Vec::new();
    let mut actions: VecDeque<Action> = actions.iter().copied().collect();
    let debugger = debug::from_fn(|pause| {
        log.push(seen(pause));
        actions.pop_front().unwrap_or(Continue)
    });
    let options = with_vars(variables);
    let outcome = program.run_blocking(options.with_debugger(at(breakpoints), debugger));
    (log, outcome)
}

/// Breakpoints at these paths.
fn at(paths: &[&str]) -> Vec<Breakpoint> {
    paths.iter().map(|&path| Breakpoint::path(path)).collect()
}

fn paths(log: &[Seen]) -> Vec<&str> {
    log.iter().map(|s| s.path.as_str()).collect()
}

fn compile(program: &Value) -> CompiledProgram {
    engine().compile(program).unwrap()
}

/// `a = 1`, `b = math.add(a, 2)`, then `b` if it's over 2.
fn straight() -> CompiledProgram {
    compile(&uses_math(m(vec![
        set("a", value!(1)),
        set("b", call("math.add", value!({ "a": var("a"), "b": 2 }))),
        when(x("b > 2"), vec![ret(var("b"))], None),
    ])))
}

/// `i` and `sum` over 1..n, then `sum`.
fn sum_loop() -> CompiledProgram {
    compile(&m(vec![
        set("i", value!(0)),
        set("sum", value!(0)),
        lp(
            x("i < n"),
            vec![set("i", x("i + 1")), set("sum", x("sum + i"))],
        ),
        ret(var("sum")),
    ]))
}

/// `r = fee(amount: a, rate: 0.03)`, then `r`; `fee` caps at 300.
fn with_fee() -> CompiledProgram {
    compile(&with_fn(
        m(vec![
            set(
                "r",
                call("fee", value!({ "amount": var("a"), "rate": 0.03 })),
            ),
            ret(var("r")),
        ]),
        [(
            "fee",
            m(vec![
                set("fee", x("amount * rate")),
                when(x("fee > 300"), vec![ret(value!(300))], None),
                ret(var("fee")),
            ]),
        )],
    ))
}

/// `sum(n)`, recursively: `n + sum(n - 1)`.
fn recursive_sum() -> CompiledProgram {
    compile(&with_fn(
        call("sum", value!({ "n": var("n") })),
        [(
            "sum",
            m(vec![
                when(x("n == 0"), vec![ret(value!(0))], None),
                set("rest", call("sum", value!({ "n": x("n - 1") }))),
                ret(x("n + rest")),
            ]),
        )],
    ))
}

mod pausing {
    use super::*;

    #[test]
    fn a_debugged_run_pauses_only_at_breakpoints() {
        // Not at its start, nor at its end: with no breakpoints, nowhere.
        let (log, outcome) = debug(&straight(), value!({ "n": 1 }), &[], &[]);
        assert!(log.is_empty());
        assert_eq!(outcome.unwrap().value, 3);
        let (log, _) = debug(&straight(), value!({ "n": 1 }), &["/do/1"], &[]);
        assert_eq!(
            log,
            [Seen {
                reason: Reason::Breakpoint,
                path: "/do/1".into(),
                variables: vars(value!({ "n": 1, "a": 1 })),
                depth: 0,
            }]
        );
        // A body that isn't a block is a step of its own, the first.
        let (log, _) = debug(&recursive_sum(), value!({ "n": 0 }), &[""], &[]);
        assert_eq!(paths(&log), [""]);
    }

    #[test]
    fn a_program_with_no_steps_never_pauses() {
        let (log, outcome) = debug(&compile(&m(vec![])), value!({}), &[], &[]);
        assert!(log.is_empty());
        assert_eq!(outcome.unwrap().value, Value::Null);
    }

    #[test]
    fn a_debugged_run_ends_as_a_plain_run_does() {
        let cases = [
            (straight(), value!({})),
            (sum_loop(), value!({ "n": 10 })),
            (with_fee(), value!({ "a": 20000 })),
            (recursive_sum(), value!({ "n": 5 })),
        ];
        for (program, variables) in cases {
            let plain = program.run_blocking(with_vars(variables.clone())).unwrap();
            let last = program.steps().last().unwrap().path;
            let (_, debugged) = debug(&program, variables.clone(), &[last], &[]);
            assert_eq!(debugged.unwrap(), plain);
            // Stepping through every step changes nothing either.
            // From the first step, a step at a time.
            let first = program.steps()[0].path;
            let (log, stepped) = debug(&program, variables, &[first], &[StepInto; 1000]);
            assert_eq!(stepped.unwrap(), plain);
            assert!(log.len() > 5);
        }
    }

    #[test]
    fn a_breakpoint_pauses_before_its_step_on_every_pass() {
        let (log, outcome) = debug(&sum_loop(), value!({ "n": 3 }), &["/do/2/do/1"], &[]);
        assert_eq!(outcome.unwrap().value, 6);
        assert!(log.iter().all(|s| s.reason == Reason::Breakpoint));
        assert_eq!(paths(&log), ["/do/2/do/1"; 3]);
        // Before the step: `i` has moved on, `sum` hasn't yet.
        let at = |s: &Seen| (s.variables["i"].clone(), s.variables["sum"].clone());
        let values: Vec<_> = log.iter().map(at).collect();
        assert_eq!(
            values,
            [
                (value!(1), value!(0)),
                (value!(2), value!(1)),
                (value!(3), value!(3))
            ]
        );
    }

    #[test]
    fn a_loops_test_is_a_step_of_its_own_before_every_pass() {
        let (log, _) = debug(&sum_loop(), value!({ "n": 2 }), &["/do/2/for"], &[]);
        // Twice true, once false.
        assert_eq!(paths(&log), ["/do/2/for"; 3]);
    }

    #[test]
    fn a_breakpoint_that_names_no_step_is_an_error() {
        let breakpoints = [
            Breakpoint::path("/do/1/set"),
            Breakpoint::path("/do/1"),
            Breakpoint::tag("nope"),
        ];
        let mut paused = false;
        let debugger = debug::from_fn(|_| {
            paused = true;
            Continue
        });
        let error = straight()
            .run_blocking(RunOptions::new().with_debugger(breakpoints, debugger))
            .unwrap_err();
        assert!(!paused, "it doesn't start");
        assert_eq!(error.code(), ErrorCode::InvalidConfig);
        assert_eq!(error.path(), "/breakpoints/0");
        assert_eq!(
            error.message(),
            r#"Invalid run configuration: No step at "/do/1/set" at /breakpoints/0"#
        );
        let issues: Vec<(&str, &str)> = error
            .issues()
            .unwrap()
            .iter()
            .map(|i| (i.path.as_str(), i.message.as_str()))
            .collect();
        assert_eq!(
            issues,
            [
                ("/breakpoints/0", r#"No step at "/do/1/set""#),
                ("/breakpoints/2", r#"No step has tag "nope""#),
            ]
        );
    }

    #[test]
    fn a_breakpoint_set_at_a_pause_must_name_a_step_too() {
        let debugger = debug::from_fn(|pause| {
            pause.breakpoints_mut().push(Breakpoint::tag("nope"));
            Continue
        });
        let error = straight()
            .run_blocking(RunOptions::new().with_debugger(at(&["/do/0"]), debugger))
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidConfig);
        assert_eq!(error.path(), "/breakpoints/1");
    }

    #[test]
    fn a_tag_breakpoint_pauses_at_the_step_so_tagged_across_edits() {
        let tagged_fee = |before: Vec<Value>| {
            let mut body = before;
            body.push(tagged(set("fee", x("amount * 0.03")), "fee"));
            body.push(ret(var("fee")));
            compile(&m(body))
        };
        let at_fee = [Breakpoint::tag("fee")];
        let paused_at = |program: &CompiledProgram| {
            let mut paused = Vec::new();
            let debugger = debug::from_fn(|pause| {
                paused.push(pause.step().path.to_owned());
                Continue
            });
            let options = with_vars(value!({ "amount": 100 }));
            program
                .run_blocking(options.with_debugger(at_fee.clone(), debugger))
                .unwrap();
            paused
        };
        assert_eq!(paused_at(&tagged_fee(vec![])), ["/do/0"]);
        // An instruction inserted before it moves the step; the breakpoint
        // moves with it.
        let edited = tagged_fee(vec![set("rate", value!(0.03))]);
        assert_eq!(paused_at(&edited), ["/do/1"]);
        // The path that named it before now names a different step.
        assert_eq!(edited.steps()[0].breakpoint(), Breakpoint::path("/do/0"));
        assert_eq!(edited.steps()[2].breakpoint(), Breakpoint::tag("fee"));
    }

    #[test]
    fn a_tag_breakpoint_pauses_at_every_step_so_tagged() {
        let program = compile(&m(vec![
            tagged(set("a", value!(1)), "rule"),
            set("b", value!(2)),
            tagged(set("c", tagged(value!({ "val": 3 }), "rule")), "other"),
        ]));
        let mut paused = Vec::new();
        let debugger = debug::from_fn(|pause| {
            paused.push(pause.step().path.to_owned());
            Continue
        });
        let breakpoints = [Breakpoint::tag("rule")];
        program
            .run_blocking(RunOptions::new().with_debugger(breakpoints, debugger))
            .unwrap();
        assert_eq!(paused, ["/do/0", "/do/2/set/c"]);
    }

    #[test]
    fn breakpoints_can_change_at_any_pause() {
        let program = sum_loop();
        let mut hits = Vec::new();
        let debugger = debug::from_fn(|pause| {
            hits.push(pause.step().path.to_owned());
            if pause.step().path == "/do/0" {
                // From now on, only inside the loop…
                *pause.breakpoints_mut() = at(&["/do/2/do/0"]);
            } else {
                // …and only once.
                pause.breakpoints_mut().clear();
            }
            Continue
        });
        let options = with_vars(value!({ "n": 5 }));
        let result = program.run_blocking(options.with_debugger(at(&["/do/0", "/do/3"]), debugger));
        assert_eq!(result.unwrap().value, 15);
        assert_eq!(hits, ["/do/0", "/do/2/do/0"]);
    }

    #[test]
    fn a_pause_describes_its_step() {
        let program = compile(&uses_math(m(vec![
            tagged(set("a", tagged(value!({ "val": 1 }), "one")), "first"),
            set("b", m(vec![ret(x("a + 1"))])),
            when(var("go"), vec![], Some(vec![])),
            lp(value!(false), vec![]),
            ret(call("math.add", value!({ "a": var("a"), "b": var("b") }))),
        ])));
        let mut log = Vec::new();
        let debugger = debug::from_fn(|pause| {
            let name = pause.step().name.map(str::to_owned);
            log.push((
                pause.step().path.to_owned(),
                pause.step().kind,
                name,
                pause.step().tag.map(str::to_owned),
                pause.steps(),
            ));
            StepInto
        });
        program
            .run_blocking(with_vars(value!({ "go": true })).with_debugger(at(&["/do/0"]), debugger))
            .unwrap();
        let s = |s: &str| Some(s.to_owned());
        assert_eq!(
            log,
            [
                ("/do/0".into(), NodeKind::Set, s("a"), s("first"), 0),
                ("/do/0/set/a".into(), NodeKind::Val, None, s("one"), 1),
                ("/do/1".into(), NodeKind::Set, s("b"), None, 2),
                ("/do/1/set/b".into(), NodeKind::Do, None, None, 3),
                ("/do/1/set/b/do/0".into(), NodeKind::Ret, None, None, 4),
                (
                    "/do/1/set/b/do/0/ret".into(),
                    NodeKind::X,
                    s("a + 1"),
                    None,
                    5
                ),
                // An expression's operands are steps, not pauses: 3 of them.
                ("/do/2".into(), NodeKind::If, None, None, 9),
                ("/do/2/if".into(), NodeKind::Ref, s("go"), None, 10),
                ("/do/3".into(), NodeKind::For, None, None, 11),
                ("/do/3/for".into(), NodeKind::Val, None, None, 12),
                ("/do/4".into(), NodeKind::Ret, None, None, 13),
                ("/do/4/ret".into(), NodeKind::Call, s("math.add"), None, 14),
                (
                    "/do/4/ret/call/math.add/a".into(),
                    NodeKind::Ref,
                    s("a"),
                    None,
                    15
                ),
                (
                    "/do/4/ret/call/math.add/b".into(),
                    NodeKind::Ref,
                    s("b"),
                    None,
                    16
                ),
            ]
        );
        assert_eq!(NodeKind::Call.as_str(), "call");
        assert_eq!(NodeKind::X.to_string(), "x");
    }

    #[test]
    fn a_pause_is_at_a_step_as_listed() {
        // Into every step of a program with a function: each pause shows the
        // step the listing has for its path, nesting and function included.
        let program = with_fee();
        let listed = program.steps();
        let mut checked = 0;
        let debugger = debug::from_fn(|pause| {
            let step = pause.step();
            assert_eq!(listed.iter().find(|s| s.path == step.path), Some(&step));
            assert_eq!(step.function, pause.frames().last().unwrap().function);
            checked += 1;
            StepInto
        });
        let options = with_vars(value!({ "a": 100 }));
        program
            .run_blocking(options.with_debugger(at(&["/do/0"]), debugger))
            .unwrap();
        assert_eq!(checked, 12);
    }

    #[test]
    fn limits_and_signals_still_apply() {
        let engine = Engine::builder().max_steps(50).build();
        let spin = engine.compile(&m(vec![lp(value!(true), vec![])])).unwrap();
        let error = spin
            .run_blocking(
                RunOptions::new().with_debugger(at(&["/do/0"]), debug::from_fn(|_| Continue)),
            )
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::StepLimit);

        // A signal aborted while paused stops the run at the step it resumes.
        let signal = Signal::new();
        let aborter = signal.clone();
        let options = RunOptions::new().with_signal(signal);
        let debugger = debug::from_fn(move |_| {
            aborter.abort();
            StepInto
        });
        let error = straight()
            .run_blocking(options.with_debugger(at(&["/do/0"]), debugger))
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::Aborted);
        assert_eq!(error.message(), "Run aborted at /do/0");
    }
}

mod stepping {
    use super::*;

    #[test]
    fn step_into_pauses_at_every_step_in_the_order_run() {
        let program = straight();
        let (log, outcome) = debug(&program, value!({}), &["/do/0"], &[StepInto; 100]);
        assert_eq!(outcome.unwrap().value, 3);
        assert_eq!(
            paths(&log),
            [
                "/do/0",
                "/do/0/set/a",
                "/do/1",
                "/do/1/set/b",
                "/do/1/set/b/call/math.add/a",
                "/do/1/set/b/call/math.add/b",
                "/do/2",
                "/do/2/if",
                "/do/2/do/0",
                "/do/2/do/0/ret",
            ]
        );
        assert_eq!(log[0].reason, Reason::Breakpoint);
        assert!(log[1..].iter().all(|s| s.reason == Reason::Step));
        // This program runs every step once, in the order written.
        let steps: Vec<&str> = program.steps().iter().map(|s| s.path).collect();
        assert_eq!(paths(&log), steps);
    }

    #[test]
    fn step_over_takes_the_whole_step() {
        let (log, outcome) = debug(&straight(), value!({}), &["/do/0"], &[StepOver; 100]);
        assert_eq!(outcome.unwrap().value, 3);
        assert_eq!(paths(&log), ["/do/0", "/do/1", "/do/2"]);
        assert_eq!(log[1].variables, vars(value!({ "a": 1 })));
        assert_eq!(log[2].variables, vars(value!({ "a": 1, "b": 3 })));
    }

    #[test]
    fn step_over_a_part_goes_on_to_the_next_part_or_step() {
        let actions = [StepInto, StepInto, StepInto, StepOver, StepInto, StepOver];
        let (log, _) = debug(&straight(), value!({}), &["/do/0"], &actions);
        assert_eq!(
            paths(&log),
            [
                "/do/0",
                "/do/0/set/a",
                "/do/1",
                "/do/1/set/b",
                // Over the call: its arguments and the call itself.
                "/do/2",
                "/do/2/if",
                // Over the test: on to what it decides.
                "/do/2/do/0",
            ]
        );
        // Over the whole `if`, with the `ret` it runs: the run is over.
        let (log, _) = debug(&straight(), value!({}), &["/do/2"], &[StepOver]);
        assert_eq!(paths(&log), ["/do/2"]);
    }

    #[test]
    fn step_over_a_loop_runs_every_pass() {
        let sum = sum_loop();
        let (log, outcome) = debug(&sum, value!({ "n": 4 }), &["/do/2"], &[StepOver]);
        assert_eq!(outcome.unwrap().value, 10);
        // Then the step after the loop: its `ret`, the program's last.
        assert_eq!(paths(&log), ["/do/2", "/do/3"]);
        assert_eq!(log[1].reason, Reason::Step);
        let after = vars(value!({ "n": 4, "i": 4, "sum": 10 }));
        assert_eq!(log[1].variables, after);
        // Going on instead, it doesn't pause there.
        let (log, _) = debug(&sum, value!({ "n": 4 }), &["/do/2"], &[Continue]);
        assert_eq!(paths(&log), ["/do/2"]);
    }

    #[test]
    fn step_over_the_last_step_of_a_pass_goes_to_the_test() {
        let actions = [StepOver, StepOver, StepOver, Stop];
        let (log, outcome) = debug(&sum_loop(), value!({ "n": 4 }), &["/do/2/do/1"], &actions);
        assert_eq!(
            paths(&log),
            ["/do/2/do/1", "/do/2/for", "/do/2/do/0", "/do/2/do/1"]
        );
        assert_eq!(outcome.unwrap_err().code(), ErrorCode::Aborted);
    }

    #[test]
    fn step_over_a_function_call_runs_all_of_it() {
        let actions = [StepInto, StepOver];
        let (log, outcome) = debug(&with_fee(), value!({ "a": 100 }), &["/do/0"], &actions);
        assert_eq!(outcome.unwrap().value, 3);
        assert_eq!(paths(&log), ["/do/0", "/do/0/set/r", "/do/1"]);
        assert_eq!(log[2].variables, vars(value!({ "a": 100, "r": 3 })));
    }

    #[test]
    fn step_into_a_function_call_pauses_in_it_after_the_arguments() {
        let program = with_fee();
        let mut frames = Vec::new();
        let debugger = debug::from_fn(|pause| {
            if pause.step().path == "/fn/fee/do/0" {
                let stack: Vec<(Option<String>, Option<String>, Vec<Map>)> = pause
                    .frames()
                    .iter()
                    .map(|f| {
                        let function = f.function.map(str::to_owned);
                        (function, f.call.map(str::to_owned), f.scopes.to_vec())
                    })
                    .collect();
                frames.push(stack);
                // The function sees its arguments, not its caller's variables.
                assert_eq!(pause.variable("amount"), Some(&value!(100)));
                assert_eq!(pause.variable("a"), None);
                assert_eq!(
                    pause.scopes(),
                    [vars(value!({ "amount": 100, "rate": 0.03 }))]
                );
            }
            StepInto
        });
        let options = with_vars(value!({ "a": 100 }));
        let result = program.run_blocking(options.with_debugger(at(&["/do/0"]), debugger));
        assert_eq!(result.unwrap().value, 3);
        assert_eq!(
            frames,
            [vec![
                (None, None, vec![vars(value!({ "a": 100 }))]),
                (
                    Some("fee".to_owned()),
                    Some("/do/0/set/r".to_owned()),
                    vec![vars(value!({ "amount": 100, "rate": 0.03 }))],
                ),
            ]]
        );

        let (log, _) = debug(&program, value!({ "a": 100 }), &["/do/0"], &[StepInto; 5]);
        assert_eq!(
            paths(&log),
            [
                "/do/0",
                "/do/0/set/r",
                "/do/0/set/r/call/fee/amount",
                "/do/0/set/r/call/fee/rate",
                "/fn/fee/do/0",
                "/fn/fee/do/0/set/fee",
            ]
        );
        assert_eq!(log[4].depth, 1);
    }

    #[test]
    fn stepping_over_a_functions_last_step_returns_to_the_caller() {
        let fee = with_fee();
        let (log, _) = debug(&fee, value!({ "a": 100 }), &["/fn/fee/do/2"], &[StepOver]);
        assert_eq!(paths(&log), ["/fn/fee/do/2", "/do/1"]);
        // An early `ret` too.
        let early = ["/fn/fee/do/1/do/0"];
        let (log, _) = debug(&fee, value!({ "a": 20000 }), &early, &[StepOver]);
        assert_eq!(paths(&log), ["/fn/fee/do/1/do/0", "/do/1"]);
        assert_eq!(log[1].variables["r"], 300);
    }

    #[test]
    fn a_breakpoint_stops_a_step_over_on_the_way() {
        let breakpoints = ["/do/0", "/fn/fee/do/1"];
        let (log, _) = debug(&with_fee(), value!({ "a": 100 }), &breakpoints, &[StepOver]);
        assert_eq!(paths(&log), ["/do/0", "/fn/fee/do/1"]);
        assert_eq!(log[1].reason, Reason::Breakpoint);
    }

    #[test]
    fn step_over_in_recursion_stays_with_the_same_call() {
        // Pause at the recursive call in sum(3), drop the breakpoint, and
        // step over: the next pause is sum(3)'s next step, not one in the
        // calls it makes, though they run the same steps.
        let program = recursive_sum();
        let mut log = Vec::new();
        let debugger = debug::from_fn(|pause| match pause.reason() {
            Reason::Breakpoint => {
                log.push(seen(pause));
                pause.breakpoints_mut().clear();
                StepOver
            }
            Reason::Step => {
                log.push(seen(pause));
                Continue
            }
        });
        let options = with_vars(value!({ "n": 3 }));
        let result = program.run_blocking(options.with_debugger(at(&["/fn/sum/do/1"]), debugger));
        assert_eq!(result.unwrap().value, 6);
        assert_eq!(
            log,
            [
                Seen {
                    reason: Reason::Breakpoint,
                    path: "/fn/sum/do/1".into(),
                    variables: vars(value!({ "n": 3 })),
                    depth: 1,
                },
                Seen {
                    reason: Reason::Step,
                    path: "/fn/sum/do/2".into(),
                    variables: vars(value!({ "n": 3, "rest": 3 })),
                    depth: 1,
                },
            ]
        );
    }

    #[test]
    fn step_over_a_body_that_isnt_a_block_runs_the_program() {
        let (log, outcome) = debug(&recursive_sum(), value!({ "n": 3 }), &[""], &[StepOver]);
        assert_eq!(paths(&log), [""]);
        assert_eq!(outcome.unwrap().value, 6);
    }

    #[test]
    fn stop_fails_the_run_before_the_step() {
        let calls = Arc::new(Mutex::new(0));
        let counter = Arc::clone(&calls);
        let engine = Engine::builder()
            .executor("count", ["it"], move |_| {
                *counter.lock().unwrap() += 1;
                Ok(Value::Null)
            })
            .build();
        let program = engine
            .compile(&with_ext(
                m(vec![
                    set("a", value!(1)),
                    tagged(set("b", call("count.it", value!({}))), "count"),
                ]),
                value!({ "count": ["it"] }),
            ))
            .unwrap();
        let (log, outcome) = debug(&program, value!({}), &["/do/1"], &[Stop]);
        assert_eq!(paths(&log), ["/do/1"]);
        let error = outcome.unwrap_err();
        assert_eq!(error.code(), ErrorCode::Aborted);
        assert_eq!(error.path(), "/do/1");
        assert_eq!(error.tag(), Some("count"));
        assert_eq!(
            error.message(),
            "Run stopped by the debugger at /do/1 (tag count)"
        );
        assert_eq!(*calls.lock().unwrap(), 0);
    }
}

mod debuggers {
    use super::*;

    /// A debugger that takes its time: it waits on a timer before every
    /// answer, holding on to the pause, as one waiting for a person would.
    struct Patient {
        seen: Vec<(String, Option<Value>)>,
    }

    impl Debugger for Patient {
        fn pause<'a>(&'a mut self, mut pause: Pause<'a>) -> BoxFuture<'a, Action> {
            Box::pin(async move {
                sleep(1).await;
                let i = pause.variable("i").cloned();
                self.seen.push((pause.step().path.to_owned(), i));
                // Seen enough of the start: pause at the end.
                pause.breakpoints_mut().push(Breakpoint::path("/do/3"));
                Continue
            })
        }
    }

    #[test]
    fn an_async_debugger_is_awaited_and_may_keep_the_pause() {
        let mut patient = Patient { seen: Vec::new() };
        let options = with_vars(value!({ "n": 3 }));
        // By reference: the host keeps its debugger.
        let result = sum_loop().run_blocking(options.with_debugger(at(&["/do/0"]), &mut patient));
        assert_eq!(result.unwrap().value, 6);
        assert_eq!(
            patient.seen,
            [("/do/0".into(), None), ("/do/3".into(), Some(value!(3)))]
        );
    }

    #[test]
    fn a_boxed_debugger_works_too() {
        let boxed: Box<dyn Debugger> = Box::new(debug::from_fn(|_| Continue));
        let result =
            straight().run_blocking(RunOptions::new().with_debugger(at(&["/do/0"]), boxed));
        assert!(result.is_ok());
    }

    #[test]
    fn a_debugged_run_is_send() {
        fn send<T: Send>(_: &T) {}
        let program = straight();
        let debugger = debug::from_fn(|_| Continue);
        let run = program.run(RunOptions::new().with_debugger(at(&["/do/0"]), debugger));
        send(&run);
    }

    #[test]
    fn a_program_run_once_is_debugged_too() {
        let program = m(vec![set("a", value!(1)), ret(x("a + 1"))]);
        let mut paused = Vec::new();
        let debugger = debug::from_fn(|pause| {
            paused.push(pause.step().path.to_owned());
            Continue
        });
        let options = RunOptions::new().with_debugger(at(&["/do/1"]), debugger);
        let result = Engine::new().run_blocking(&program, options).unwrap();
        assert_eq!(result.value, 2);
        assert_eq!(paused, ["/do/1"]);

        let options = RunOptions::new().with_debugger(at(&["/do/9"]), debug::from_fn(|_| Continue));
        let error = Engine::new().run_blocking(&program, options).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidConfig);
    }
}

mod steps {
    use super::*;

    #[test]
    fn every_step_is_listed_in_the_order_written() {
        let program = engine()
            .compile(&with_fn(
                uses_math(m(vec![
                    set("a", call("math.add", value!({ "a": 1, "b": var("n") }))),
                    tagged(
                        when(
                            x("a > 1"),
                            vec![ret(var("a"))],
                            Some(vec![set("a", m(vec![]))]),
                        ),
                        "check",
                    ),
                    lp(
                        value!(false),
                        vec![ret(call("twice", value!({ "v": var("a") })))],
                    ),
                ])),
                [("twice", x("v * 2"))],
            ))
            .unwrap();
        let steps = program.steps();
        let listed: Vec<_> = steps
            .iter()
            .map(|s| (s.path, s.kind, s.name, s.tag, s.depth))
            .collect();
        let functions: Vec<_> = steps.iter().map(|s| s.function).collect();
        assert_eq!(functions[..15], [None; 15]);
        assert_eq!(functions[15], Some("twice"));
        assert_eq!(
            listed,
            [
                ("/do/0", NodeKind::Set, Some("a"), None, 0),
                ("/do/0/set/a", NodeKind::Call, Some("math.add"), None, 1),
                ("/do/0/set/a/call/math.add/a", NodeKind::Val, None, None, 2),
                (
                    "/do/0/set/a/call/math.add/b",
                    NodeKind::Ref,
                    Some("n"),
                    None,
                    2
                ),
                ("/do/1", NodeKind::If, None, Some("check"), 0),
                ("/do/1/if", NodeKind::X, Some("a > 1"), None, 1),
                ("/do/1/do/0", NodeKind::Ret, None, None, 1),
                ("/do/1/do/0/ret", NodeKind::Ref, Some("a"), None, 2),
                ("/do/1/el/0", NodeKind::Set, Some("a"), None, 1),
                ("/do/1/el/0/set/a", NodeKind::Do, None, None, 2),
                ("/do/2", NodeKind::For, None, None, 0),
                ("/do/2/for", NodeKind::Val, None, None, 1),
                ("/do/2/do/0", NodeKind::Ret, None, None, 1),
                ("/do/2/do/0/ret", NodeKind::Call, Some("twice"), None, 2),
                (
                    "/do/2/do/0/ret/call/twice/v",
                    NodeKind::Ref,
                    Some("a"),
                    None,
                    3
                ),
                // The function's body isn't a block, so it's a step itself.
                ("/fn/twice", NodeKind::X, Some("v * 2"), None, 0),
            ]
        );
    }

    #[test]
    fn obj_and_list_are_steps_and_so_is_each_part_in_order() {
        let program = compile(&m(vec![ret(value!({ "obj": {
            "a": 1,
            "b": { "list": [{ "ref": "x" }, 2] }
        } }))]));
        let listed: Vec<_> = program
            .steps()
            .iter()
            .map(|s| (s.path, s.kind, s.depth))
            .collect();
        assert_eq!(
            listed,
            [
                ("/do/0", NodeKind::Ret, 0),
                ("/do/0/ret", NodeKind::Obj, 1),
                ("/do/0/ret/obj/a", NodeKind::Val, 2),
                ("/do/0/ret/obj/b", NodeKind::List, 2),
                ("/do/0/ret/obj/b/list/0", NodeKind::Ref, 3),
                ("/do/0/ret/obj/b/list/1", NodeKind::Val, 3),
            ]
        );
        assert_eq!(NodeKind::Obj.as_str(), "obj");
        assert_eq!(NodeKind::List.to_string(), "list");

        // Stepping into goes through each in that order; stepping over the
        // list skips its items.
        let into = [StepInto; 6];
        let (log, outcome) = debug(&program, value!({ "x": 5 }), &["/do/0"], &into);
        let listed_paths: Vec<_> = listed.iter().map(|(path, ..)| *path).collect();
        assert_eq!(paths(&log), listed_paths);
        assert_eq!(outcome.unwrap().value, value!({ "a": 1, "b": [5, 2] }));
        let over = [StepOver, StepOver];
        let (log, _) = debug(&program, value!({ "x": 5 }), &["/do/0/ret/obj/a"], &over);
        assert_eq!(paths(&log), ["/do/0/ret/obj/a", "/do/0/ret/obj/b"]);
    }

    #[test]
    fn a_program_of_one_value_is_one_step() {
        let program = compile(&value!(1));
        let steps = program.steps();
        assert_eq!(steps.len(), 1);
        assert_eq!(
            (steps[0].path, steps[0].kind, steps[0].depth),
            ("", NodeKind::Val, 0)
        );
        assert!(compile(&m(vec![])).steps().is_empty());
    }

    #[test]
    fn listing_a_deep_program_doesnt_recurse() {
        // Compiling recurses (within `max_depth`), and so does dropping, so
        // they have a big stack; listing borrows the program on a stack far
        // too small to recurse 3,000 levels.
        let big = std::thread::Builder::new().stack_size(256 * 1024 * 1024);
        big.spawn(|| {
            let engine = Engine::builder().max_depth(5_000).build();
            let mut program = value!(1);
            for _ in 0..1_500 {
                program = m(vec![ret(program)]);
            }
            let compiled = engine.compile(&program).unwrap();
            let (count, deepest) = std::thread::scope(|scope| {
                let small = std::thread::Builder::new().stack_size(64 * 1024);
                let listing = small.spawn_scoped(scope, || {
                    let steps = compiled.steps();
                    (steps.len(), steps.iter().map(|s| s.depth).max())
                });
                listing.unwrap().join().unwrap()
            });
            // 1,500 `ret`s, the 1,499 blocks between them, and the 1.
            assert_eq!(count, 3_000);
            assert_eq!(deepest, Some(2_999));
        })
        .unwrap()
        .join()
        .unwrap();
    }
}

mod run_configs {
    use super::*;
    use codyscript_core::RunConfig;

    #[test]
    fn check_finds_every_breakpoint_that_names_no_step() {
        let program = with_fee();
        let mut config = RunConfig {
            variables: vars(value!({ "a": 100 })),
            breakpoints: vec![Breakpoint::path("/fn/fee/do/1"), Breakpoint::path("/do/9")],
        };
        let error = program.check(&config).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidConfig);
        assert_eq!(error.issues().unwrap().len(), 1);
        assert_eq!(error.path(), "/breakpoints/1");
        config.breakpoints.pop();
        assert!(program.check(&config).is_ok());
        // A configuration with no breakpoints fits any program.
        assert!(compile(&m(vec![])).check(&RunConfig::default()).is_ok());
    }
}
