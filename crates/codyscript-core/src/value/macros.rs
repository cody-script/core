//! [`value!`](crate::value!): values written as literals.

/// Builds a [`Value`](crate::Value) from literal syntax: `null`, `true`,
/// `false`, numbers, strings, `[lists]` and `{ "objects": … }`, nested freely.
/// Anything else is an expression converted with `Value::from`, so Rust
/// variables and function calls can be spliced in. Object keys are string
/// literals or any expression that converts into a `String` (parenthesize
/// complex ones); they keep the order they're written in.
///
/// ```
/// use codyscript_core::{value, Value};
/// let name = "total";
/// let program = value!({ "do": [
///     { "set": { (name): { "x": "price * 2" } } },
///     { "ret": { "ref": name } }
/// ] });
/// assert_eq!(program["do"][1]["ret"]["ref"], "total");
/// assert_eq!(value!([1, null, -2.5]), Value::from(vec![Value::from(1), Value::Null, Value::from(-2.5)]));
/// ```
#[macro_export]
macro_rules! value {
    ($($value:tt)+) => {
        $crate::__value!($($value)+)
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! __value {
    // ── Lists: munch elements into [built,] ─────────────────────────────────
    (@array [$($elems:expr,)*]) => {
        ::std::vec![$($elems,)*]
    };
    (@array [$($elems:expr),*]) => {
        ::std::vec![$($elems),*]
    };
    (@array [$($elems:expr,)*] null $($rest:tt)*) => {
        $crate::__value!(@array [$($elems,)* $crate::__value!(null)] $($rest)*)
    };
    (@array [$($elems:expr,)*] true $($rest:tt)*) => {
        $crate::__value!(@array [$($elems,)* $crate::__value!(true)] $($rest)*)
    };
    (@array [$($elems:expr,)*] false $($rest:tt)*) => {
        $crate::__value!(@array [$($elems,)* $crate::__value!(false)] $($rest)*)
    };
    (@array [$($elems:expr,)*] [$($array:tt)*] $($rest:tt)*) => {
        $crate::__value!(@array [$($elems,)* $crate::__value!([$($array)*])] $($rest)*)
    };
    (@array [$($elems:expr,)*] {$($map:tt)*} $($rest:tt)*) => {
        $crate::__value!(@array [$($elems,)* $crate::__value!({$($map)*})] $($rest)*)
    };
    (@array [$($elems:expr,)*] $next:expr, $($rest:tt)*) => {
        $crate::__value!(@array [$($elems,)* $crate::__value!($next),] $($rest)*)
    };
    (@array [$($elems:expr,)*] $last:expr) => {
        $crate::__value!(@array [$($elems,)* $crate::__value!($last)])
    };
    (@array [$($elems:expr),*] , $($rest:tt)*) => {
        $crate::__value!(@array [$($elems,)*] $($rest)*)
    };

    // ── Objects: (key tokens so far) (remaining tokens) ─────────────────────
    // Done.
    (@object $object:ident () ()) => {};
    // Insert an entry, then carry on after its comma.
    (@object $object:ident [$($key:tt)+] ($value:expr) , $($rest:tt)*) => {
        let _ = $object.insert(::std::string::String::from($($key)+), $value);
        $crate::__value!(@object $object () ($($rest)*));
    };
    // Insert the last entry.
    (@object $object:ident [$($key:tt)+] ($value:expr)) => {
        let _ = $object.insert(::std::string::String::from($($key)+), $value);
    };
    // The value after a key's colon.
    (@object $object:ident ($($key:tt)+) (: null $($rest:tt)*)) => {
        $crate::__value!(@object $object [$($key)+] ($crate::__value!(null)) $($rest)*);
    };
    (@object $object:ident ($($key:tt)+) (: true $($rest:tt)*)) => {
        $crate::__value!(@object $object [$($key)+] ($crate::__value!(true)) $($rest)*);
    };
    (@object $object:ident ($($key:tt)+) (: false $($rest:tt)*)) => {
        $crate::__value!(@object $object [$($key)+] ($crate::__value!(false)) $($rest)*);
    };
    (@object $object:ident ($($key:tt)+) (: [$($array:tt)*] $($rest:tt)*)) => {
        $crate::__value!(@object $object [$($key)+] ($crate::__value!([$($array)*])) $($rest)*);
    };
    (@object $object:ident ($($key:tt)+) (: {$($map:tt)*} $($rest:tt)*)) => {
        $crate::__value!(@object $object [$($key)+] ($crate::__value!({$($map)*})) $($rest)*);
    };
    (@object $object:ident ($($key:tt)+) (: $value:expr , $($rest:tt)*)) => {
        $crate::__value!(@object $object [$($key)+] ($crate::__value!($value)) , $($rest)*);
    };
    (@object $object:ident ($($key:tt)+) (: $value:expr)) => {
        $crate::__value!(@object $object [$($key)+] ($crate::__value!($value)));
    };
    // A parenthesized key.
    (@object $object:ident () (($key:expr) : $($rest:tt)*)) => {
        $crate::__value!(@object $object ($key) (: $($rest)*));
    };
    // Munch one more token into the key.
    (@object $object:ident ($($key:tt)*) ($tt:tt $($rest:tt)*)) => {
        $crate::__value!(@object $object ($($key)* $tt) ($($rest)*));
    };

    // ── Entry points ────────────────────────────────────────────────────────
    (null) => {
        $crate::Value::Null
    };
    (true) => {
        $crate::Value::Bool(true)
    };
    (false) => {
        $crate::Value::Bool(false)
    };
    ([]) => {
        $crate::Value::Array(::std::vec::Vec::new())
    };
    ([ $($tt:tt)+ ]) => {
        $crate::Value::Array($crate::__value!(@array [] $($tt)+))
    };
    ({}) => {
        $crate::Value::Object($crate::Map::new())
    };
    ({ $($tt:tt)+ }) => {
        $crate::Value::Object({
            let mut object = $crate::Map::new();
            $crate::__value!(@object object () ($($tt)+));
            object
        })
    };
    // Anything else converts: numbers, strings, variables, calls.
    ($other:expr) => {
        $crate::Value::from($other)
    };
}

#[cfg(test)]
mod tests {
    use crate::{Map, Number, Value};

    #[test]
    fn literals() {
        assert_eq!(value!(null), Value::Null);
        assert_eq!(value!(true), Value::Bool(true));
        assert_eq!(value!(-1), Value::Number(Number::from(-1)));
        assert_eq!(value!("s"), Value::String("s".into()));
        assert_eq!(value!([]), Value::Array(vec![]));
        assert_eq!(value!({}), Value::Object(Map::new()));
    }

    #[test]
    fn nesting_trailing_commas_and_spliced_expressions() {
        let n = 2;
        let key = String::from("dynamic");
        let v = value!({
            "list": [1, [null, false], { "a": n + 1 }, "x",],
            (key): -n,
            "nested": { "t": true, },
        });
        assert_eq!(v["list"][1][1], false);
        assert_eq!(v["list"][2]["a"], 3);
        assert_eq!(v["dynamic"], -2);
        assert_eq!(v["nested"]["t"], true);
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["list", "dynamic", "nested"]);
    }

    #[test]
    fn splices_any_edition_2024_expression() {
        // Edition 2024's `expr` fragment also matches inline `const` blocks.
        let v = value!({ "a": const { 1 + 1 }, "b": [const { 3 }] });
        assert_eq!(v, value!({ "a": 2, "b": [3] }));
    }
}
