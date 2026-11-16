mod common;

use std::sync::{Arc, Mutex};

use codyscript_core::{value, Value};
use codyscript_core::{
    Engine, Error, ErrorCode, Executor, ExecutorCall, Limits, RunOptions, Signal,
};
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
        let program = m(vec![
            set("i", value!(0)),
            lp(
                op("math", "lt", value!({ "a": var("i"), "b": 3 })),
                vec![set(
                    "i",
                    op("math", "add", value!({ "a": var("i"), "b": 1 })),
                )],
            ),
            when(value!(false), vec![ret(value!("no"))], None),
            ret(var("i")),
        ]);
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

mod operations {
    use super::*;

    /// An executor that records its calls and returns `result`.
    fn recorder(result: Value) -> (Arc<Mutex<Vec<ExecutorCall>>>, Engine) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&calls);
        let engine = Engine::builder()
            .executor("exe", move |call| {
                log.lock().unwrap().push(call);
                Ok(result.clone())
            })
            .build();
        (calls, engine)
    }

    #[test]
    fn call_the_executor_with_op_arg_and_resolved_input() {
        let (calls, engine) = recorder(value!("ok"));
        let program = op_with(
            "exe",
            "doIt",
            value!({ "a": val(value!(1)), "b": var("x") }),
            Some(value!({ "cfg": true })),
        );
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
        run(&engine, &call("exe", "x")).unwrap();
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
        run(&engine, &op_with("exe", "x", value!({}), Some(Value::Null))).unwrap();
        assert_eq!(calls.lock().unwrap()[0].arg, Some(Value::Null));
    }

    #[test]
    fn null_from_an_executor_is_null() {
        let engine = Engine::builder()
            .executor("exe", |_| Ok(Value::Null))
            .build();
        assert_eq!(run(&engine, &call("exe", "x")).unwrap().value, Value::Null);
    }

    #[test]
    fn async_executors_are_awaited() {
        let engine = Engine::builder()
            .async_executor("exe", |_| async {
                sleep(5).await;
                Ok(value!(5))
            })
            .build();
        assert_eq!(run(&engine, &call("exe", "x")).unwrap().value, value!(5));
    }

    #[test]
    fn arguments_resolve_one_at_a_time_in_order() {
        let log = Arc::new(Mutex::new(Vec::<String>::new()));
        let slow_log = Arc::clone(&log);
        let engine = Engine::builder()
            .async_executor("slow", move |call| {
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
            .executor("pair", |call| Ok(Value::Object(call.input)))
            .build();
        let program = op(
            "pair",
            "x",
            value!({
                "first": op_with("slow", "x", value!({}), Some(value!("a"))),
                "second": op_with("slow", "x", value!({}), Some(value!("b"))),
            }),
        );
        let result = run(&engine, &program).unwrap();
        assert_eq!(*log.lock().unwrap(), ["a", "b"]);
        assert_eq!(result.value, value!({ "first": "a", "second": "b" }));
    }

    #[test]
    fn arguments_keep_their_written_order() {
        let engine = Engine::builder()
            .executor("pair", |call| Ok(Value::Object(call.input)))
            .build();
        let program = value!({ "op": { "pair.x": { "z": 1, "a": 2, "m": 3 } } });
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
            .executor("bad", |_| Err(Box::new(Boom)))
            .build();
        let program = m(vec![set("x", tagged(call("bad", "go"), "n1"))]);
        let error = failure(run(&engine, &program));
        assert_eq!(error.code(), ErrorCode::ExecutorFailed);
        assert_eq!(error.path(), "/do/0/set/x");
        assert_eq!(error.tag(), Some("n1"));
        assert!(error.cause().unwrap().downcast_ref::<Boom>().is_some());
        assert!(std::error::Error::source(&error).is_some());
        assert_eq!(
            error.message(),
            r#"Executor "bad" failed on "go": boom at /do/0/set/x (tag n1)"#
        );
    }

    #[test]
    fn a_string_error_is_reported_too() {
        let engine = Engine::builder()
            .async_executor("bad", |_| async { Err("nope".into()) })
            .build();
        let error = failure(run(&engine, &call("bad", "go")));
        assert!(error.message().contains(": nope"), "{}", error.message());
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
            run(&engine, &program(value!(0))).unwrap().value,
            value!("no")
        );
        assert_eq!(
            run(&engine, &program(value!([]))).unwrap().value,
            value!("yes")
        );
        let program = m(vec![when(
            val(value!(false)),
            vec![ret(val(value!(1)))],
            None,
        )]);
        assert_eq!(run(&engine, &program).unwrap().value, Value::Null);
    }

    #[test]
    fn truthiness_is_javascripts() {
        let engine = engine();
        let falsy = [
            value!(false),
            value!(0),
            value!(0.0),
            value!(-0.0),
            value!(""),
            value!(null),
        ];
        let truthy = [
            value!(true),
            value!(1),
            value!(-0.5),
            value!("0"),
            value!([]),
            value!({}),
        ];
        for (values, expected) in [(&falsy, "no"), (&truthy, "yes")] {
            for v in values.iter() {
                let program = m(vec![when(
                    val(v.clone()),
                    vec![ret(value!("yes"))],
                    Some(vec![ret(value!("no"))]),
                )]);
                assert_eq!(
                    run(&engine, &program).unwrap().value,
                    value!(expected),
                    "{v:?}"
                );
            }
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
        let program = m(vec![
            set("i", val(value!(0))),
            lp(
                op("math", "lt", value!({ "a": var("i"), "b": val(value!(5)) })),
                vec![set(
                    "i",
                    op(
                        "math",
                        "add",
                        value!({ "a": var("i"), "b": val(value!(1)) }),
                    ),
                )],
            ),
            ret(var("i")),
        ]);
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

mod compile {
    use super::*;

    #[test]
    fn a_compiled_program_runs_many_times_with_fresh_state() {
        let source = m(vec![
            set(
                "x",
                op(
                    "math",
                    "add",
                    value!({ "a": var("n"), "b": val(value!(1)) }),
                ),
            ),
            ret(var("x")),
        ]);
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
            .executor("other", |_| Ok(value!(1)))
            .build();
        let error = compile_error(&engine(), &call("other", "x"));
        assert_eq!(error.code(), ErrorCode::UnknownExecutor);
    }

    #[test]
    fn executors_may_be_given_as_a_collection() {
        let one: Arc<dyn Executor> =
            Arc::new(codyscript_core::executor::from_fn(|_| Ok(value!(1))));
        let engine = Engine::builder().executors([("one", one)]).build();
        assert!(engine.has_executor("one"));
        assert_eq!(run(&engine, &call("one", "x")).unwrap().value, value!(1));
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
            .custom_executor("count", Counter(Mutex::new(0)))
            .build();
        let program = m(vec![set("a", call("count", "x")), ret(call("count", "x"))]);
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
            .executor("stop", move |call| {
                let signal = call.signal.expect("the run's signal");
                signal.abort_with("enough");
                log.lock().unwrap().push(signal);
                Ok(value!(true))
            })
            .build();
        let program = m(vec![lp(call("stop", "x"), vec![])]);
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
            .executor("x", |_| {
                Err(Box::new(Error::new(ErrorCode::Aborted, "stop")))
            })
            .build();
        let error = failure(run(&engine, &call("x", "y")));
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
                calls = op("math", "add", value!({ "a": calls, "b": 1 }));
            }
            assert_eq!(run(&engine(), &calls).unwrap().value, value!(43));
        });
    }
}
