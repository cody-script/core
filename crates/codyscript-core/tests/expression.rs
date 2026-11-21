mod common;

use codyscript_core::{Engine, Error, ErrorCode, RunOptions};
use codyscript_core::{Value, value};
use common::*;

fn eval(source: &str, variables: Value) -> Result<Value, Error> {
    Engine::new()
        .run_blocking(&x(source), with_vars(variables))
        .map(|result| result.value)
}

fn compile_error_at(source: &str, max_depth: Option<usize>) -> Error {
    let mut builder = Engine::builder();
    if let Some(depth) = max_depth {
        builder = builder.max_depth(depth);
    }
    compile_error(&builder.build(), &x(source))
}

#[test]
fn arithmetic() {
    let vars = value!({ "a": 2, "gross_sales": 100, "discounts": 20, "promotions": 5 });
    for (source, value) in [
        ("1 + 2 * 3", value!(7)),
        ("(1 + 2) * 3", value!(9)),
        ("10 / 4", value!(2.5)),
        ("2 - -1", value!(3)),
        ("10 - 4 - 3", value!(3)),
        ("-a + 1", value!(-1)),
        ("a * 0.03", value!(0.06)),
        ("gross_sales - discounts - promotions", value!(75)),
        ("0.1 + 0.2", value!(0.30000000000000004)),
        ("007", value!(7)),
        ("1.50", value!(1.5)),
    ] {
        assert_eq!(eval(source, vars.clone()).unwrap(), value, "{source}");
    }
}

#[test]
fn whole_results_are_integers() {
    assert_eq!(eval("5 / 2 * 2", value!({})).unwrap().as_i64(), Some(5));
    assert_eq!(eval("-0", value!({})).unwrap().as_i64(), Some(0));
    assert_eq!(eval("1e3", value!({})).ok(), None); // no exponents in the language
    let quarter = eval("0.5 + 0.25", value!({})).unwrap();
    assert!(quarter.as_number().unwrap().is_f64() && quarter == 0.75);
}

#[test]
fn comparisons() {
    let vars = value!({ "a": 1, "b": 2, "s": "paid", "n": null, "l": [], "o": {} });
    for (source, value) in [
        ("a < b", true),
        ("a <= 1", true),
        ("a > b", false),
        ("b >= 2", true),
        ("a == 1", true),
        ("a != 1", false),
        ("'apple' < 'banana'", true),
        ("s == 'paid'", true),
        ("n == null", true),
        ("true == true", true),
        ("1 == '1'", false),
        ("a < b && b < 3", true),
        ("a == 1.0", true),
        ("null != false", true),
        // Anything can be tested for null; a list or object never is.
        ("o == null", false),
        ("null != l", true),
        ("o?.missing == null", true),
        // Text orders by Unicode code point.
        ("'\u{FF5E}' < '\u{1F600}'", true),
        ("'Z' < 'a'", true),
    ] {
        assert_eq!(
            eval(source, vars.clone()).unwrap(),
            value!(value),
            "{source}"
        );
    }
}

#[test]
fn logic_is_on_booleans() {
    let vars = value!({ "t": true, "f": false });
    for (source, value) in [
        ("t && f", false),
        ("t && t", true),
        ("f || t", true),
        ("f || f", false),
        ("!f", true),
        ("!!t", true),
        ("1 < 2 && 'a' == 'a'", true),
    ] {
        assert_eq!(
            eval(source, vars.clone()).unwrap(),
            value!(value),
            "{source}"
        );
    }
}

#[test]
fn a_default_replaces_only_null() {
    let vars = value!({ "n": null, "z": 0, "e": "", "f": false, "l": [] });
    for (source, value) in [
        ("n ?? 5", value!(5)),
        ("z ?? 5", value!(0)),
        ("e ?? 'x'", value!("")),
        ("f ?? true", value!(false)),
        ("l ?? 1", value!([])),
        ("n ?? n ?? 'last'", value!("last")),
        // The loosest operator: `??` applies to the whole comparison.
        ("n ?? 1 < 2", value!(true)),
        ("(n ?? 1) + 1", value!(2)),
    ] {
        assert_eq!(eval(source, vars.clone()).unwrap(), value, "{source}");
    }
}

#[test]
fn a_default_skips_its_right_side_unless_needed() {
    // `missing` would be undefined: it's never evaluated.
    assert_eq!(eval("1 ?? missing", value!({})).unwrap(), value!(1));
    assert_eq!(run(&Engine::new(), &x("1 ?? 2 + 3")).unwrap().steps, 3);
}

#[test]
fn a_number_passed_through_keeps_its_exact_json_form() {
    let big = value!(u64::MAX);
    assert_eq!(eval("n ?? 0", value!({ "n": big.clone() })).unwrap(), big);
    assert!(
        eval("n", value!({ "n": 1.0 }))
            .unwrap()
            .as_number()
            .unwrap()
            .is_f64()
    );
}

#[test]
fn and_and_or_short_circuit() {
    assert_eq!(eval("false && missing", value!({})).unwrap(), value!(false));
    assert_eq!(eval("true || missing", value!({})).unwrap(), value!(true));
}

#[test]
fn literals_text_with_escapes_true_false_null() {
    assert_eq!(eval(r"'it\'s'", value!({})).unwrap(), value!("it's"));
    assert_eq!(
        eval(r"'a\\b\n\tc'", value!({})).unwrap(),
        value!("a\\b\n\tc")
    );
    assert_eq!(eval("''", value!({})).unwrap(), value!(""));
    assert_eq!(
        eval("'two\nlines'", value!({})).unwrap(),
        value!("two\nlines")
    );
    assert_eq!(eval("null", value!({})).unwrap(), Value::Null);
    assert_eq!(eval("  1  ", value!({})).unwrap(), value!(1));
    assert_eq!(eval("\u{A0}1\u{3000}", value!({})).unwrap(), value!(1));
}

#[test]
fn reads_the_enclosing_scope_and_works_anywhere_an_input_does() {
    let program = m(vec![
        set("i", value!(0)),
        value!({ "for": x("i < 3"), "do": [set("i", x("i + 1"))] }),
        value!({ "if": x("i == 3 && !done"), "do": [ret(m(vec![ret(x("i * 10"))]))], "el": [] }),
    ]);
    let result = Engine::new().run_blocking(&program, with_vars(value!({ "done": false })));
    assert_eq!(result.unwrap().value, value!(30));
}

#[test]
fn a_compiled_expression_runs_many_times() {
    let compiled = Engine::new().compile(&x("a * 2")).unwrap();
    assert_eq!(
        compiled_value(&compiled, with_vars(value!({ "a": 2 }))),
        value!(4)
    );
    assert_eq!(
        compiled_value(&compiled, with_vars(value!({ "a": 5 }))),
        value!(10)
    );
}

#[test]
fn an_undefined_variable_fails_at_its_expression_with_the_source() {
    let program = m(vec![set("fee", tagged(x("amount * rate"), "fee"))]);
    let error = failure(run(&Engine::new(), &program));
    assert_eq!(error.code(), ErrorCode::UndefinedVariable);
    assert_eq!(error.path(), "/do/0/set/fee/x");
    assert_eq!(error.tag(), Some("fee"));
    assert_eq!(
        error.message(),
        r#"Variable "amount" is not defined in "amount * rate" at /do/0/set/fee/x (tag fee)"#
    );
}

#[test]
fn type_errors() {
    let vars = value!({ "s": "x", "n": null, "l": [1], "o": { "a": 1 }, "big": 1e308 });
    for (source, message) in [
        ("1 + 'a'", r#""+" needs numbers, got number and string"#),
        ("-s", r#""-" needs a number, got string"#),
        ("!s", r#""!" needs a boolean, got string"#),
        ("n && true", r#""&&" needs booleans, got null"#),
        ("true && l", r#""&&" needs booleans, got list"#),
        ("false || 1", r#""||" needs booleans, got number"#),
        (
            "'a' < 1",
            r#""<" needs two numbers or two texts, got string and number"#,
        ),
        (
            "n >= 1",
            r#"">=" needs two numbers or two texts, got null and number"#,
        ),
        ("l == 1", r#""==" can't compare list and number"#),
        ("l == o", r#""==" can't compare list and object"#),
        ("1 != o", r#""!=" can't compare number and object"#),
        ("1 / 0", r#""/" divided by zero"#),
        ("-1 / 0", r#""/" divided by zero"#),
        ("0 / 0", r#""/" divided by zero"#),
        ("big * 10", r#""*" gave Infinity"#),
        ("-big - big", r#""-" gave -Infinity"#),
        ("true + 1", r#""+" needs numbers, got boolean and number"#),
        (
            "l < o",
            r#""<" needs two numbers or two texts, got list and object"#,
        ),
    ] {
        let error = eval(source, vars.clone()).unwrap_err();
        assert_eq!(error.code(), ErrorCode::TypeError, "{source}");
        assert_eq!(error.path(), "/x");
        assert_eq!(error.message(), format!("{message} in \"{source}\" at /x"));
    }
}

#[test]
fn every_operator_counts_toward_max_steps() {
    let limited = Engine::builder().max_steps(5).build();
    // The node (1), then 1 + 2 * 3: +, 1, *, 2, 3 (5 more).
    let error = failure(run(&limited, &x("1 + 2 * 3")));
    assert_eq!(error.code(), ErrorCode::StepLimit);
    assert_eq!(error.path(), "/x");
    assert_eq!(run(&Engine::new(), &x("1 + 2 * 3")).unwrap().steps, 6);
}

#[test]
fn an_operator_is_counted_before_its_operands_are_read() {
    // The node (1), then + (2), && (3) and a (4), which isn't defined.
    let at = |max_steps| {
        let engine = Engine::builder().max_steps(max_steps).build();
        failure(run(&engine, &x("(a && b) + b"))).code()
    };
    assert_eq!(at(2), ErrorCode::StepLimit); // at "+", before anything is read
    assert_eq!(at(3), ErrorCode::StepLimit); // at "&&"
    assert_eq!(at(4), ErrorCode::UndefinedVariable); // "a", read
}

#[test]
fn a_short_circuit_skips_the_steps_of_its_other_side() {
    assert_eq!(run(&Engine::new(), &x("false && 1 + 2")).unwrap().steps, 3);
}

#[test]
fn refuses_bad_syntax_at_compile_time() {
    for (source, message) in [
        ("1 +", "Expression ends too early"),
        ("", "Expected a non-empty string"),
        ("a b", r#"Unexpected "b""#),
        ("1 2", r#"Unexpected "2""#),
        ("1 'a'", r#"Unexpected "'a'""#),
        ("1 true", r#"Unexpected "true""#),
        ("1 2.50", r#"Unexpected "2.50""#),
        ("(1 + 2", r#"Missing ")""#),
        ("1 + 2)", r#"Unexpected ")""#),
        ("1 < 2 < 3", "Comparisons can't be chained; use &&"),
        ("a = 1", r#"Unexpected "=""#),
        ("a @ b", r#"Unexpected "@""#),
        ("'open", r#"Text has no closing "'""#),
        ("'ends in \\", r#"Text has no closing "'""#),
        (r"'a\q'", r#"Unknown escape "\q"; use \n, \t, \\ or \'"#),
        ("'bad\\\nescape'", r#"Unknown escape "\\n"; use"#),
        (&format!("1{}", "0".repeat(400)), "is too large"),
        ("*", r#"Unexpected "*""#),
        ("1.", r#"Expected a field name after ".""#),
        ("a.", r#"Expected a field name after ".""#),
        (
            "a.0",
            r#"Expected a field name after "."; for a list's item, use [0]"#,
        ),
        ("a.'b'", r#"Expected a field name after ".""#),
        ("a?.", r#"Expected a field name after "?.""#),
        ("a[0", r#"Missing "]""#),
        ("a[]", r#"Unexpected "]""#),
        ("a]", r#"Unexpected "]""#),
        (".a", r#"Unexpected ".""#),
        ("a?b", r#"Unexpected "?""#),
        ("a & b", r#"Unexpected "&""#),
    ] {
        let error = compile_error_at(source, None);
        assert_eq!(error.code(), ErrorCode::InvalidProgram, "{source}");
        assert_eq!(error.path(), "/x");
        assert!(
            error.message().contains(message),
            "{source}: {}",
            error.message()
        );
    }
}

fn steps(source: &str, variables: Value) -> u64 {
    Engine::new()
        .run_blocking(&x(source), with_vars(variables))
        .unwrap()
        .steps
}

#[test]
fn fields_and_items() {
    let vars = value!({
        "body": {
            "user": { "name": "Ada", "tags": ["x", "y"] },
            "content-type": "json",
            "null": 0,
            "type": "t"
        },
        "items": [{ "price": 10 }, { "price": 20 }],
        "grid": [[1, 2], [3, 4]],
        "i": 1,
        "key": "content-type"
    });
    for (source, value) in [
        ("body.user.name", value!("Ada")),
        ("body.user.tags[1]", value!("y")),
        ("items[0].price + items[i].price", value!(30)),
        ("items[i - 1].price * 2", value!(20)),
        ("items[1.0].price", value!(20)),
        ("grid[1][0]", value!(3)),
        // An index may be any text: a name that isn't a word, or computed.
        ("body['content-type']", value!("json")),
        ("body[key] == 'json'", value!(true)),
        // After `.`, any word is a field's name, keywords included.
        ("body.null", value!(0)),
        ("body.type", value!("t")),
        // Reads bind tighter than any operator.
        ("-items[1].price", value!(-20)),
        ("!(body.user.name == 'Ada')", value!(false)),
        ("(body).user . name", value!("Ada")),
        ("body.user", value!({ "name": "Ada", "tags": ["x", "y"] })),
    ] {
        assert_eq!(eval(source, vars.clone()).unwrap(), value, "{source}");
    }
}

#[test]
fn optional_reads_are_null_when_missing() {
    let vars = value!({
        "body": { "name": "Ada", "address": null },
        "items": [1],
        "n": null
    });
    for (source, value) in [
        ("body?.name", value!("Ada")),
        ("body?.nickname", value!(null)),
        ("body?.nickname ?? 'anon'", value!("anon")),
        ("body.address?.city", value!(null)),
        ("body?.address?.city ?? 'nowhere'", value!("nowhere")),
        ("body?['name']", value!("Ada")),
        ("items?[0]", value!(1)),
        ("items?[5]", value!(null)),
        ("n?.a", value!(null)),
        // An optional index of `null` isn't evaluated: `missing` is never read.
        ("n?[missing]", value!(null)),
    ] {
        assert_eq!(eval(source, vars.clone()).unwrap(), value, "{source}");
    }
}

#[test]
fn reading_what_isnt_there_is_a_type_error() {
    let vars = value!({
        "body": { "name": "Ada" },
        "items": [1, 2, 3],
        "n": null,
        "s": "abc",
        "num": 4
    });
    for (source, message) in [
        ("body.nmae", r#"Object has no field "nmae""#),
        ("body['nmae']", r#"Object has no field "nmae""#),
        ("items[9]", "Index 9 is out of range for a list of 3"),
        (
            "items[-1]",
            "List index must be a whole number from 0, got -1",
        ),
        (
            "items[1.5]",
            "List index must be a whole number from 0, got 1.5",
        ),
        ("items['0']", "List index must be a number, got string"),
        ("body[0]", "Object key must be a string, got number"),
        ("n.a", r#"".a" needs an object, got null"#),
        ("num.a", r#"".a" needs an object, got number"#),
        ("items.a", r#"".a" needs an object, got list"#),
        ("body.name.first", r#"".first" needs an object, got string"#),
        ("s[0]", r#""[]" needs a list or an object, got string"#),
        ("n[0]", r#""[]" needs a list or an object, got null"#),
        // Optional reads forgive absence, not the wrong kind of value.
        ("num?.a", r#"".a" needs an object, got number"#),
        (
            "items?[1.5]",
            "List index must be a whole number from 0, got 1.5",
        ),
        ("body?[0]", "Object key must be a string, got number"),
    ] {
        let error = eval(source, vars.clone()).unwrap_err();
        assert_eq!(error.code(), ErrorCode::TypeError, "{source}");
        assert_eq!(error.message(), format!("{message} in \"{source}\" at /x"));
    }
}

#[test]
fn each_read_is_a_step() {
    let vars = value!({ "o": { "a": [1] }, "n": null });
    // The node, then `[]`, `.a`, `o` and `0`.
    assert_eq!(steps("o.a[0]", vars.clone()), 5);
    // The node, `?[]` and `n`: the index is never evaluated.
    assert_eq!(steps("n?[0 + 1]", vars), 3);
}

#[test]
fn a_long_chain_of_reads_runs_into_the_step_limit_not_the_stack() {
    let source = format!("n{}", "?.a".repeat(100_001));
    let error = failure(Engine::new().run_blocking(&x(&source), with_vars(value!({ "n": null }))));
    assert_eq!(error.code(), ErrorCode::StepLimit);
}

#[test]
fn refuses_nesting_past_max_depth_parentheses_or_operators() {
    let indexes = format!("{}0{}", "a[".repeat(200), "]".repeat(200));
    assert!(
        compile_error_at(&indexes, None)
            .message()
            .contains("Expression nests deeper than 128 levels")
    );
    let parens = format!("{}1{}", "(".repeat(200), ")".repeat(200));
    assert!(
        compile_error_at(&parens, None)
            .message()
            .contains("Expression nests deeper than 128 levels")
    );
    let nots = format!("{}a", "!".repeat(200));
    assert!(
        compile_error_at(&nots, None)
            .message()
            .contains("nests deeper")
    );
    assert!(
        compile_error_at("((1))", Some(2))
            .message()
            .contains("nests deeper than 2")
    );
}

#[test]
fn a_long_chain_runs_into_the_step_limit_not_the_stack() {
    let source = vec!["1"; 100_001].join(" + ");
    let error = failure(run(&Engine::new(), &x(&source)));
    assert_eq!(error.code(), ErrorCode::StepLimit);
    let short = vec!["1"; 1_000].join(" + ");
    assert_eq!(run(&Engine::new(), &x(&short)).unwrap().value, value!(1000));
}

#[test]
fn refuses_a_non_string_expression() {
    assert_eq!(
        compile_error(&Engine::new(), &value!({ "x": 1 })).path(),
        "/x"
    );
}

#[test]
fn an_aborted_signal_stops_an_expression() {
    let signal = codyscript_core::Signal::new();
    signal.abort_with("stop");
    let options = RunOptions::new().with_signal(signal);
    let error = Engine::new()
        .run_blocking(&m(vec![ret(x("1"))]), options)
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::Aborted);
    assert_eq!(error.path(), "/do/0");
}
