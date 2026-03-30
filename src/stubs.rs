use crate::type_map::ClassMembers;

pub fn builtin_class_members(name: &str) -> Option<ClassMembers> {
    Some(match name {
        // Exception hierarchy
        "Throwable" | "Exception" | "Error" => exception_members(name),
        "RuntimeException"
        | "BadFunctionCallException"
        | "BadMethodCallException"
        | "DomainException"
        | "InvalidArgumentException"
        | "LengthException"
        | "LogicException"
        | "OutOfBoundsException"
        | "OutOfRangeException"
        | "OverflowException"
        | "RangeException"
        | "UnderflowException"
        | "UnexpectedValueException" => ClassMembers {
            parent: Some("Exception".to_string()),
            methods: exception_methods(),
            ..Default::default()
        },
        "TypeError" | "ValueError" | "ArithmeticError" | "DivisionByZeroError" | "ParseError" => {
            ClassMembers {
                parent: Some("Error".to_string()),
                methods: exception_methods(),
                ..Default::default()
            }
        }
        // DateTime
        "DateTime" => datetime_members(false),
        "DateTimeImmutable" => datetime_members(true),
        "DateInterval" => date_interval_members(),
        "DateTimeZone" => datetimezone_members(),
        // PDO
        "PDO" => pdo_members(),
        "PDOStatement" => pdo_statement_members(),
        "PDOException" => ClassMembers {
            parent: Some("RuntimeException".to_string()),
            methods: exception_methods(),
            ..Default::default()
        },
        // SPL
        "ArrayObject" | "ArrayIterator" => array_object_members(),
        "SplStack" | "SplQueue" | "SplDoublyLinkedList" => spl_list_members(),
        "SplFixedArray" => spl_fixed_array_members(),
        "SplPriorityQueue" | "SplMinHeap" | "SplMaxHeap" | "SplHeap" => spl_heap_members(),
        "SplObjectStorage" => spl_object_storage_members(),
        // Interfaces
        "Iterator" => ClassMembers {
            methods: m(&[
                ("current", false),
                ("key", false),
                ("next", false),
                ("rewind", false),
                ("valid", false),
            ]),
            ..Default::default()
        },
        "IteratorAggregate" => ClassMembers {
            methods: m(&[("getIterator", false)]),
            ..Default::default()
        },
        "Countable" => ClassMembers {
            methods: m(&[("count", false)]),
            ..Default::default()
        },
        "ArrayAccess" => ClassMembers {
            methods: m(&[
                ("offsetExists", false),
                ("offsetGet", false),
                ("offsetSet", false),
                ("offsetUnset", false),
            ]),
            ..Default::default()
        },
        "Stringable" => ClassMembers {
            methods: m(&[("__toString", false)]),
            ..Default::default()
        },
        // Closure, Generator, others
        "Closure" => closure_members(),
        "Generator" => generator_members(),
        "WeakReference" => ClassMembers {
            methods: m(&[("get", false), ("create", true)]),
            ..Default::default()
        },
        "stdClass" | "StdClass" => ClassMembers::default(),
        _ => return None,
    })
}

fn m(pairs: &[(&str, bool)]) -> Vec<(String, bool)> {
    pairs.iter().map(|(n, s)| (n.to_string(), *s)).collect()
}

fn p(pairs: &[(&str, bool)]) -> Vec<(String, bool)> {
    pairs.iter().map(|(n, s)| (n.to_string(), *s)).collect()
}

fn exception_methods() -> Vec<(String, bool)> {
    m(&[
        ("getMessage", false),
        ("getCode", false),
        ("getFile", false),
        ("getLine", false),
        ("getTrace", false),
        ("getTraceAsString", false),
        ("getPrevious", false),
        ("__toString", false),
    ])
}

fn exception_members(name: &str) -> ClassMembers {
    let parent = match name {
        "Throwable" => None,
        "Error" => None,
        _ => Some("Exception".to_string()),
    };
    ClassMembers {
        parent,
        methods: exception_methods(),
        ..Default::default()
    }
}

fn datetime_members(_immutable: bool) -> ClassMembers {
    ClassMembers {
        methods: m(&[
            ("format", false),
            ("modify", false),
            ("add", false),
            ("sub", false),
            ("diff", false),
            ("getTimestamp", false),
            ("setTimestamp", false),
            ("getTimezone", false),
            ("setTimezone", false),
            ("setDate", false),
            ("setTime", false),
            ("setISODate", false),
            ("createFromFormat", true),
            ("createFromTimestamp", true),
            ("createFromInterface", true),
        ]),
        ..Default::default()
    }
}

fn date_interval_members() -> ClassMembers {
    ClassMembers {
        properties: p(&[
            ("y", false),
            ("m", false),
            ("d", false),
            ("h", false),
            ("i", false),
            ("s", false),
            ("f", false),
            ("invert", false),
            ("days", false),
        ]),
        methods: m(&[("format", false), ("createFromDateString", true)]),
        ..Default::default()
    }
}

fn datetimezone_members() -> ClassMembers {
    ClassMembers {
        methods: m(&[
            ("getName", false),
            ("getOffset", false),
            ("getTransitions", false),
            ("getLocation", false),
            ("listAbbreviations", true),
            ("listIdentifiers", true),
        ]),
        ..Default::default()
    }
}

fn pdo_members() -> ClassMembers {
    ClassMembers {
        methods: m(&[
            ("query", false),
            ("prepare", false),
            ("exec", false),
            ("lastInsertId", false),
            ("beginTransaction", false),
            ("commit", false),
            ("rollBack", false),
            ("inTransaction", false),
            ("quote", false),
            ("errorCode", false),
            ("errorInfo", false),
            ("getAttribute", false),
            ("setAttribute", false),
            ("getAvailableDrivers", true),
        ]),
        constants: vec![
            "ATTR_ERRMODE".into(),
            "ERRMODE_EXCEPTION".into(),
            "ERRMODE_SILENT".into(),
            "ERRMODE_WARNING".into(),
            "FETCH_ASSOC".into(),
            "FETCH_OBJ".into(),
            "FETCH_NUM".into(),
            "FETCH_BOTH".into(),
            "FETCH_CLASS".into(),
            "FETCH_INTO".into(),
            "FETCH_LAZY".into(),
            "FETCH_NAMED".into(),
            "PARAM_INT".into(),
            "PARAM_STR".into(),
            "PARAM_BOOL".into(),
            "PARAM_NULL".into(),
            "PARAM_LOB".into(),
        ],
        ..Default::default()
    }
}

fn pdo_statement_members() -> ClassMembers {
    ClassMembers {
        methods: m(&[
            ("fetch", false),
            ("fetchAll", false),
            ("fetchColumn", false),
            ("fetchObject", false),
            ("bindParam", false),
            ("bindValue", false),
            ("bindColumn", false),
            ("execute", false),
            ("rowCount", false),
            ("columnCount", false),
            ("closeCursor", false),
            ("debugDumpParams", false),
            ("errorCode", false),
            ("errorInfo", false),
            ("getColumnMeta", false),
            ("nextRowset", false),
            ("setFetchMode", false),
        ]),
        ..Default::default()
    }
}

fn array_object_members() -> ClassMembers {
    ClassMembers {
        methods: m(&[
            ("append", false),
            ("count", false),
            ("getArrayCopy", false),
            ("getIterator", false),
            ("offsetExists", false),
            ("offsetGet", false),
            ("offsetSet", false),
            ("offsetUnset", false),
            ("asort", false),
            ("ksort", false),
            ("uasort", false),
            ("uksort", false),
            ("serialize", false),
            ("unserialize", false),
        ]),
        ..Default::default()
    }
}

fn spl_list_members() -> ClassMembers {
    ClassMembers {
        methods: m(&[
            ("push", false),
            ("pop", false),
            ("top", false),
            ("bottom", false),
            ("shift", false),
            ("unshift", false),
            ("count", false),
            ("isEmpty", false),
            ("current", false),
            ("key", false),
            ("next", false),
            ("prev", false),
            ("rewind", false),
            ("valid", false),
            ("offsetExists", false),
            ("offsetGet", false),
            ("offsetSet", false),
            ("offsetUnset", false),
        ]),
        ..Default::default()
    }
}

fn spl_fixed_array_members() -> ClassMembers {
    ClassMembers {
        methods: m(&[
            ("count", false),
            ("getSize", false),
            ("setSize", false),
            ("current", false),
            ("key", false),
            ("next", false),
            ("prev", false),
            ("rewind", false),
            ("valid", false),
            ("offsetExists", false),
            ("offsetGet", false),
            ("offsetSet", false),
            ("offsetUnset", false),
            ("toArray", false),
            ("fromArray", true),
        ]),
        ..Default::default()
    }
}

fn spl_heap_members() -> ClassMembers {
    ClassMembers {
        methods: m(&[
            ("insert", false),
            ("top", false),
            ("extract", false),
            ("count", false),
            ("isEmpty", false),
            ("current", false),
            ("key", false),
            ("next", false),
            ("rewind", false),
            ("valid", false),
            ("recoverFromCorruption", false),
        ]),
        ..Default::default()
    }
}

fn spl_object_storage_members() -> ClassMembers {
    ClassMembers {
        methods: m(&[
            ("attach", false),
            ("detach", false),
            ("contains", false),
            ("count", false),
            ("addAll", false),
            ("removeAll", false),
            ("current", false),
            ("key", false),
            ("next", false),
            ("rewind", false),
            ("valid", false),
            ("getInfo", false),
            ("setInfo", false),
            ("offsetExists", false),
            ("offsetGet", false),
            ("offsetSet", false),
            ("offsetUnset", false),
        ]),
        ..Default::default()
    }
}

fn closure_members() -> ClassMembers {
    ClassMembers {
        methods: m(&[
            ("bind", true),
            ("bindTo", false),
            ("call", false),
            ("fromCallable", true),
        ]),
        ..Default::default()
    }
}

fn generator_members() -> ClassMembers {
    ClassMembers {
        methods: m(&[
            ("current", false),
            ("key", false),
            ("next", false),
            ("rewind", false),
            ("valid", false),
            ("send", false),
            ("throw", false),
            ("getReturn", false),
        ]),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exception_has_get_message() {
        let members = builtin_class_members("Exception").unwrap();
        assert!(members.methods.iter().any(|(n, _)| n == "getMessage"));
    }

    #[test]
    fn pdo_has_prepare_method() {
        let members = builtin_class_members("PDO").unwrap();
        assert!(members.methods.iter().any(|(n, _)| n == "prepare"));
    }

    #[test]
    fn pdo_has_fetch_assoc_constant() {
        let members = builtin_class_members("PDO").unwrap();
        assert!(members.constants.iter().any(|n| n == "FETCH_ASSOC"));
    }

    #[test]
    fn unknown_class_returns_none() {
        assert!(builtin_class_members("MyCustomClass").is_none());
    }

    #[test]
    fn datetime_has_format_method() {
        let members = builtin_class_members("DateTime").unwrap();
        assert!(members.methods.iter().any(|(n, _)| n == "format"));
    }

    #[test]
    fn generator_has_send_method() {
        let members = builtin_class_members("Generator").unwrap();
        assert!(members.methods.iter().any(|(n, _)| n == "send"));
    }
}
