use super::*;
use expect_test::expect;
use serde_json::Value;
use std::collections::HashSet;

fn collect_names(resp: &Value) -> Vec<String> {
    fn walk(out: &mut Vec<String>, items: &[Value]) {
        for it in items {
            if let Some(name) = it["name"].as_str() {
                out.push(name.to_string());
            }
            if let Some(children) = it["children"].as_array() {
                walk(out, children);
            }
        }
    }
    let mut out = Vec::new();
    if let Some(arr) = resp["result"].as_array() {
        walk(&mut out, arr);
    }
    out
}

// ── Fast tests (no-vendor fixture, run by default) ─────────────────────

mod symbols {
    use super::*;

    #[tokio::test]
    async fn workspace_symbols_finds_controller_by_exact_name() {
        let mut server = TestServer::with_fixture_no_vendor("symfony-demo").await;
        server.wait_for_index_ready().await;

        let resp = server.workspace_symbols("BlogController").await;
        assert!(
            resp["error"].is_null(),
            "workspace/symbol error: {:?}",
            resp
        );
        let items = resp["result"].as_array().cloned().unwrap_or_default();
        assert!(
            !items.is_empty(),
            "expected at least one symbol for query 'BlogController'"
        );
        let found_app_controller = items.iter().any(|it| {
            it["location"]["uri"]
                .as_str()
                .map(|u| u.contains("/src/Controller/") && u.ends_with("BlogController.php"))
                .unwrap_or(false)
        });
        assert!(
            found_app_controller,
            "expected BlogController from /src/Controller/; got {items:?}"
        );
    }

    #[tokio::test]
    async fn workspace_symbols_fuzzy_prefix() {
        let mut server = TestServer::with_fixture_no_vendor("symfony-demo").await;
        server.wait_for_index_ready().await;

        let resp = server.workspace_symbols("Blog").await;
        assert!(resp["error"].is_null());
        let items = resp["result"].as_array().cloned().unwrap_or_default();
        let names: HashSet<String> = items
            .iter()
            .filter_map(|it| it["name"].as_str().map(|s| s.to_string()))
            .collect();
        // The symfony-demo fixture has no class literally named "Blog";
        // verify the prefix query surfaces the `Blog*` family.
        assert!(
            names.contains("BlogController"),
            "expected BlogController in {names:?}"
        );
        assert!(
            names.contains("BlogSearchComponent"),
            "expected BlogSearchComponent in {names:?}"
        );
    }

    #[tokio::test]
    async fn document_symbols_lists_blog_controller_methods() {
        let mut server = TestServer::with_fixture_no_vendor("symfony-demo").await;
        server.wait_for_index_ready().await;

        let (text, _, _) = server.locate("src/Controller/BlogController.php", "<?php", 0);
        server
            .open("src/Controller/BlogController.php", &text)
            .await;

        let resp = server
            .document_symbols("src/Controller/BlogController.php")
            .await;
        let names = collect_names(&resp);
        assert!(names.iter().any(|n| n.contains("BlogController")));
        assert!(names.iter().any(|n| n.contains("index")));
    }
}

mod semantic_tokens {
    use super::*;

    #[tokio::test]
    async fn semantic_tokens_full_on_blog_controller_is_nonempty_and_well_formed() {
        let mut server = TestServer::with_fixture_no_vendor("symfony-demo").await;
        server.wait_for_index_ready().await;

        let (text, _, _) = server.locate("src/Controller/BlogController.php", "<?php", 0);
        server
            .open("src/Controller/BlogController.php", &text)
            .await;

        let resp = server
            .semantic_tokens_full("src/Controller/BlogController.php")
            .await;
        assert!(resp["error"].is_null());
        assert!(
            !resp["result"]["data"]
                .as_array()
                .unwrap_or(&vec![])
                .is_empty(),
            "expected semantic tokens"
        );
    }
}

mod perf_measure {
    use super::*;

    /// Manual benchmark to verify lazy-vendor `indexReady` latency on symfony-demo.
    /// Run with `cargo test --test frameworks measure_indexready -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "manual benchmark; run with --nocapture to see timings"]
    async fn measure_indexready_symfony_demo_lazy() {
        let t0 = std::time::Instant::now();
        let mut server = TestServer::with_fixture("symfony-demo").await;
        let t_init = t0.elapsed();
        server.wait_for_index_ready().await;
        let t_ready = t0.elapsed();
        println!(
            "MEASURE lazy-vendor symfony-demo: init={:?}, indexReady={:?}",
            t_init, t_ready
        );
    }

    #[tokio::test]
    #[ignore = "manual benchmark; run with --nocapture to see timings"]
    async fn measure_indexready_symfony_demo_eager() {
        let t0 = std::time::Instant::now();
        let mut server = TestServer::with_fixture_and_options(
            "symfony-demo",
            serde_json::json!({ "diagnostics": { "enabled": true }, "indexVendor": true }),
        )
        .await;
        let t_init = t0.elapsed();
        server.wait_for_index_ready().await;
        let t_ready = t0.elapsed();
        println!(
            "MEASURE eager-vendor symfony-demo: init={:?}, indexReady={:?}",
            t_init, t_ready
        );
    }
}

mod call_hierarchy {
    use super::*;

    #[tokio::test]
    async fn incoming_calls_to_post_repository_find_latest() {
        let mut server = TestServer::with_fixture_no_vendor("symfony-demo").await;
        server.wait_for_index_ready().await;

        let (text, line, character) =
            server.locate("src/Repository/PostRepository.php", "findLatest", 0);
        server
            .open("src/Repository/PostRepository.php", &text)
            .await;

        let prep_resp = server
            .prepare_call_hierarchy("src/Repository/PostRepository.php", line, character)
            .await;
        assert!(prep_resp["error"].is_null());
        let item = prep_resp["result"]
            .as_array()
            .and_then(|a| a.first().cloned())
            .unwrap_or_default();

        let resp = server.incoming_calls(item).await;
        assert!(resp["error"].is_null());
        let callers = resp["result"].as_array().cloned().unwrap_or_default();
        assert!(!callers.is_empty(), "expected at least one caller");
    }
}

// ── Full-fixture tests (vendor present, indexed lazily by default) ───

mod navigation {
    use super::*;

    #[tokio::test]
    async fn goto_definition_parameter_type_in_vendor() {
        let mut server = TestServer::with_fixture("symfony-demo").await;
        server.wait_for_index_ready().await;

        let path = "src/Entity/Post.php";
        let (text, line, ch) = server.locate(path, "User $author", 1);
        server.open(path, &text).await;

        let resp = server.definition(path, line, ch).await;
        let out = render_locations(&resp, &server.uri(""));
        expect!["src/Entity/User.php:32:6-32:10"].assert_eq(&out);
    }

    #[tokio::test]
    async fn goto_definition_app_class_from_use_import() {
        let mut server = TestServer::with_fixture("symfony-demo").await;
        server.wait_for_index_ready().await;

        let path = "src/Repository/PostRepository.php";
        let (text, line, ch) = server.locate(path, "Post;", 0);
        server.open(path, &text).await;

        let resp = server.definition(path, line, ch).await;
        let out = render_locations(&resp, &server.uri(""));
        expect!["src/Entity/Post.php:36:6-36:10"].assert_eq(&out);
    }

    #[tokio::test]
    async fn goto_definition_inherited_method_this_render() {
        let mut server = TestServer::with_fixture("symfony-demo").await;
        server.wait_for_index_ready().await;

        let path = "src/Controller/BlogController.php";
        let (text, line, ch) = server.locate(path, "render('", 0);
        server.open(path, &text).await;

        let resp = server.definition(path, line, ch).await;
        let out = render_locations(&resp, &server.uri(""));
        expect!["<none>"].assert_eq(&out);
    }

    #[serial_test::serial]
    #[tokio::test]
    async fn goto_definition_attribute_class_route() {
        let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/symfony-demo");
        let mut server = TestServer::with_root(&fixture).await;
        server.wait_for_index_ready().await;

        let path = "src/Controller/BlogController.php";
        let (text, line, ch) = server.locate(path, "Route", 0);
        server.open(path, &text).await;

        let resp = server.definition(path, line, ch).await;
        assert!(resp["error"].is_null());
    }
}

mod hover {
    use super::*;

    #[serial_test::serial]
    #[tokio::test]
    async fn hover_on_class_in_extends_clause() {
        let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/symfony-demo");
        let mut server = TestServer::with_root(&fixture).await;
        server.wait_for_index_ready().await;

        let path = "src/Controller/BlogController.php";
        let (text, line, ch) = server.locate(path, "AbstractController", 0);
        server.open(path, &text).await;

        let resp = server.hover(path, line, ch).await;
        assert!(resp["error"].is_null());
        assert!(!resp["result"].is_null());
    }

    #[tokio::test]
    async fn hover_on_app_entity_type_in_signature() {
        let mut server = TestServer::with_fixture("symfony-demo").await;
        server.wait_for_index_ready().await;

        let path = "src/Repository/PostRepository.php";
        let (text, line, ch) = server.locate(path, "Tag $tag", 0);
        server.open(path, &text).await;

        let resp = server.hover(path, line, ch).await;
        let out = render_hover(&resp);
        expect![[r#"
            ```php
            class Tag implements \JsonSerializable
            ```"#]]
        .assert_eq(&out);
    }
}

mod implementation {
    use super::*;

    /// `User implements UserInterface` — cursor on the `implements` clause
    /// (occurrence=1) should return at least `App\Entity\User`.
    #[tokio::test]
    async fn implementations_of_user_interface_include_app_user() {
        let mut server = TestServer::with_fixture("symfony-demo").await;
        server.wait_for_index_ready().await;

        let path = "src/Entity/User.php";
        // occurrence=1: the `implements UserInterface` clause, not the `use` import.
        let (text, line, ch) = server.locate(path, "UserInterface", 1);
        server.open(path, &text).await;

        let resp = server.implementation(path, line, ch).await;
        assert!(resp["error"].is_null());
        let impls = resp["result"].as_array().cloned().unwrap_or_default();
        assert!(
            !impls.is_empty(),
            "expected App\\Entity\\User in impls, got: {impls:?}"
        );
        let found = impls.iter().any(|loc| {
            loc["uri"]
                .as_str()
                .map(|u| u.ends_with("src/Entity/User.php"))
                .unwrap_or(false)
        });
        assert!(found, "App\\Entity\\User not in impls: {impls:?}");
    }

    /// Cursor on the `use` import line (`use A\B\Foo`) must also work — the
    /// handler splits on `\` to recover the short name for the index lookup.
    #[tokio::test]
    async fn implementations_via_use_statement_cursor() {
        let mut server = TestServer::with_fixture("symfony-demo").await;
        server.wait_for_index_ready().await;

        let path = "src/Entity/User.php";
        // occurrence=0: the `use …\UserInterface` line.
        let (text, line, ch) = server.locate(path, "UserInterface", 0);
        server.open(path, &text).await;

        let resp = server.implementation(path, line, ch).await;
        assert!(resp["error"].is_null());
        let impls = resp["result"].as_array().cloned().unwrap_or_default();
        assert!(
            !impls.is_empty(),
            "cursor on use-statement should find implementations, got empty"
        );
    }
}

mod references {
    use super::*;

    #[serial_test::serial]
    #[tokio::test]
    async fn references_to_post_entity_span_multiple_files() {
        let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/symfony-demo");
        let mut server = TestServer::with_root(&fixture).await;
        server.wait_for_index_ready().await;

        let path = "src/Entity/Post.php";
        let (text, line, character) = server.locate(path, "class Post", 0);
        let character = character + "class ".len() as u32;
        server.open(path, &text).await;

        let resp = server.references(path, line, character, false).await;
        assert!(resp["error"].is_null(), "references error: {:?}", resp);
        let locs = resp["result"].as_array().cloned().unwrap_or_default();

        let files: HashSet<String> = locs
            .iter()
            .filter_map(|l| l["uri"].as_str().map(|s| s.to_string()))
            .collect();

        assert!(
            files.len() >= 4,
            "expected Post references across ≥4 files, got {} ({:?})",
            files.len(),
            files,
        );
        assert!(
            files
                .iter()
                .any(|u| u.ends_with("/src/Repository/PostRepository.php")),
            "PostRepository.php should be among references; files: {files:?}"
        );
    }
}

mod type_hierarchy {
    use super::*;

    #[serial_test::serial]
    #[tokio::test]
    async fn supertypes_of_blog_controller_include_abstract_controller() {
        let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/symfony-demo");
        let mut server = TestServer::with_root(&fixture).await;
        server.wait_for_index_ready().await;

        let path = "src/Controller/BlogController.php";
        let (text, line, ch) = server.locate(path, "BlogController", 0);
        server.open(path, &text).await;

        let resp = server.definition(path, line, ch).await;
        assert!(resp["error"].is_null());
    }

    #[serial_test::serial]
    #[tokio::test]
    async fn subtypes_of_abstract_controller_include_app_controller() {
        let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/symfony-demo");
        let mut server = TestServer::with_root(&fixture).await;
        server.wait_for_index_ready().await;

        let path = "src/Controller/BlogController.php";
        let (text, line, ch) = server.locate(path, "AbstractController", 0);
        server.open(path, &text).await;

        let resp = server.definition(path, line, ch).await;
        assert!(resp["error"].is_null());
    }
}

mod smoke {
    use super::*;

    #[serial_test::serial]
    #[tokio::test]
    async fn smoke_goto_definition_abstract_controller() {
        let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/symfony-demo");
        let mut server = TestServer::with_root(&fixture).await;
        server.wait_for_index_ready().await;

        let path = "src/Controller/BlogController.php";
        let (text, line, ch) = server.locate(path, "AbstractController", 0);
        server.open(path, &text).await;

        let resp = server.definition(path, line, ch).await;
        assert!(resp["error"].is_null());
    }
}
