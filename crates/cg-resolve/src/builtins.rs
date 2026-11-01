//! Names that are part of the language or its standard library and must never be linked to a
//! same-named project symbol (design doc 6.5 step 7, upstream `js-builtins.ts` and friends).

use cg_core::LangFamily;

/// Free functions / globals / types that exist without an import.
pub fn is_builtin(family: LangFamily, name: &str) -> bool {
    match family {
        LangFamily::Js => matches!(
            name,
            "console" | "Math" | "JSON" | "Object" | "Array" | "String" | "Number" | "Boolean" | "Symbol" | "BigInt"
                | "Promise" | "Map" | "Set" | "WeakMap" | "WeakSet" | "Date" | "RegExp" | "Error" | "TypeError"
                | "RangeError" | "SyntaxError" | "Function" | "Reflect" | "Proxy" | "Intl" | "parseInt" | "parseFloat"
                | "isNaN" | "isFinite" | "setTimeout" | "setInterval" | "clearTimeout" | "clearInterval"
                | "setImmediate" | "queueMicrotask" | "require" | "module" | "exports" | "process" | "Buffer"
                | "global" | "globalThis" | "window" | "document" | "navigator" | "fetch" | "URL" | "URLSearchParams"
                | "encodeURIComponent" | "decodeURIComponent" | "encodeURI" | "decodeURI" | "structuredClone"
                | "atob" | "btoa" | "alert" | "super" | "Uint8Array" | "Float32Array" | "Float64Array" | "Int32Array"
                | "ArrayBuffer" | "DataView" | "TextEncoder" | "TextDecoder" | "AbortController" | "Event"
                | "EventTarget" | "Record" | "Partial" | "Required" | "Readonly" | "Pick" | "Omit" | "Exclude"
                | "Extract" | "NonNullable" | "ReturnType" | "Parameters" | "InstanceType" | "Awaited" | "Iterable"
                | "Iterator" | "AsyncIterable" | "PromiseLike" | "ArrayLike" | "any" | "unknown" | "never" | "void"
                | "string" | "number" | "boolean" | "object" | "undefined" | "null" | "this"
        ),
        LangFamily::Python => matches!(
            name,
            "print" | "len" | "range" | "enumerate" | "zip" | "map" | "filter" | "sorted" | "reversed" | "sum" | "min"
                | "max" | "abs" | "all" | "any" | "isinstance" | "issubclass" | "hasattr" | "getattr" | "setattr"
                | "delattr" | "callable" | "super" | "type" | "id" | "hash" | "repr" | "str" | "int" | "float"
                | "bool" | "list" | "dict" | "set" | "tuple" | "frozenset" | "bytes" | "bytearray" | "object"
                | "open" | "input" | "iter" | "next" | "format" | "round" | "pow" | "divmod" | "chr" | "ord" | "hex"
                | "oct" | "bin" | "vars" | "dir" | "globals" | "locals" | "exec" | "eval" | "compile" | "property"
                | "staticmethod" | "classmethod" | "slice" | "memoryview" | "NotImplemented" | "Ellipsis"
                | "Exception" | "ValueError" | "TypeError" | "KeyError" | "IndexError" | "AttributeError"
                | "RuntimeError" | "NotImplementedError" | "StopIteration" | "OSError" | "IOError" | "ImportError"
                | "AssertionError" | "ZeroDivisionError" | "FileNotFoundError" | "PermissionError" | "TimeoutError"
                | "BaseException" | "KeyboardInterrupt" | "SystemExit" | "UnicodeDecodeError" | "UnicodeEncodeError"
        ),
        LangFamily::Go => matches!(
            name,
            "make" | "new" | "len" | "cap" | "append" | "copy" | "delete" | "panic" | "recover" | "print" | "println"
                | "close" | "complex" | "real" | "imag" | "min" | "max" | "clear" | "string" | "int" | "int8" | "int16"
                | "int32" | "int64" | "uint" | "uint8" | "uint16" | "uint32" | "uint64" | "uintptr" | "float32"
                | "float64" | "bool" | "byte" | "rune" | "error" | "any" | "complex64" | "complex128" | "nil" | "true"
                | "false" | "iota"
        ),
        LangFamily::Jvm => matches!(
            name,
            "String" | "Object" | "Integer" | "Long" | "Double" | "Float" | "Boolean" | "Byte" | "Short" | "Character"
                | "Math" | "System" | "Thread" | "Runnable" | "Exception" | "RuntimeException" | "Error" | "Throwable"
                | "StringBuilder" | "Class" | "Iterable" | "Comparable" | "Override" | "Deprecated"
                | "SuppressWarnings" | "FunctionalInterface" | "Void" | "Number" | "Enum" | "Record" | "Any" | "Unit"
                | "Int" | "Nothing" | "Array" | "List" | "Map" | "Set" | "Pair" | "println" | "print" | "listOf"
                | "mapOf" | "setOf" | "mutableListOf" | "mutableMapOf" | "mutableSetOf" | "arrayOf" | "emptyList"
                | "emptyMap" | "require" | "check" | "error" | "lazy" | "run" | "let" | "apply" | "also" | "with"
                | "to" | "super" | "this" | "assert" | "TODO" | "repeat"
        ),
        LangFamily::Rust => matches!(
            name,
            "Some" | "None" | "Ok" | "Err" | "Vec" | "String" | "Box" | "Option" | "Result" | "Default" | "Clone" | "Copy"
                | "Send" | "Sync" | "Sized" | "Drop" | "From" | "Into" | "AsRef" | "AsMut" | "Iterator" | "IntoIterator"
                | "Fn" | "FnMut" | "FnOnce" | "ToString" | "ToOwned" | "PartialEq" | "Eq" | "PartialOrd" | "Ord"
                | "Debug" | "Display" | "drop" | "panic" | "println" | "print" | "eprintln" | "eprint" | "format"
                | "vec" | "assert" | "assert_eq" | "assert_ne" | "debug_assert" | "write" | "writeln" | "todo"
                | "unimplemented" | "unreachable" | "matches" | "dbg" | "include_str" | "env" | "cfg" | "concat"
                | "stringify" | "line" | "file" | "column" | "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8"
                | "i16" | "i32" | "i64" | "i128" | "isize" | "f32" | "f64" | "bool" | "char" | "str" | "self" | "Self"
                | "super" | "crate"
        ),
        _ => false,
    }
}

/// Method names so common on standard-library types that a call on an *untyped* receiver
/// must not be linked to a project method of the same name.
pub fn is_common_method(family: LangFamily, name: &str) -> bool {
    match family {
        LangFamily::Js => matches!(
            name,
            "push" | "pop" | "shift" | "unshift" | "slice" | "splice" | "map" | "filter" | "reduce" | "forEach" | "find"
                | "findIndex" | "some" | "every" | "join" | "split" | "trim" | "replace" | "get" | "set" | "has" | "add"
                | "delete" | "clear" | "keys" | "values" | "entries" | "then" | "catch" | "finally" | "toString"
                | "log" | "error" | "warn" | "info" | "debug" | "call" | "apply" | "bind" | "includes" | "indexOf"
                | "concat" | "sort" | "reverse" | "startsWith" | "endsWith" | "toLowerCase" | "toUpperCase"
                | "substring" | "substr" | "charAt" | "match" | "test" | "exec" | "parse" | "stringify" | "assign"
                | "freeze" | "toFixed" | "valueOf" | "flat" | "flatMap" | "fill" | "from" | "of" | "isArray" | "now"
                | "resolve" | "reject" | "all" | "race" | "length" | "size" | "next" | "return" | "throw" | "close"
                | "end" | "write" | "read" | "open" | "send" | "on" | "off" | "once" | "emit" | "removeListener"
                | "addEventListener" | "removeEventListener" | "preventDefault" | "stopPropagation"
                | "querySelector" | "getElementById" | "appendChild" | "removeChild" | "setAttribute"
                | "getAttribute" | "toJSON" | "hasOwnProperty" | "dispatch" | "subscribe" | "unsubscribe"
        ),
        LangFamily::Python => matches!(
            name,
            "append" | "extend" | "insert" | "remove" | "pop" | "clear" | "index" | "count" | "sort" | "reverse" | "copy"
                | "get" | "keys" | "values" | "items" | "update" | "setdefault" | "add" | "discard" | "union"
                | "intersection" | "difference" | "join" | "split" | "strip" | "lstrip" | "rstrip" | "replace" | "find"
                | "format" | "lower" | "upper" | "startswith" | "endswith" | "encode" | "decode" | "read" | "write"
                | "close" | "open" | "readline" | "readlines" | "flush" | "seek" | "isdigit" | "isalpha" | "title"
                | "capitalize" | "splitlines" | "partition" | "info" | "debug" | "warning" | "error" | "exception"
                | "critical" | "run" | "start" | "stop" | "send" | "recv" | "connect" | "put" | "post" | "delete"
                | "patch" | "head" | "options" | "json" | "text" | "save" | "load" | "dump" | "dumps" | "loads"
        ),
        LangFamily::Go => matches!(
            name,
            "Error" | "String" | "Close" | "Read" | "Write" | "Len" | "Less" | "Swap" | "Lock" | "Unlock" | "RLock"
                | "RUnlock" | "Add" | "Done" | "Wait" | "Println" | "Printf" | "Sprintf" | "Fprintf" | "Errorf"
                | "New" | "Get" | "Set" | "Run" | "Start" | "Stop" | "Init" | "Do" | "Next" | "Scan" | "Reset" | "Flush"
                | "Unwrap" | "Is" | "As" | "Format" | "Marshal" | "Unmarshal" | "Encode" | "Decode" | "Value" | "Key"
        ),
        LangFamily::Jvm => matches!(
            name,
            "get" | "set" | "put" | "add" | "remove" | "contains" | "size" | "isEmpty" | "clear" | "equals" | "hashCode"
                | "toString" | "length" | "append" | "toList" | "stream" | "map" | "filter" | "collect" | "forEach"
                | "close" | "run" | "start" | "stop" | "next" | "hasNext" | "iterator" | "valueOf" | "getName"
                | "getClass" | "compareTo" | "println" | "print" | "format" | "of" | "asList" | "build" | "create"
                | "accept" | "apply" | "test" | "call" | "invoke" | "read" | "write" | "info" | "debug" | "warn"
                | "error" | "trace" | "let" | "also" | "apply_" | "isNotEmpty" | "first" | "last" | "joinToString"
        ),
        LangFamily::Rust => matches!(
            name,
            "new" | "clone" | "len" | "is_empty" | "iter" | "iter_mut" | "into_iter" | "push" | "pop" | "insert" | "remove"
                | "get" | "get_mut" | "contains" | "contains_key" | "unwrap" | "expect" | "unwrap_or" | "unwrap_or_else"
                | "unwrap_or_default" | "map" | "map_err" | "and_then" | "ok_or" | "ok" | "err" | "as_ref" | "as_mut"
                | "to_string" | "to_owned" | "into" | "from" | "default" | "fmt" | "eq" | "cmp" | "partial_cmp" | "hash"
                | "next" | "collect" | "filter" | "fold" | "any" | "all" | "find" | "position" | "take" | "skip" | "rev"
                | "zip" | "chain" | "enumerate" | "join" | "split" | "trim" | "lines" | "bytes" | "chars" | "as_str"
                | "as_bytes" | "lock" | "read" | "write" | "send" | "recv" | "borrow" | "borrow_mut" | "drop" | "extend"
                | "clear" | "sort" | "sort_by" | "dedup" | "first" | "last" | "min" | "max" | "sum" | "count" | "flush"
                | "close" | "run" | "build" | "with" | "is_some" | "is_none" | "is_ok" | "is_err" | "or_insert"
                | "entry" | "or_default" | "keys" | "values" | "set" | "add" | "path" | "display" | "exists"
        ),
        _ => false,
    }
}
