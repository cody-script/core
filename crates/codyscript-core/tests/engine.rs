mod common;

use std::sync::{Arc, Mutex};

use codyscript_core::{
    Engine, Error, ErrorCode, Executor, ExecutorCall, Limits, RunOptions, Signal,
};
use codyscript_core::{Value, value};
use common::*;

mod inputs {
    use super::*;

    #[test]
    fn a_constant_is_its_value() {
        let engine = engine();
        for v in [
            value!(0),
            value!("s"),
            value!(true),
            value!(null),
            value!([1, [2]]),
            value!({ "a": { "b": 1 } }),
        ] {
            assert_eq!(run(&engine, &val(v.clone())).unwrap().value, v);
        }
    }

    #[test]
    fn a_reference_reads_a_variable() {
        let engine = engine();
        let result = engine.run_blocking(&var("x"), with_vars(value!({ "x": 7 })));
        assert_eq!(result.unwrap().value, value!(7));
        let result = engine.run_blocking(&var("x"), with_vars(value!({ "x": null })));
        assert_eq!(result.unwrap().value, Value::Null);
    }

    #[test]
    fn an_undefined_reference_fails_rather_than_yielding_null() {
        let error = failure(run(&engine(), &value!({ "ref": "nope", "tag": "t1" })));
        assert_eq!(error.code(), ErrorCode::UndefinedVariable);
        assert_eq!(error.tag(), Some("t1"));
        assert_eq!(
            error.message(),
            r#"Variable "nope" is not defined (tag t1)"#
        );
    }

    #[test]
    fn a_bare_number_string_boolean_or_null_is_its_own_value() {
        let engine = engine();
        for v in [
            value!(0),
            value!(2.5),
            value!(""),
            value!("fee"),
            value!(true),
            value!(false),
            value!(null),
        ] {
            assert_eq!(run(&engine, &v).unwrap().value, v);
        }
    }

    #[test]
    fn a_bare_string_is_text_never_a_variable() {
        let program = m(vec![set("fee", value!(1)), ret(value!("fee"))]);
        assert_eq!(run(&engine(), &program).unwrap().value, value!("fee"));
    }

    #[test]
    fn literals_work_anywhere_an_input_does() {
        let program = uses_math(m(vec![
            set("i", value!(0)),
            lp(
                call("math.lt", value!({ "a": var("i"), "b": 3 })),
                vec![set(
                    "i",
                    call("math.add", value!({ "a": var("i"), "b": 1 })),
                )],
            ),
            when(value!(false), vec![ret(value!("no"))], None),
            ret(var("i")),
        ]));
        assert_eq!(run(&engine(), &program).unwrap().value, value!(3));
    }

    #[test]
    fn a_literal_is_a_step() {
        let program = m(vec![set("a", value!(1))]);
        assert_eq!(run(&engine(), &program).unwrap().steps, 2);
    }

    #[test]
    fn a_block_with_no_return_is_null() {
        assert_eq!(run(&engine(), &m(vec![])).unwrap().value, Value::Null);
    }
}

mod external_functions {
    use super::*;

    /// An executor providing `exe.doIt` and `exe.x` that records its calls
    /// and returns `result`.
    fn recorder(result: Value) -> (Arc<Mutex<Vec<ExecutorCall>>>, Engine) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&calls);
        let engine = Engine::builder()
            .executor("exe", ["doIt", "x"], move |call| {
                log.lock().unwrap().push(call);
                Ok(result.clone())
            })
            .build();
        (calls, engine)
    }

    /// `program`, declaring `exe.doIt` and `exe.x`.
    fn uses_exe(program: Value) -> Value {
        with_ext(program, value!({ "exe": ["doIt", "x"] }))
    }

    #[test]
    fn call_the_executor_with_the_functions_name_arg_and_resolved_input() {
        let (calls, engine) = recorder(value!("ok"));
        let program = uses_exe(call_with(
            "exe.doIt",
            value!({ "a": val(value!(1)), "b": var("x") }),
            value!({ "cfg": true }),
        ));
        let result = engine.run_blocking(&program, with_vars(value!({ "x": "y" })));
        assert_eq!(result.unwrap().value, value!("ok"));
        assert_eq!(
            *calls.lock().unwrap(),
            vec![ExecutorCall {
                op: "doIt".into(),
                arg: Some(value!({ "cfg": true })),
                input: vars(value!({ "a": 1, "b": "y" })),
                signal: None,
            }]
        );
    }

    #[test]
    fn pass_empty_input_and_no_arg_when_the_program_has_none() {
        let (calls, engine) = recorder(value!(1));
        run(&engine, &uses_exe(call("exe.x", value!({})))).unwrap();
        assert_eq!(
            *calls.lock().unwrap(),
            vec![ExecutorCall {
                op: "x".into(),
                arg: None,
                input: Default::default(),
                signal: None,
            }]
        );
    }

    #[test]
    fn an_explicit_null_arg_is_passed_on() {
        let (calls, engine) = recorder(value!(1));
        run(
            &engine,
            &uses_exe(call_with("exe.x", value!({}), Value::Null)),
        )
        .unwrap();
        assert_eq!(calls.lock().unwrap()[0].arg, Some(Value::Null));
    }

    #[test]
    fn a_function_name_with_dots_splits_at_the_first() {
        let engine = Engine::builder()
            .executor("num", ["round.up"], |call| Ok(call.op.into()))
            .build();
        let program = with_ext(
            call("num.round.up", value!({})),
            value!({ "num": ["round.up"] }),
        );
        assert_eq!(run(&engine, &program).unwrap().value, value!("round.up"));
        assert!(engine.has_function("num.round.up"));
        assert!(!engine.has_function("num.round"));
    }

    #[test]
    fn null_from_an_executor_is_null() {
        let engine = Engine::builder()
            .executor("exe", ["x"], |_| Ok(Value::Null))
            .build();
        let program = with_ext(call("exe.x", value!({})), value!({ "exe": ["x"] }));
        assert_eq!(run(&engine, &program).unwrap().value, Value::Null);
    }

    #[test]
    fn async_executors_are_awaited() {
        let engine = Engine::builder()
            .async_executor("exe", ["x"], |_| async {
                sleep(5).await;
                Ok(value!(5))
            })
            .build();
        let program = with_ext(call("exe.x", value!({})), value!({ "exe": ["x"] }));
        assert_eq!(run(&engine, &program).unwrap().value, value!(5));
    }

    #[test]
    fn arguments_resolve_one_at_a_time_in_order() {
        let log = Arc::new(Mutex::new(Vec::<String>::new()));
        let slow_log = Arc::clone(&log);
        let engine = Engine::builder()
            .async_executor("slow", ["x"], move |call| {
                let log = Arc::clone(&slow_log);
                async move {
                    let arg = call.arg.unwrap_or_default();
                    sleep(if arg == "a" { 10 } else { 0 }).await;
                    log.lock()
                        .unwrap()
                        .push(arg.as_str().unwrap_or_default().to_owned());
                    Ok(arg)
                }
            })
            .executor("pair", ["x"], |call| Ok(Value::Object(call.input)))
            .build();
        let program = with_ext(
            call(
                "pair.x",
                value!({
                    "first": call_with("slow.x", value!({}), value!("a")),
                    "second": call_with("slow.x", value!({}), value!("b")),
                }),
            ),
            value!({ "pair": ["x"], "slow": ["x"] }),
        );
        let result = run(&engine, &program).unwrap();
        assert_eq!(*log.lock().unwrap(), ["a", "b"]);
        assert_eq!(result.value, value!({ "first": "a", "second": "b" }));
    }

    #[test]
    fn arguments_keep_their_written_order() {
        let engine = Engine::builder()
            .executor("pair", ["x"], |call| Ok(Value::Object(call.input)))
            .build();
        let program = value!({
            "ext": { "pair": ["x"] },
            "call": { "pair.x": { "z": 1, "a": 2, "m": 3 } }
        });
        let value = run(&engine, &program).unwrap().value;
        assert_eq!(keys(&value), ["z", "a", "m"]);
    }

    #[derive(Debug)]
    struct Boom;
    impl std::fmt::Display for Boom {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("boom")
        }
    }
    impl std::error::Error for Boom {}

    #[test]
    fn an_executors_error_fails_the_run_with_its_path_and_cause() {
        let engine = Engine::builder()
            .executor("bad", ["go"], |_| Err(Box::new(Boom)))
            .build();
        let program = with_ext(
            m(vec![set("x", tagged(call("bad.go", value!({})), "n1"))]),
            value!({ "bad": ["go"] }),
        );
        let error = failure(run(&engine, &program));
        assert_eq!(error.code(), ErrorCode::FunctionFailed);
        assert_eq!(error.code().as_str(), "function_failed");
        assert_eq!(error.path(), "/do/0/set/x");
        assert_eq!(error.tag(), Some("n1"));
        assert!(error.cause().unwrap().downcast_ref::<Boom>().is_some());
        assert!(std::error::Error::source(&error).is_some());
        assert_eq!(
            error.message(),
            r#"Function "bad.go" failed: boom at /do/0/set/x (tag n1)"#
        );
    }

    #[test]
    fn a_string_error_is_reported_too() {
        let engine = Engine::builder()
            .async_executor("bad", ["go"], |_| async { Err("nope".into()) })
            .build();
        let program = with_ext(call("bad.go", value!({})), value!({ "bad": ["go"] }));
        let error = failure(run(&engine, &program));
        assert_eq!(error.message(), r#"Function "bad.go" failed: nope"#);
    }

    #[test]
    fn registering_an_id_again_replaces_the_executor_and_its_functions() {
        let engine = Engine::builder()
            .executor("exe", ["old"], |_| Ok(value!("old")))
            .executor("exe", ["new"], |_| Ok(value!("new")))
            .build();
        assert!(!engine.has_function("exe.old"));
        let program = with_ext(call("exe.new", value!({})), value!({ "exe": ["new"] }));
        assert_eq!(run(&engine, &program).unwrap().value, value!("new"));
    }
}

mod instructions {
    use super::*;

    #[test]
    fn set_assigns_and_the_result_reports_top_level_variables() {
        let program = m(vec![set("a", val(value!(1))), set("b", var("a"))]);
        let result = engine()
            .run_blocking(&program, with_vars(value!({ "c": 3 })))
            .unwrap();
        assert_eq!(result.variables, vars(value!({ "a": 1, "b": 1, "c": 3 })));
        // Initial variables first, then in the order they were first set.
        assert_eq!(keys(&Value::Object(result.variables)), ["c", "a", "b"]);
        assert_eq!(result.value, Value::Null);
    }

    #[test]
    fn ret_stops_the_block() {
        let program = m(vec![ret(val(value!(1))), set("never", val(value!(2)))]);
        let result = run(&engine(), &program).unwrap();
        assert_eq!(result.value, value!(1));
        assert!(result.variables.is_empty());
    }

    #[test]
    fn if_runs_do_or_el() {
        let engine = engine();
        let program = |c: Value| {
            m(vec![when(
                val(c),
                vec![ret(val(value!("yes")))],
                Some(vec![ret(val(value!("no")))]),
            )])
        };
        assert_eq!(
            run(&engine, &program(value!(true))).unwrap().value,
            value!("yes")
        );
        assert_eq!(
            run(&engine, &program(value!(false))).unwrap().value,
            value!("no")
        );
        let program = m(vec![when(
            val(value!(false)),
            vec![ret(val(value!(1)))],
            None,
        )]);
        assert_eq!(run(&engine, &program).unwrap().value, Value::Null);
    }

    #[test]
    fn if_and_for_need_a_boolean() {
        let engine = engine();
        for v in [
            value!(0),
            value!(1),
            value!(""),
            value!("true"),
            value!(null),
            value!([]),
            value!({}),
        ] {
            let kind = match &v {
                Value::Number(_) => "number",
                Value::String(_) => "string",
                Value::Null => "null",
                Value::Array(_) => "list",
                _ => "object",
            };
            let test = tagged(val(v.clone()), "t");
            let branch = m(vec![when(test.clone(), vec![], None)]);
            let error = failure(run(&engine, &branch));
            assert_eq!(error.code(), ErrorCode::TypeError, "{v:?}");
            assert_eq!(error.path(), "/do/0/if", "{v:?}");
            assert_eq!(error.tag(), Some("t"));
            assert_eq!(
                error.message(),
                format!(r#""if" needs a boolean, got {kind} at /do/0/if (tag t)"#),
                "{v:?}"
            );
            let looped = m(vec![lp(test, vec![])]);
            let error = failure(run(&engine, &looped));
            assert_eq!(error.path(), "/do/0/for", "{v:?}");
            assert_eq!(
                error.message(),
                format!(r#""for" needs a boolean, got {kind} at /do/0/for (tag t)"#),
                "{v:?}"
            );
        }
    }

    #[test]
    fn ret_inside_if_leaves_the_block_skipping_the_rest() {
        let program = m(vec![
            when(
                val(value!(true)),
                vec![ret(val(value!(1))), set("x", val(value!(2)))],
                None,
            ),
            set("y", val(value!(3))),
        ]);
        let result = run(&engine(), &program).unwrap();
        assert_eq!(result.value, value!(1));
        assert!(result.variables.is_empty());
    }

    #[test]
    fn for_loops_while_its_condition_holds() {
        let program = uses_math(m(vec![
            set("i", val(value!(0))),
            lp(
                call("math.lt", value!({ "a": var("i"), "b": val(value!(5)) })),
                vec![set(
                    "i",
                    call("math.add", value!({ "a": var("i"), "b": val(value!(1)) })),
                )],
            ),
            ret(var("i")),
        ]));
        assert_eq!(run(&engine(), &program).unwrap().value, value!(5));
    }

    #[test]
    fn ret_leaves_an_endless_loop() {
        let program = m(vec![lp(val(value!(true)), vec![ret(val(value!(45)))])]);
        assert_eq!(run(&engine(), &program).unwrap().value, value!(45));
    }
}

mod blocks {
    use super::*;

    #[test]
    fn a_nested_block_returns_to_its_caller_which_carries_on() {
        let program = m(vec![
            set("x", m(vec![ret(val(value!(1)))])),
            set("y", val(value!(2))),
            ret(val(value!("end"))),
        ]);
        let result = run(&engine(), &program).unwrap();
        assert_eq!(result.value, value!("end"));
        assert_eq!(result.variables, vars(value!({ "x": 1, "y": 2 })));
    }

    #[test]
    fn a_nested_block_reads_outer_variables_but_assigns_its_own() {
        let program = m(vec![
            set("a", val(value!(1))),
            set("b", m(vec![set("a", val(value!(99))), ret(var("a"))])),
            ret(var("a")),
        ]);
        let result = run(&engine(), &program).unwrap();
        assert_eq!(result.value, value!(1));
        assert_eq!(result.variables, vars(value!({ "a": 1, "b": 99 })));
    }

    #[test]
    fn a_nested_blocks_variables_dont_leak_out() {
        let program = m(vec![
            set("_", m(vec![set("inner", val(value!(1)))])),
            ret(var("inner")),
        ]);
        let error = failure(run(&engine(), &program));
        assert_eq!(error.code(), ErrorCode::UndefinedVariable);
        assert_eq!(error.path(), "/do/1/ret");
    }

    #[test]
    fn a_nested_block_is_a_step_but_the_top_level_one_isnt() {
        assert_eq!(run(&engine(), &m(vec![])).unwrap().steps, 0);
        assert_eq!(run(&engine(), &m(vec![ret(m(vec![]))])).unwrap().steps, 2);
    }
}

mod functions {
    use super::*;

    /// A program whose body calls `fee`, which caps a 3% fee.
    fn fee_program() -> Value {
        with_fn(
            m(vec![
                set("fee", call("fee", value!({ "amount": var("Amount") }))),
                ret(var("fee")),
            ]),
            [(
                "fee",
                m(vec![
                    set("fee", x("amount * 0.03")),
                    when(x("fee > 300"), vec![ret(value!(300))], None),
                    ret(var("fee")),
                ]),
            )],
        )
    }

    #[test]
    fn a_programs_body_runs_and_calls_its_functions() {
        let engine = engine();
        let compiled = engine.compile(&fee_program()).unwrap();
        let result = compiled
            .run_blocking(with_vars(value!({ "Amount": 1000 })))
            .unwrap();
        assert_eq!(result.value, value!(30));
        // The body's variables are the result's; the function's `fee` isn't.
        assert_eq!(
            result.variables,
            vars(value!({ "Amount": 1000, "fee": 30 }))
        );
        assert_eq!(
            compiled_value(&compiled, with_vars(value!({ "Amount": 20000 }))),
            value!(300)
        );
    }

    #[test]
    fn the_roots_fields_may_come_in_any_order() {
        let before = value!({ "fn": { "two": 2 }, "call": { "two": {} } });
        let after = value!({ "call": { "two": {} }, "fn": { "two": 2 } });
        for program in [before, after] {
            assert_eq!(run(&engine(), &program).unwrap().value, value!(2));
        }
    }

    #[test]
    fn defining_and_declaring_functions_changes_nothing_until_they_are_called() {
        let engine = engine();
        let programs = [
            x("n * 2"),
            val(value!({ "a": [1] })),
            m(vec![]),
            m(vec![set("a", value!(1)), set("b", x("a + n"))]),
            m(vec![
                set("i", value!(0)),
                lp(x("i < n"), vec![set("i", x("i + 1"))]),
                ret(m(vec![ret(var("i"))])),
            ]),
        ];
        for program in programs {
            let alone = engine
                .run_blocking(&program, with_vars(value!({ "n": 4 })))
                .unwrap();
            let beside = with_fn(uses_math(program.clone()), [("f", value!(1))]);
            let beside = engine
                .run_blocking(&beside, with_vars(value!({ "n": 4 })))
                .unwrap();
            assert_eq!(alone, beside, "{program:?}");
        }
    }

    #[test]
    fn arguments_are_a_functions_only_variables() {
        let engine = engine();
        let reads = |name: &str| {
            with_fn(
                m(vec![
                    set("local", value!(1)),
                    ret(call("f", value!({ "arg": 2 }))),
                ]),
                [("f", var(name))],
            )
        };
        let options = || with_vars(value!({ "host": 3 }));
        let result = engine.run_blocking(&reads("arg"), options()).unwrap();
        assert_eq!(result.value, value!(2));
        for hidden in ["local", "host"] {
            let error = failure(engine.run_blocking(&reads(hidden), options()));
            assert_eq!(error.code(), ErrorCode::UndefinedVariable, "{hidden}");
            assert_eq!(error.path(), "/fn/f");
        }
        // Expressions see the same variables as `ref`.
        let program = with_fn(
            m(vec![set("a", value!(1)), ret(call("f", value!({})))]),
            [("f", x("a"))],
        );
        let error = failure(run(&engine, &program));
        assert_eq!(error.code(), ErrorCode::UndefinedVariable);
        assert_eq!(
            error.message(),
            r#"Variable "a" is not defined in "a" at /fn/f/x"#
        );
    }

    #[test]
    fn a_functions_assignments_stay_in_it_and_the_callers_are_visible_again() {
        let program = with_fn(
            m(vec![
                set("a", value!(1)),
                set("r", call("f", value!({ "a": 5 }))),
                ret(x("a * 100 + r")),
            ]),
            [(
                "f",
                m(vec![
                    set("a", x("a + 1")),
                    set("b", value!(9)),
                    ret(var("a")),
                ]),
            )],
        );
        let result = run(&engine(), &program).unwrap();
        assert_eq!(result.value, value!(106));
        assert_eq!(result.variables, vars(value!({ "a": 1, "r": 6 })));
    }

    #[test]
    fn ret_returns_from_the_function_and_the_caller_carries_on() {
        let program = with_fn(
            m(vec![
                set("a", call("first", value!({}))),
                set("b", value!("after")),
                ret(value!("end")),
            ]),
            [(
                "first",
                m(vec![
                    lp(
                        value!(true),
                        vec![when(value!(true), vec![ret(value!(1))], None)],
                    ),
                    ret(value!("never")),
                ]),
            )],
        );
        let result = run(&engine(), &program).unwrap();
        assert_eq!(result.value, value!("end"));
        assert_eq!(result.variables, vars(value!({ "a": 1, "b": "after" })));
    }

    #[test]
    fn a_function_with_no_ret_returns_null() {
        let program = with_fn(call("f", value!({})), [("f", m(vec![set("a", value!(1))]))]);
        assert_eq!(run(&engine(), &program).unwrap().value, Value::Null);
    }

    #[test]
    fn arguments_resolve_in_order_before_the_call_and_may_be_calls() {
        let log = Arc::new(Mutex::new(Vec::<Value>::new()));
        let seen = Arc::clone(&log);
        let engine = Engine::builder()
            .executor("log", ["x"], move |call| {
                seen.lock()
                    .unwrap()
                    .push(call.arg.clone().unwrap_or_default());
                Ok(call.arg.unwrap_or_default())
            })
            .build();
        let logged = |v: Value| call_with("log.x", value!({}), v);
        let program = with_ext(
            with_fn(
                call(
                    "pair",
                    value!({
                        "z": logged(value!("z")),
                        "a": call("pair", value!({ "z": logged(value!("inner")), "a": 0 })),
                    }),
                ),
                [(
                    "pair",
                    m(vec![ret(logged(value!("body"))), ret(var("never"))]),
                )],
            ),
            value!({ "log": ["x"] }),
        );
        let value = run(&engine, &program).unwrap().value;
        assert_eq!(value, value!("body"));
        assert_eq!(
            *log.lock().unwrap(),
            [value!("z"), value!("inner"), value!("body"), value!("body")]
        );
        let program = with_fn(
            call("sub", value!({ "z": 10, "a": 3 })),
            [("sub", x("z - a"))],
        );
        assert_eq!(run(&engine, &program).unwrap().value, value!(7));
    }

    #[test]
    fn functions_and_external_functions_call_each_other() {
        // A namespaced CodyScript function beside the executor's functions,
        // each calling the other kind.
        let program = with_fn(
            uses_math(call("math.double", value!({ "a": 20 }))),
            [
                (
                    "math.double",
                    call(
                        "math.add",
                        value!({ "a": var("a"), "b": call("math.one", value!({})) }),
                    ),
                ),
                (
                    "math.one",
                    call("math.add", value!({ "a": var("a"), "b": 1 })),
                ),
            ],
        );
        // math.one sees no `a`: arguments are its only variables.
        let error = failure(run(&engine(), &program));
        assert_eq!(error.path(), "/fn/math.one/call/math.add/a");
        let program = with_fn(
            uses_math(call("math.double", value!({ "a": 20 }))),
            [
                (
                    "math.double",
                    call(
                        "math.add",
                        value!({ "a": var("a"), "b": call("math.one", value!({ "a": var("a") })) }),
                    ),
                ),
                (
                    "math.one",
                    call("math.add", value!({ "a": var("a"), "b": 1 })),
                ),
            ],
        );
        assert_eq!(run(&engine(), &program).unwrap().value, value!(41));
    }

    #[test]
    fn functions_may_call_each_other_and_themselves() {
        let program = with_fn(m(vec![ret(val(value!([])))]), [("unused", value!(0))]);
        assert_eq!(run(&engine(), &program).unwrap().value, value!([]));

        let factorial = with_fn(
            call("factorial", value!({ "n": var("n") })),
            [(
                "factorial",
                m(vec![
                    when(x("n <= 1"), vec![ret(value!(1))], None),
                    ret(x("n * r")),
                ]),
            )],
        );
        // `r` isn't set: a recursive call has to be made explicitly.
        let error = failure(engine().run_blocking(&factorial, with_vars(value!({ "n": 3 }))));
        assert_eq!(error.path(), "/fn/factorial/do/1/ret/x");

        let factorial = with_fn(
            call("factorial", value!({ "n": var("n") })),
            [(
                "factorial",
                m(vec![
                    when(x("n <= 1"), vec![ret(value!(1))], None),
                    set("r", call("factorial", value!({ "n": x("n - 1") }))),
                    ret(x("n * r")),
                ]),
            )],
        );
        let compiled = engine().compile(&factorial).unwrap();
        assert_eq!(
            compiled_value(&compiled, with_vars(value!({ "n": 1 }))),
            value!(1)
        );
        assert_eq!(
            compiled_value(&compiled, with_vars(value!({ "n": 10 }))),
            value!(3_628_800)
        );

        let parity = with_fn(
            call("even", value!({ "n": var("n") })),
            [
                (
                    "even",
                    m(vec![
                        when(x("n == 0"), vec![ret(value!(true))], None),
                        ret(call("odd", value!({ "n": x("n - 1") }))),
                    ]),
                ),
                (
                    "odd",
                    m(vec![
                        when(x("n == 0"), vec![ret(value!(false))], None),
                        ret(call("even", value!({ "n": x("n - 1") }))),
                    ]),
                ),
            ],
        );
        let compiled = engine().compile(&parity).unwrap();
        assert_eq!(
            compiled_value(&compiled, with_vars(value!({ "n": 7 }))),
            value!(false)
        );
        assert_eq!(
            compiled_value(&compiled, with_vars(value!({ "n": 10 }))),
            value!(true)
        );
    }

    fn endless() -> Value {
        with_fn(
            call("spin", value!({ "n": 0 })),
            [(
                "spin",
                tagged(call("spin", value!({ "n": x("n + 1") })), "again"),
            )],
        )
    }

    #[test]
    fn endless_recursion_stops_at_max_depth() {
        let error = failure(run(&engine(), &endless()));
        assert_eq!(error.code(), ErrorCode::DepthLimit);
        assert_eq!(error.code().as_str(), "depth_limit");
        assert_eq!(error.path(), "/fn/spin");
        assert_eq!(error.tag(), Some("again"));
        assert_eq!(
            error.message(),
            "Calls nest deeper than 128 levels at /fn/spin (tag again)"
        );
        // Recursion isn't unbounded by steps alone: with steps to spare,
        // depth still stops it.
        let engine = Engine::builder().max_steps(u64::MAX).build();
        assert_eq!(
            failure(run(&engine, &endless())).code(),
            ErrorCode::DepthLimit
        );
    }

    #[test]
    fn max_depth_calls_may_nest_and_no_more() {
        let countdown = with_fn(
            call("down", value!({ "n": var("n") })),
            [(
                "down",
                m(vec![
                    when(
                        x("n > 0"),
                        vec![ret(call("down", value!({ "n": x("n - 1") })))],
                        None,
                    ),
                    ret(value!("bottom")),
                ]),
            )],
        );
        let engine = Engine::builder().max_depth(16).build();
        let compiled = engine.compile(&countdown).unwrap();
        // n = 15: sixteen calls in progress at once, the body's to down(15)
        // through down(1)'s to down(0).
        assert_eq!(
            compiled_value(&compiled, with_vars(value!({ "n": 15 }))),
            value!("bottom")
        );
        let error = failure(compiled.run_blocking(with_vars(value!({ "n": 16 }))));
        assert_eq!(error.code(), ErrorCode::DepthLimit);
        assert_eq!(error.path(), "/fn/down/do/0/do/0/ret");
        // The depth is of calls in progress, not of calls made: a thousand
        // calls one after another are fine.
        let program = with_fn(
            m(vec![
                set("i", value!(0)),
                lp(
                    x("i < 1000"),
                    vec![set("i", call("next", value!({ "i": var("i") })))],
                ),
                ret(var("i")),
            ]),
            [("next", x("i + 1"))],
        );
        assert_eq!(run(&engine, &program).unwrap().value, value!(1000));
    }

    #[test]
    fn calls_of_external_functions_dont_count_towards_max_depth() {
        let engine = Engine::builder()
            .executor("id", ["x"], |call| Ok(call.input["v"].clone()))
            .max_depth(12)
            .build();
        let mut nested = value!(1);
        for _ in 0..3 {
            nested = call("id.x", obj([("v", nested)]));
        }
        let program = with_ext(nested, value!({ "id": ["x"] }));
        assert_eq!(run(&engine, &program).unwrap().value, value!(1));
    }

    #[test]
    fn a_call_is_a_step_and_a_functions_top_level_block_isnt() {
        // call (1) + its argument (1) + ret (1) + the ref (1)
        let program = with_fn(
            call("f", value!({ "a": 1 })),
            [("f", m(vec![ret(var("a"))]))],
        );
        assert_eq!(run(&engine(), &program).unwrap().steps, 4);
        // A nested block is a step, as it is anywhere.
        let program = with_fn(call("f", value!({})), [("f", m(vec![ret(m(vec![]))]))]);
        assert_eq!(run(&engine(), &program).unwrap().steps, 3);
        // An external call is one step, plus its arguments.
        let program = uses_math(call("math.add", value!({ "a": 1, "b": 2 })));
        assert_eq!(run(&engine(), &program).unwrap().steps, 3);
        // Steps are counted across calls: recursion hits the step limit.
        let engine = Engine::builder().max_steps(20).build();
        let error = failure(run(&engine, &endless()));
        assert_eq!(error.code(), ErrorCode::StepLimit);
    }

    #[test]
    fn errors_inside_a_function_point_into_it() {
        let engine = Engine::builder()
            .executor("bad", ["go"], |_| Err("boom".into()))
            .build();
        let program = with_ext(
            with_fn(
                m(vec![set("x", call("f", value!({}))), ret(value!(1))]),
                [("f", m(vec![ret(tagged(call("bad.go", value!({})), "n2"))]))],
            ),
            value!({ "bad": ["go"] }),
        );
        let error = failure(run(&engine, &program));
        assert_eq!(error.code(), ErrorCode::FunctionFailed);
        assert_eq!(error.path(), "/fn/f/do/0/ret");
        assert_eq!(
            error.message(),
            r#"Function "bad.go" failed: boom at /fn/f/do/0/ret (tag n2)"#
        );
        let program = with_fn(call("f", value!({ "a": 1 })), [("f", x("a + 'b'"))]);
        let error = failure(run(&engine, &program));
        assert_eq!(error.code(), ErrorCode::TypeError);
        assert_eq!(error.path(), "/fn/f/x");
    }

    #[test]
    fn a_signal_stops_a_run_inside_a_function() {
        let signal = Signal::new();
        let stopper = signal.clone();
        let engine = Engine::builder()
            .executor("stop", ["x"], move |_| {
                stopper.abort();
                Ok(value!(1))
            })
            .build();
        let program = with_ext(
            with_fn(
                call("f", value!({})),
                [(
                    "f",
                    m(vec![set("a", call("stop.x", value!({}))), ret(var("a"))]),
                )],
            ),
            value!({ "stop": ["x"] }),
        );
        let error = failure(engine.run_blocking(&program, RunOptions::new().with_signal(signal)));
        assert_eq!(error.code(), ErrorCode::Aborted);
        assert_eq!(error.path(), "/fn/f/do/1");
    }

    #[test]
    fn a_compiled_program_with_functions_runs_again_with_fresh_state() {
        let compiled = engine().compile(&fee_program()).unwrap();
        let failed = compiled.run_blocking(RunOptions::new());
        assert_eq!(failure(failed).code(), ErrorCode::UndefinedVariable);
        for _ in 0..3 {
            let result = compiled
                .run_blocking(with_vars(value!({ "Amount": 100 })))
                .unwrap();
            assert_eq!(result.value, value!(3));
            assert_eq!(result.variables, vars(value!({ "Amount": 100, "fee": 3 })));
        }
    }
}

mod compile {
    use super::*;

    #[test]
    fn a_compiled_program_runs_many_times_with_fresh_state() {
        let source = uses_math(m(vec![
            set(
                "x",
                call("math.add", value!({ "a": var("n"), "b": val(value!(1)) })),
            ),
            ret(var("x")),
        ]));
        let compiled = engine().compile(&source).unwrap();
        assert_eq!(
            compiled_value(&compiled, with_vars(value!({ "n": 1 }))),
            value!(2)
        );
        assert_eq!(
            compiled_value(&compiled, with_vars(value!({ "n": 10 }))),
            value!(11)
        );
        assert_eq!(compiled.program(), &source);
    }

    #[test]
    fn compile_owned_keeps_the_program_it_is_given() {
        let source = m(vec![ret(x("n + 1"))]);
        let engine = engine();
        let compiled = engine.compile_owned(source.clone()).unwrap();
        assert_eq!(compiled.program(), &source);
        assert_eq!(
            compiled_value(&compiled, with_vars(value!({ "n": 1 }))),
            value!(2)
        );
        let invalid = with_ext(call("nope.x", value!({})), value!({ "nope": ["x"] }));
        let owned = engine.compile_owned(invalid.clone()).unwrap_err();
        let borrowed = compile_error(&engine, &invalid);
        assert_eq!(owned.to_string(), borrowed.to_string());
        assert_eq!(owned.issues(), borrowed.issues());
    }

    #[test]
    fn input_variables_are_copied_not_mutated() {
        let variables = vars(value!({ "a": 1 }));
        let result = engine()
            .run_blocking(
                &m(vec![set("a", val(value!(2)))]),
                RunOptions::from(variables.clone()),
            )
            .unwrap();
        assert_eq!(variables, vars(value!({ "a": 1 })));
        assert_eq!(result.variables, vars(value!({ "a": 2 })));
    }

    #[test]
    fn engines_dont_share_executors() {
        let _other = Engine::builder()
            .executor("other", ["x"], |_| Ok(value!(1)))
            .build();
        let program = with_ext(call("other.x", value!({})), value!({ "other": ["x"] }));
        let error = compile_error(&engine(), &program);
        assert_eq!(error.code(), ErrorCode::UnknownFunction);
        assert_eq!(error.path(), "/ext/other/0");
    }

    #[test]
    fn a_shared_executor_may_be_registered_as_an_arc() {
        let one: Arc<dyn Executor> =
            Arc::new(codyscript_core::executor::from_fn(|_| Ok(value!(1))));
        let engine = Engine::builder()
            .custom_executor("one", ["x"], Arc::clone(&one))
            .custom_executor("uno", ["x"], one)
            .build();
        assert!(engine.has_function("one.x") && engine.has_function("uno.x"));
        let program = with_ext(call("uno.x", value!({})), value!({ "uno": ["x"] }));
        assert_eq!(run(&engine, &program).unwrap().value, value!(1));
    }

    #[test]
    fn a_custom_executor_may_borrow_its_own_state() {
        struct Counter(Mutex<i64>);
        impl Executor for Counter {
            fn call(
                &self,
                _: ExecutorCall,
            ) -> codyscript_core::BoxFuture<'_, codyscript_core::ExecutorResult> {
                Box::pin(async move {
                    let mut n = self.0.lock().unwrap();
                    *n += 1;
                    Ok(value!(*n))
                })
            }
        }
        let engine = Engine::builder()
            .custom_executor("count", ["next"], Counter(Mutex::new(0)))
            .build();
        let next = || call("count.next", value!({}));
        let program = with_ext(
            m(vec![set("a", next()), ret(next())]),
            value!({ "count": ["next"] }),
        );
        assert_eq!(run(&engine, &program).unwrap().value, value!(2));
    }

    #[test]
    fn an_engine_with_no_executors_runs_pure_programs() {
        assert_eq!(
            run(&Engine::new(), &val(value!(1))).unwrap().value,
            value!(1)
        );
        assert_eq!(Engine::default().limits(), Limits::default());
    }

    #[test]
    fn compiled_programs_run_concurrently() {
        let compiled = Arc::new(engine().compile(&x("n * 2")).unwrap());
        let handles: Vec<_> = (0..8)
            .map(|n| {
                let compiled = Arc::clone(&compiled);
                std::thread::spawn(move || compiled_value(&compiled, with_vars(value!({ "n": n }))))
            })
            .collect();
        for (n, handle) in handles.into_iter().enumerate() {
            assert_eq!(handle.join().unwrap(), value!(n * 2));
        }
    }

    #[test]
    fn engines_programs_and_runs_can_cross_threads() {
        fn send_sync<T: Send + Sync>(_: &T) {}
        fn send<T: Send>(_: &T) {}
        let engine = engine();
        let compiled = engine.compile(&val(value!(1))).unwrap();
        send_sync(&engine);
        send_sync(&compiled);
        send(&compiled.run(RunOptions::new()));
        let program = val(value!(1));
        send(&engine.run(&program, RunOptions::new()));
    }
}

mod limits {
    use super::*;

    #[test]
    fn an_endless_loop_hits_the_step_limit() {
        let engine = Engine::builder().max_steps(50).build();
        let program = m(vec![
            value!({ "for": val(value!(true)), "do": [], "tag": "spin" }),
        ]);
        let error = failure(run(&engine, &program));
        assert_eq!(error.code(), ErrorCode::StepLimit);
        assert_eq!(error.path(), "/do/0/for");
        assert_eq!(error.message(), "Program exceeded 50 steps at /do/0/for");
    }

    #[test]
    fn the_default_limit_stops_an_endless_loop_too() {
        let error = failure(run(&Engine::new(), &m(vec![lp(val(value!(true)), vec![])])));
        assert_eq!(error.code(), ErrorCode::StepLimit);
    }

    #[test]
    fn steps_are_reported() {
        // block body: set (1) + its val (1)
        assert_eq!(
            run(&engine(), &m(vec![set("a", val(value!(1)))]))
                .unwrap()
                .steps,
            2
        );
    }

    #[test]
    fn an_aborted_signal_stops_the_run_and_reaches_executors() {
        let signal = Signal::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        let engine = Engine::builder()
            .executor("stop", ["x"], move |call| {
                let signal = call.signal.expect("the run's signal");
                signal.abort_with("enough");
                log.lock().unwrap().push(signal);
                Ok(value!(true))
            })
            .build();
        let program = with_ext(
            m(vec![lp(call("stop.x", value!({})), vec![])]),
            value!({ "stop": ["x"] }),
        );
        let error =
            failure(engine.run_blocking(&program, RunOptions::new().with_signal(signal.clone())));
        assert_eq!(*seen.lock().unwrap(), vec![signal.clone()]);
        assert_eq!(error.code(), ErrorCode::Aborted);
        assert_eq!(error.cause().unwrap().to_string(), "enough");
        assert!(Arc::ptr_eq(
            error.cause().unwrap(),
            &signal.reason().unwrap()
        ));
        assert_eq!(error.message(), "Run aborted at /do/0/for");
    }

    #[test]
    fn a_signal_aborted_before_the_run_stops_it_at_the_first_step() {
        let signal = Signal::new();
        signal.abort();
        signal.abort_with("ignored: only the first abort counts");
        assert!(signal.is_aborted());
        assert!(signal.reason().is_none());
        let options = RunOptions::new().with_signal(signal);
        let error = failure(Engine::new().run_blocking(&value!(1), options));
        assert_eq!(error.code(), ErrorCode::Aborted);
        assert!(error.cause().is_none());
        assert_eq!(error.path(), "");
        assert_eq!(error.message(), "Run aborted");
    }

    #[test]
    fn an_executors_own_error_passes_through_unwrapped() {
        let engine = Engine::builder()
            .executor("x", ["y"], |_| {
                Err(Box::new(Error::new(ErrorCode::Aborted, "stop")))
            })
            .build();
        let program = with_ext(call("x.y", value!({})), value!({ "x": ["y"] }));
        let error = failure(run(&engine, &program));
        assert_eq!(error.code(), ErrorCode::Aborted);
        assert_eq!(error.message(), "stop");
        assert_eq!(error.path(), "");
        assert!(error.cause().is_none());
    }
}

mod stack {
    use super::*;

    /// Runs `f` on a thread with WebAssembly's default stack, 1 MiB. (The
    /// deepest default-limit programs need under 512 KiB in a debug build,
    /// 192 KiB in release: compiling recurses once per level; running
    /// doesn't recurse at all.)
    fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .stack_size(1024 * 1024)
            .spawn(f)
            .unwrap()
            .join()
            .unwrap()
    }

    #[test]
    fn the_deepest_programs_the_default_limit_allows_run_on_a_wasm_sized_stack() {
        on_small_stack(|| {
            let mut blocks = value!(1);
            for _ in 0..42 {
                blocks = m(vec![ret(blocks)]);
            }
            assert_eq!(run(&engine(), &blocks).unwrap().value, value!(1));

            let mut list = vec![ret(value!(1))];
            for _ in 0..62 {
                list = vec![when(value!(true), list, None)];
            }
            assert_eq!(run(&engine(), &m(list)).unwrap().value, value!(1));

            let mut calls = value!(1);
            for _ in 0..42 {
                calls = call("math.add", value!({ "a": calls, "b": 1 }));
            }
            let calls = uses_math(calls);
            assert_eq!(run(&engine(), &calls).unwrap().value, value!(43));

            // Calls nest on the machine's own stacks, not the thread's.
            let countdown = with_fn(
                call("down", value!({ "n": 127 })),
                [(
                    "down",
                    m(vec![
                        when(
                            x("n > 0"),
                            vec![ret(call("down", value!({ "n": x("n - 1") })))],
                            None,
                        ),
                        ret(var("n")),
                    ]),
                )],
            );
            assert_eq!(run(&engine(), &countdown).unwrap().value, value!(0));
        });
    }

    #[test]
    fn deep_recursion_doesnt_use_the_threads_stack() {
        on_small_stack(|| {
            let engine = Engine::builder()
                .max_depth(100_000)
                .max_steps(10_000_000)
                .build();
            // 100 000 calls in progress at once: the body's to sum(99 999)
            // through sum(1)'s to sum(0).
            let sum = with_fn(
                call("sum", value!({ "n": 99_999 })),
                [(
                    "sum",
                    m(vec![
                        when(x("n == 0"), vec![ret(value!(0))], None),
                        set("rest", call("sum", value!({ "n": x("n - 1") }))),
                        ret(x("n + rest")),
                    ]),
                )],
            );
            assert_eq!(run(&engine, &sum).unwrap().value, value!(4_999_950_000_u64));
        });
    }
}
