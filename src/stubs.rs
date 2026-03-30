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
        // WeakMap (PHP 8.0)
        "WeakMap" => ClassMembers {
            methods: m(&[
                ("count", false),
                ("offsetExists", false),
                ("offsetGet", false),
                ("offsetSet", false),
                ("offsetUnset", false),
            ]),
            ..Default::default()
        },
        // Fiber (PHP 8.1)
        "Fiber" => fiber_members(),
        // mysqli
        "mysqli" => mysqli_members(),
        "mysqli_result" => mysqli_result_members(),
        "mysqli_stmt" => mysqli_stmt_members(),
        "mysqli_driver" => ClassMembers {
            properties: p(&[
                ("client_info", false),
                ("client_version", false),
                ("driver_version", false),
                ("embedded", false),
                ("reconnect", false),
                ("report_mode", false),
            ]),
            ..Default::default()
        },
        // SPL file / directory iterators
        "SplFileInfo" => spl_file_info_members(),
        "SplFileObject" => spl_file_object_members(),
        "DirectoryIterator" | "FilesystemIterator" | "GlobIterator" => {
            directory_iterator_members()
        }
        "RecursiveDirectoryIterator" => ClassMembers {
            parent: Some("FilesystemIterator".to_string()),
            methods: m(&[
                ("getChildren", false),
                ("getSubPath", false),
                ("getSubPathname", false),
                ("hasChildren", false),
                ("key", false),
                ("rewind", false),
            ]),
            ..Default::default()
        },
        // DOM extension
        "DOMNode" => dom_node_members(),
        "DOMDocument" => dom_document_members(),
        "DOMElement" => dom_element_members(),
        "DOMNodeList" => ClassMembers {
            methods: m(&[("item", false), ("count", false)]),
            properties: p(&[("length", false)]),
            ..Default::default()
        },
        "DOMAttr" => ClassMembers {
            parent: Some("DOMNode".to_string()),
            properties: p(&[("name", false), ("ownerElement", false), ("value", false)]),
            ..Default::default()
        },
        "DOMText" => ClassMembers {
            parent: Some("DOMNode".to_string()),
            methods: m(&[("isWhitespaceInElementContent", false), ("splitText", false)]),
            properties: p(&[("wholeText", false)]),
            ..Default::default()
        },
        "DOMXPath" => ClassMembers {
            methods: m(&[
                ("evaluate", false),
                ("query", false),
                ("registerNamespace", false),
                ("registerPhpFunctions", false),
            ]),
            ..Default::default()
        },
        "DOMException" => ClassMembers {
            parent: Some("RuntimeException".to_string()),
            methods: exception_methods(),
            ..Default::default()
        },
        // SimpleXML
        "SimpleXMLElement" => simple_xml_element_members(),
        "SimpleXMLIterator" => ClassMembers {
            parent: Some("SimpleXMLElement".to_string()),
            methods: m(&[
                ("current", false),
                ("key", false),
                ("next", false),
                ("rewind", false),
                ("valid", false),
                ("getChildren", false),
                ("hasChildren", false),
            ]),
            ..Default::default()
        },
        // XML
        "XMLReader" => xml_reader_members(),
        "XMLWriter" => xml_writer_members(),
        // ZipArchive
        "ZipArchive" => zip_archive_members(),
        // Reflection
        "ReflectionClass" => reflection_class_members(),
        "ReflectionMethod" => reflection_method_members(),
        "ReflectionProperty" => reflection_property_members(),
        "ReflectionFunction" => reflection_function_members(),
        "ReflectionParameter" => reflection_parameter_members(),
        "ReflectionException" => ClassMembers {
            parent: Some("Exception".to_string()),
            methods: exception_methods(),
            ..Default::default()
        },
        "ReflectionNamedType" | "ReflectionType" | "ReflectionUnionType" => ClassMembers {
            methods: m(&[("getName", false), ("isBuiltin", false), ("allowsNull", false)]),
            ..Default::default()
        },
        // Errors/exceptions: PHP 8.x
        "JsonException" => ClassMembers {
            parent: Some("RuntimeException".to_string()),
            methods: exception_methods(),
            ..Default::default()
        },
        "FiberError" => ClassMembers {
            parent: Some("Error".to_string()),
            methods: exception_methods(),
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

fn fiber_members() -> ClassMembers {
    ClassMembers {
        methods: m(&[
            ("start", false),
            ("resume", false),
            ("getReturn", false),
            ("isStarted", false),
            ("isRunning", false),
            ("isSuspended", false),
            ("isTerminated", false),
            ("getCurrent", true),
            ("suspend", true),
        ]),
        ..Default::default()
    }
}

fn mysqli_members() -> ClassMembers {
    ClassMembers {
        methods: m(&[
            ("query", false),
            ("prepare", false),
            ("real_query", false),
            ("multi_query", false),
            ("execute", false),
            ("store_result", false),
            ("use_result", false),
            ("next_result", false),
            ("close", false),
            ("autocommit", false),
            ("begin_transaction", false),
            ("commit", false),
            ("rollback", false),
            ("ping", false),
            ("real_escape_string", false),
            ("escape_string", false),
            ("character_set_name", false),
            ("get_charset", false),
            ("set_charset", false),
            ("select_db", false),
            ("get_server_info", false),
            ("get_host_info", false),
            ("dump_debug_info", false),
        ]),
        properties: p(&[
            ("connect_error", false),
            ("connect_errno", false),
            ("error", false),
            ("errno", false),
            ("affected_rows", false),
            ("insert_id", false),
            ("field_count", false),
            ("warning_count", false),
            ("info", false),
            ("server_info", false),
            ("host_info", false),
            ("protocol_version", false),
            ("sqlstate", false),
        ]),
        ..Default::default()
    }
}

fn mysqli_result_members() -> ClassMembers {
    ClassMembers {
        methods: m(&[
            ("fetch_all", false),
            ("fetch_array", false),
            ("fetch_assoc", false),
            ("fetch_object", false),
            ("fetch_row", false),
            ("fetch_column", false),
            ("free", false),
            ("close", false),
            ("field_seek", false),
            ("fetch_field", false),
            ("fetch_fields", false),
            ("fetch_field_direct", false),
        ]),
        properties: p(&[
            ("num_rows", false),
            ("num_fields", false),
            ("current_field", false),
            ("lengths", false),
        ]),
        ..Default::default()
    }
}

fn mysqli_stmt_members() -> ClassMembers {
    ClassMembers {
        methods: m(&[
            ("execute", false),
            ("fetch", false),
            ("bind_param", false),
            ("bind_result", false),
            ("store_result", false),
            ("get_result", false),
            ("close", false),
            ("reset", false),
            ("send_long_data", false),
            ("free_result", false),
        ]),
        properties: p(&[
            ("affected_rows", false),
            ("insert_id", false),
            ("num_rows", false),
            ("param_count", false),
            ("field_count", false),
            ("errno", false),
            ("error", false),
            ("error_list", false),
            ("sqlstate", false),
            ("id", false),
        ]),
        ..Default::default()
    }
}

fn spl_file_info_members() -> ClassMembers {
    ClassMembers {
        methods: m(&[
            ("getBasename", false),
            ("getExtension", false),
            ("getFilename", false),
            ("getPath", false),
            ("getPathname", false),
            ("getRealPath", false),
            ("getType", false),
            ("isDir", false),
            ("isFile", false),
            ("isLink", false),
            ("isReadable", false),
            ("isWritable", false),
            ("isExecutable", false),
            ("openFile", false),
            ("getATime", false),
            ("getCTime", false),
            ("getMTime", false),
            ("getPerms", false),
            ("getSize", false),
            ("getOwner", false),
            ("getGroup", false),
            ("getInode", false),
            ("getLinkTarget", false),
            ("__toString", false),
        ]),
        ..Default::default()
    }
}

fn spl_file_object_members() -> ClassMembers {
    ClassMembers {
        parent: Some("SplFileInfo".to_string()),
        methods: m(&[
            ("fseek", false),
            ("fgets", false),
            ("fgetcsv", false),
            ("fgetss", false),
            ("fwrite", false),
            ("feof", false),
            ("fflush", false),
            ("ftell", false),
            ("ftruncate", false),
            ("fstat", false),
            ("flock", false),
            ("fputcsv", false),
            ("fscanf", false),
            ("getCurrentLine", false),
            ("getMaxLineLen", false),
            ("setMaxLineLen", false),
            ("setFlags", false),
            ("getFlags", false),
            ("current", false),
            ("key", false),
            ("next", false),
            ("rewind", false),
            ("valid", false),
            ("seek", false),
            ("getChildren", false),
            ("hasChildren", false),
        ]),
        ..Default::default()
    }
}

fn directory_iterator_members() -> ClassMembers {
    ClassMembers {
        parent: Some("SplFileInfo".to_string()),
        methods: m(&[
            ("current", false),
            ("getATime", false),
            ("getBasename", false),
            ("getCTime", false),
            ("getExtension", false),
            ("getFileInfo", false),
            ("getFilename", false),
            ("getGroup", false),
            ("getInode", false),
            ("getMTime", false),
            ("getOwner", false),
            ("getPath", false),
            ("getPathInfo", false),
            ("getPathname", false),
            ("getPerms", false),
            ("getRealPath", false),
            ("getSize", false),
            ("getType", false),
            ("isDot", false),
            ("isDir", false),
            ("isExecutable", false),
            ("isFile", false),
            ("isLink", false),
            ("isReadable", false),
            ("isValid", false),
            ("isWritable", false),
            ("key", false),
            ("next", false),
            ("rewind", false),
            ("seek", false),
            ("valid", false),
        ]),
        ..Default::default()
    }
}

fn dom_node_members() -> ClassMembers {
    ClassMembers {
        methods: m(&[
            ("appendChild", false),
            ("cloneNode", false),
            ("getNodePath", false),
            ("hasAttributes", false),
            ("hasChildNodes", false),
            ("insertBefore", false),
            ("isDefaultNamespace", false),
            ("isSameNode", false),
            ("lookupNamespaceURI", false),
            ("lookupPrefix", false),
            ("normalize", false),
            ("removeChild", false),
            ("replaceChild", false),
            ("C14N", false),
            ("C14NFile", false),
        ]),
        properties: p(&[
            ("nodeName", false),
            ("nodeValue", false),
            ("nodeType", false),
            ("parentNode", false),
            ("childNodes", false),
            ("firstChild", false),
            ("lastChild", false),
            ("previousSibling", false),
            ("nextSibling", false),
            ("attributes", false),
            ("ownerDocument", false),
            ("namespaceURI", false),
            ("prefix", false),
            ("localName", false),
            ("baseURI", false),
            ("textContent", false),
        ]),
        ..Default::default()
    }
}

fn dom_document_members() -> ClassMembers {
    ClassMembers {
        parent: Some("DOMNode".to_string()),
        methods: m(&[
            ("createElement", false),
            ("createTextNode", false),
            ("createAttribute", false),
            ("createComment", false),
            ("createDocumentFragment", false),
            ("createCDATASection", false),
            ("createProcessingInstruction", false),
            ("createEntityReference", false),
            ("getElementById", false),
            ("getElementsByTagName", false),
            ("getElementsByTagNameNS", false),
            ("importNode", false),
            ("adoptNode", false),
            ("normalizeDocument", false),
            ("load", false),
            ("loadHTML", false),
            ("loadHTMLFile", false),
            ("loadXML", false),
            ("save", false),
            ("saveHTML", false),
            ("saveHTMLFile", false),
            ("saveXML", false),
            ("validate", false),
            ("schemaValidate", false),
            ("schemaValidateSource", false),
            ("relaxNGValidate", false),
            ("relaxNGValidateSource", false),
            ("xinclude", false),
        ]),
        properties: p(&[
            ("doctype", false),
            ("implementation", false),
            ("documentElement", false),
            ("actualEncoding", false),
            ("encoding", false),
            ("xmlEncoding", false),
            ("standalone", false),
            ("xmlStandalone", false),
            ("version", false),
            ("xmlVersion", false),
            ("strictErrorChecking", false),
            ("documentURI", false),
            ("config", false),
            ("formatOutput", false),
            ("validateOnParse", false),
            ("resolveExternals", false),
            ("preserveWhiteSpace", false),
            ("recover", false),
            ("substituteEntities", false),
        ]),
        ..Default::default()
    }
}

fn dom_element_members() -> ClassMembers {
    ClassMembers {
        parent: Some("DOMNode".to_string()),
        methods: m(&[
            ("getAttribute", false),
            ("getAttributeNS", false),
            ("getAttributeNode", false),
            ("getAttributeNodeNS", false),
            ("getElementsByTagName", false),
            ("getElementsByTagNameNS", false),
            ("hasAttribute", false),
            ("hasAttributeNS", false),
            ("removeAttribute", false),
            ("removeAttributeNS", false),
            ("removeAttributeNode", false),
            ("setAttribute", false),
            ("setAttributeNS", false),
            ("setAttributeNode", false),
            ("setAttributeNodeNS", false),
            ("setIdAttribute", false),
            ("setIdAttributeNS", false),
            ("setIdAttributeNode", false),
        ]),
        properties: p(&[
            ("tagName", false),
            ("schemaTypeInfo", false),
        ]),
        ..Default::default()
    }
}

fn simple_xml_element_members() -> ClassMembers {
    ClassMembers {
        methods: m(&[
            ("addChild", false),
            ("addAttribute", false),
            ("asXML", false),
            ("saveXML", false),
            ("attributes", false),
            ("children", false),
            ("count", false),
            ("getName", false),
            ("registerXPathNamespace", false),
            ("xpath", false),
            ("__toString", false),
        ]),
        ..Default::default()
    }
}

fn xml_reader_members() -> ClassMembers {
    ClassMembers {
        methods: m(&[
            ("open", true),
            ("XML", true),
            ("close", false),
            ("read", false),
            ("next", false),
            ("readInnerXml", false),
            ("readOuterXml", false),
            ("readString", false),
            ("expand", false),
            ("getAttribute", false),
            ("getAttributeNo", false),
            ("getAttributeNs", false),
            ("moveToAttribute", false),
            ("moveToAttributeNo", false),
            ("moveToAttributeNs", false),
            ("moveToElement", false),
            ("moveToFirstAttribute", false),
            ("moveToNextAttribute", false),
            ("isValid", false),
            ("lookupNamespace", false),
            ("setRelaxNGSchema", false),
            ("setRelaxNGSchemaSource", false),
            ("setSchema", false),
        ]),
        properties: p(&[
            ("attributeCount", false),
            ("baseURI", false),
            ("depth", false),
            ("hasAttributes", false),
            ("hasValue", false),
            ("isDefault", false),
            ("isEmptyElement", false),
            ("localName", false),
            ("name", false),
            ("namespaceURI", false),
            ("nodeType", false),
            ("prefix", false),
            ("value", false),
            ("xmlLang", false),
        ]),
        ..Default::default()
    }
}

fn xml_writer_members() -> ClassMembers {
    ClassMembers {
        methods: m(&[
            ("openMemory", false),
            ("openUri", false),
            ("outputMemory", false),
            ("flush", false),
            ("setIndent", false),
            ("setIndentString", false),
            ("startDocument", false),
            ("endDocument", false),
            ("startElement", false),
            ("endElement", false),
            ("startElementNs", false),
            ("fullEndElement", false),
            ("startAttribute", false),
            ("endAttribute", false),
            ("startAttributeNs", false),
            ("startComment", false),
            ("endComment", false),
            ("startCdata", false),
            ("endCdata", false),
            ("startDtd", false),
            ("endDtd", false),
            ("startPi", false),
            ("endPi", false),
            ("text", false),
            ("writeRaw", false),
            ("writeAttribute", false),
            ("writeAttributeNs", false),
            ("writeCdata", false),
            ("writeComment", false),
            ("writeDtd", false),
            ("writeDtdAttlist", false),
            ("writeDtdElement", false),
            ("writeDtdEntity", false),
            ("writeElement", false),
            ("writeElementNs", false),
            ("writePi", false),
        ]),
        ..Default::default()
    }
}

fn zip_archive_members() -> ClassMembers {
    ClassMembers {
        methods: m(&[
            ("open", false),
            ("close", false),
            ("addFile", false),
            ("addFromString", false),
            ("addEmptyDir", false),
            ("extractTo", false),
            ("getFromName", false),
            ("getFromIndex", false),
            ("statName", false),
            ("statIndex", false),
            ("deleteIndex", false),
            ("deleteName", false),
            ("locateName", false),
            ("getNameIndex", false),
            ("renameIndex", false),
            ("renameName", false),
            ("getArchiveComment", false),
            ("setArchiveComment", false),
            ("getFileComment", false),
            ("setFileComment", false),
            ("setPassword", false),
            ("getStatusString", false),
            ("unchangeAll", false),
            ("unchangeArchive", false),
            ("unchangeIndex", false),
            ("unchangeName", false),
        ]),
        properties: p(&[
            ("status", false),
            ("statusSys", false),
            ("numFiles", false),
            ("filename", false),
            ("comment", false),
        ]),
        constants: vec![
            "CREATE".into(),
            "OVERWRITE".into(),
            "EXCL".into(),
            "RDONLY".into(),
            "ER_OK".into(),
            "ER_MULTIDISK".into(),
            "ER_RENAME".into(),
            "ER_CLOSE".into(),
            "ER_SEEK".into(),
            "ER_READ".into(),
            "ER_WRITE".into(),
            "ER_CRC".into(),
            "ER_ZIPCLOSED".into(),
            "ER_NOENT".into(),
            "ER_EXISTS".into(),
            "ER_OPEN".into(),
            "ER_TMPOPEN".into(),
            "ER_ZLIB".into(),
            "ER_MEMORY".into(),
            "ER_CHANGED".into(),
            "ER_COMPNOTSUPP".into(),
            "ER_EOF".into(),
            "ER_INVAL".into(),
            "ER_NOZIP".into(),
            "ER_INTERNAL".into(),
            "ER_INCONS".into(),
            "ER_REMOVE".into(),
            "ER_DELETED".into(),
            "FL_NOCASE".into(),
            "FL_NODIR".into(),
            "FL_COMPRESSED".into(),
            "FL_UNCHANGED".into(),
            "FL_OVERWRITE".into(),
            "CM_DEFAULT".into(),
            "CM_STORE".into(),
            "CM_DEFLATE".into(),
            "CM_DEFLATE64".into(),
            "CM_PKWARE_IMPLODE".into(),
            "CM_BZIP2".into(),
            "CM_LZMA".into(),
            "CM_TERSE".into(),
            "CM_LZ77".into(),
            "CM_WAVPACK".into(),
            "CM_PPMD".into(),
        ],
        ..Default::default()
    }
}

fn reflection_class_members() -> ClassMembers {
    ClassMembers {
        methods: m(&[
            ("getAttributes", false),
            ("getConstructor", false),
            ("getDefaultProperties", false),
            ("getDocComment", false),
            ("getEndLine", false),
            ("getExtension", false),
            ("getExtensionName", false),
            ("getFileName", false),
            ("getInterfaceNames", false),
            ("getInterfaces", false),
            ("getMethods", false),
            ("getMethod", false),
            ("getName", false),
            ("getNamespaceName", false),
            ("getParentClass", false),
            ("getProperties", false),
            ("getProperty", false),
            ("getReflectionConstants", false),
            ("getShortName", false),
            ("getStartLine", false),
            ("getStaticProperties", false),
            ("getStaticPropertyValue", false),
            ("getTraitAliases", false),
            ("getTraitNames", false),
            ("getTraits", false),
            ("hasConstant", false),
            ("hasMethod", false),
            ("hasProperty", false),
            ("implementsInterface", false),
            ("inNamespace", false),
            ("isAbstract", false),
            ("isAnonymous", false),
            ("isCloneable", false),
            ("isEnum", false),
            ("isFinal", false),
            ("isInstance", false),
            ("isInstantiable", false),
            ("isInterface", false),
            ("isInternal", false),
            ("isIterable", false),
            ("isReadOnly", false),
            ("isSubclassOf", false),
            ("isTrait", false),
            ("isUserDefined", false),
            ("newInstance", false),
            ("newInstanceArgs", false),
            ("newInstanceWithoutConstructor", false),
            ("setStaticPropertyValue", false),
        ]),
        constants: vec![
            "IS_IMPLICIT_ABSTRACT".into(),
            "IS_EXPLICIT_ABSTRACT".into(),
            "IS_FINAL".into(),
            "IS_READONLY".into(),
        ],
        ..Default::default()
    }
}

fn reflection_method_members() -> ClassMembers {
    ClassMembers {
        methods: m(&[
            ("getAttributes", false),
            ("getClosure", false),
            ("getDeclaringClass", false),
            ("getDocComment", false),
            ("getEndLine", false),
            ("getFileName", false),
            ("getModifiers", false),
            ("getName", false),
            ("getNumberOfParameters", false),
            ("getNumberOfRequiredParameters", false),
            ("getParameters", false),
            ("getReturnType", false),
            ("getShortName", false),
            ("getStartLine", false),
            ("getStaticVariables", false),
            ("hasReturnType", false),
            ("inNamespace", false),
            ("invoke", false),
            ("invokeArgs", false),
            ("isAbstract", false),
            ("isConstructor", false),
            ("isDestructor", false),
            ("isDeprecated", false),
            ("isGenerator", false),
            ("isInternal", false),
            ("isPrivate", false),
            ("isProtected", false),
            ("isPublic", false),
            ("isStatic", false),
            ("isUserDefined", false),
            ("isVariadic", false),
            ("returnsReference", false),
            ("setAccessible", false),
        ]),
        constants: vec![
            "IS_STATIC".into(),
            "IS_ABSTRACT".into(),
            "IS_FINAL".into(),
            "IS_PUBLIC".into(),
            "IS_PROTECTED".into(),
            "IS_PRIVATE".into(),
        ],
        ..Default::default()
    }
}

fn reflection_property_members() -> ClassMembers {
    ClassMembers {
        methods: m(&[
            ("getAttributes", false),
            ("getDeclaringClass", false),
            ("getDefaultValue", false),
            ("getDocComment", false),
            ("getModifiers", false),
            ("getName", false),
            ("getType", false),
            ("getValue", false),
            ("hasDefaultValue", false),
            ("hasType", false),
            ("isDefault", false),
            ("isInitialized", false),
            ("isPrivate", false),
            ("isPromoted", false),
            ("isProtected", false),
            ("isPublic", false),
            ("isReadOnly", false),
            ("isStatic", false),
            ("setValue", false),
            ("setAccessible", false),
        ]),
        constants: vec![
            "IS_STATIC".into(),
            "IS_READONLY".into(),
            "IS_PUBLIC".into(),
            "IS_PROTECTED".into(),
            "IS_PRIVATE".into(),
        ],
        ..Default::default()
    }
}

fn reflection_function_members() -> ClassMembers {
    ClassMembers {
        methods: m(&[
            ("getAttributes", false),
            ("getClosure", false),
            ("getDocComment", false),
            ("getEndLine", false),
            ("getExtension", false),
            ("getExtensionName", false),
            ("getFileName", false),
            ("getName", false),
            ("getNamespaceName", false),
            ("getNumberOfParameters", false),
            ("getNumberOfRequiredParameters", false),
            ("getParameters", false),
            ("getReturnType", false),
            ("getShortName", false),
            ("getStartLine", false),
            ("getStaticVariables", false),
            ("hasReturnType", false),
            ("inNamespace", false),
            ("invoke", false),
            ("invokeArgs", false),
            ("isAnonymous", false),
            ("isClosure", false),
            ("isDeprecated", false),
            ("isGenerator", false),
            ("isInternal", false),
            ("isUserDefined", false),
            ("isVariadic", false),
            ("returnsReference", false),
        ]),
        ..Default::default()
    }
}

fn reflection_parameter_members() -> ClassMembers {
    ClassMembers {
        methods: m(&[
            ("allowsNull", false),
            ("canBePassedByValue", false),
            ("getAttributes", false),
            ("getDeclaringClass", false),
            ("getDeclaringFunction", false),
            ("getDefaultValue", false),
            ("getDefaultValueConstantName", false),
            ("getName", false),
            ("getPosition", false),
            ("getType", false),
            ("hasDefaultValue", false),
            ("hasType", false),
            ("isDefaultValueAvailable", false),
            ("isDefaultValueConstant", false),
            ("isOptional", false),
            ("isPassedByReference", false),
            ("isVariadic", false),
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
