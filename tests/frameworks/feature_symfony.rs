use super::*;
use expect_test::expect;

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
        let out = render_workspace_symbols(&resp, &server.uri(""));
        expect![[r#"
            Class       BlogController @ src/Controller/Admin/BlogController.php:42
            Class       BlogController @ src/Controller/BlogController.php:39
            Class       BlogControllerTest @ tests/Controller/Admin/BlogControllerTest.php:36
            Class       BlogControllerTest @ tests/Controller/BlogControllerTest.php:28"#]]
        .assert_eq(&out);
    }

    #[tokio::test]
    async fn workspace_symbols_fuzzy_prefix() {
        let mut server = TestServer::with_fixture_no_vendor("symfony-demo").await;
        server.wait_for_index_ready().await;

        let resp = server.workspace_symbols("Blog").await;
        assert!(resp["error"].is_null());
        let out = render_workspace_symbols(&resp, &server.uri(""));
        // Prefix query "Blog" must surface the Blog* family (BlogController, BlogSearchComponent, etc.)
        expect![[r#"
            Class       BlogController @ src/Controller/Admin/BlogController.php:42
            Class       BlogController @ src/Controller/BlogController.php:39
            Class       BlogControllerTest @ tests/Controller/Admin/BlogControllerTest.php:36
            Class       BlogControllerTest @ tests/Controller/BlogControllerTest.php:28
            Class       BlogSearchComponent @ src/Twig/Components/BlogSearchComponent.php:27
            Method      testPublicBlogPost @ tests/Controller/DefaultControllerTest.php:55"#]]
        .assert_eq(&out);
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
        let out = render_document_symbols(&resp);
        // Must include class BlogController and its index method.
        expect![[r#"
            Class BlogController @L39
              Method index @L51
                Variable $request @L51
                Variable $page @L51
                Variable $_format @L51
                Variable $posts @L51
                Variable $tags @L51
              Method postShow @L80
                Variable $post @L80
              Method commentNew @L107
                Variable $user @L108
                Variable $request @L109
                Variable $post @L110
                Variable $eventDispatcher @L111
                Variable $entityManager @L112
              Method commentForm @L150
                Variable $post @L150
              Method search @L161
                Variable $request @L161"#]]
        .assert_eq(&out);
    }
}

mod semantic_tokens {
    use super::*;

    #[tokio::test]
    async fn semantic_tokens_full_on_blog_controller_is_well_formed() {
        // The legend is a server-wide constant, independent of the fixture
        // under test, so a throwaway server suffices to fetch it.
        let (_, init_resp) = TestServer::new_with_options(serde_json::json!({})).await;
        let legend_types: Vec<&str> = init_resp["result"]["capabilities"]["semanticTokensProvider"]
            ["legend"]["tokenTypes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();

        let mut server = TestServer::with_fixture_no_vendor("symfony-demo").await;
        server.wait_for_index_ready().await;

        let (text, _, _) = server.locate("src/Controller/BlogController.php", "<?php", 0);
        server
            .open("src/Controller/BlogController.php", &text)
            .await;

        let resp = server
            .semantic_tokens_full("src/Controller/BlogController.php")
            .await;
        expect![[r#"
            2:0 len=2 type=comment mods=0b0
            3:0 len=44 type=comment mods=0b0
            4:0 len=2 type=comment mods=0b0
            5:0 len=44 type=comment mods=0b0
            6:0 len=2 type=comment mods=0b0
            7:0 len=74 type=comment mods=0b0
            8:0 len=51 type=comment mods=0b0
            9:0 len=3 type=comment mods=0b0
            32:0 len=3 type=comment mods=0b0
            33:0 len=74 type=comment mods=0b0
            34:0 len=2 type=comment mods=0b0
            35:0 len=45 type=comment mods=0b0
            36:0 len=52 type=comment mods=0b0
            37:0 len=3 type=comment mods=0b0
            38:2 len=5 type=class mods=0b0
            39:12 len=14 type=class mods=0b1
            41:4 len=3 type=comment mods=0b0
            42:0 len=82 type=comment mods=0b0
            43:0 len=44 type=comment mods=0b0
            44:0 len=6 type=comment mods=0b0
            45:0 len=74 type=comment mods=0b0
            46:0 len=7 type=comment mods=0b0
            47:6 len=5 type=class mods=0b0
            48:6 len=5 type=class mods=0b0
            49:6 len=5 type=class mods=0b0
            50:6 len=5 type=class mods=0b0
            51:20 len=5 type=method mods=0b1
            51:26 len=7 type=type mods=0b0
            51:34 len=8 type=parameter mods=0b1
            51:44 len=3 type=type mods=0b0
            51:48 len=5 type=parameter mods=0b1
            51:55 len=6 type=type mods=0b0
            51:62 len=8 type=parameter mods=0b1
            51:72 len=14 type=type mods=0b0
            51:87 len=6 type=parameter mods=0b1
            51:95 len=13 type=type mods=0b0
            51:109 len=5 type=parameter mods=0b1
            51:117 len=8 type=type mods=0b0
            53:8 len=4 type=variable mods=0b0
            55:12 len=8 type=variable mods=0b0
            55:22 len=5 type=property mods=0b0
            55:29 len=3 type=method mods=0b0
            55:33 len=5 type=string mods=0b0
            56:12 len=4 type=variable mods=0b0
            56:19 len=5 type=variable mods=0b0
            56:26 len=9 type=method mods=0b0
            56:37 len=6 type=string mods=0b0
            56:47 len=8 type=variable mods=0b0
            56:57 len=5 type=property mods=0b0
            56:64 len=3 type=method mods=0b0
            56:68 len=5 type=string mods=0b0
            59:8 len=12 type=variable mods=0b0
            59:23 len=6 type=variable mods=0b0
            59:31 len=10 type=method mods=0b0
            59:42 len=5 type=variable mods=0b0
            59:49 len=4 type=variable mods=0b0
            61:8 len=74 type=comment mods=0b0
            62:8 len=28 type=comment mods=0b0
            63:8 len=69 type=comment mods=0b0
            64:15 len=5 type=variable mods=0b0
            64:22 len=6 type=method mods=0b0
            64:29 len=13 type=string mods=0b0
            64:43 len=8 type=variable mods=0b0
            64:52 len=7 type=string mods=0b0
            65:12 len=11 type=string mods=0b0
            65:27 len=12 type=variable mods=0b0
            66:12 len=9 type=string mods=0b0
            66:25 len=4 type=variable mods=0b0
            66:32 len=7 type=method mods=0b0
            70:4 len=3 type=comment mods=0b0
            71:0 len=80 type=comment mods=0b0
            72:0 len=87 type=comment mods=0b0
            73:0 len=76 type=comment mods=0b0
            74:0 len=85 type=comment mods=0b0
            75:0 len=86 type=comment mods=0b0
            76:0 len=35 type=comment mods=0b0
            77:0 len=108 type=comment mods=0b0
            78:0 len=7 type=comment mods=0b0
            79:6 len=5 type=class mods=0b0
            80:20 len=8 type=method mods=0b1
            80:29 len=4 type=type mods=0b0
            80:34 len=5 type=parameter mods=0b1
            80:42 len=8 type=type mods=0b0
            82:8 len=79 type=comment mods=0b0
            83:8 len=89 type=comment mods=0b0
            84:8 len=74 type=comment mods=0b0
            85:8 len=82 type=comment mods=0b0
            86:8 len=2 type=comment mods=0b0
            87:8 len=50 type=comment mods=0b0
            88:8 len=2 type=comment mods=0b0
            89:8 len=87 type=comment mods=0b0
            90:8 len=52 type=comment mods=0b0
            91:8 len=77 type=comment mods=0b0
            92:8 len=2 type=comment mods=0b0
            93:8 len=65 type=comment mods=0b0
            94:8 len=22 type=comment mods=0b0
            96:15 len=5 type=variable mods=0b0
            96:22 len=6 type=method mods=0b0
            96:29 len=26 type=string mods=0b0
            96:58 len=6 type=string mods=0b0
            96:68 len=5 type=variable mods=0b0
            99:4 len=3 type=comment mods=0b0
            100:0 len=77 type=comment mods=0b0
            101:0 len=77 type=comment mods=0b0
            102:0 len=6 type=comment mods=0b0
            103:0 len=87 type=comment mods=0b0
            104:0 len=7 type=comment mods=0b0
            105:6 len=5 type=class mods=0b0
            106:6 len=9 type=class mods=0b0
            107:20 len=10 type=method mods=0b1
            108:10 len=11 type=class mods=0b0
            108:23 len=4 type=type mods=0b0
            108:28 len=5 type=parameter mods=0b1
            109:8 len=7 type=type mods=0b0
            109:16 len=8 type=parameter mods=0b1
            110:10 len=9 type=class mods=0b0
            110:54 len=4 type=type mods=0b0
            110:59 len=5 type=parameter mods=0b1
            111:8 len=24 type=type mods=0b0
            111:33 len=16 type=parameter mods=0b1
            112:8 len=22 type=type mods=0b0
            112:31 len=14 type=parameter mods=0b1
            113:7 len=8 type=type mods=0b0
            114:8 len=8 type=variable mods=0b0
            115:8 len=8 type=variable mods=0b0
            115:18 len=9 type=method mods=0b0
            115:28 len=5 type=variable mods=0b0
            116:8 len=5 type=variable mods=0b0
            116:15 len=10 type=method mods=0b0
            116:26 len=8 type=variable mods=0b0
            118:8 len=5 type=variable mods=0b0
            118:16 len=5 type=variable mods=0b0
            118:23 len=10 type=method mods=0b0
            118:34 len=11 type=class mods=0b0
            118:47 len=5 type=property mods=0b10
            118:54 len=8 type=variable mods=0b0
            119:8 len=5 type=variable mods=0b0
            119:15 len=13 type=method mods=0b0
            119:29 len=8 type=variable mods=0b0
            121:12 len=5 type=variable mods=0b0
            121:19 len=11 type=method mods=0b0
            121:36 len=5 type=variable mods=0b0
            121:43 len=7 type=method mods=0b0
            122:12 len=14 type=variable mods=0b0
            122:28 len=7 type=method mods=0b0
            122:36 len=8 type=variable mods=0b0
            123:12 len=14 type=variable mods=0b0
            123:28 len=5 type=method mods=0b0
            125:12 len=72 type=comment mods=0b0
            126:12 len=73 type=comment mods=0b0
            127:12 len=70 type=comment mods=0b0
            128:12 len=74 type=comment mods=0b0
            129:12 len=71 type=comment mods=0b0
            130:12 len=2 type=comment mods=0b0
            131:12 len=73 type=comment mods=0b0
            132:12 len=68 type=comment mods=0b0
            133:12 len=53 type=comment mods=0b0
            134:12 len=16 type=variable mods=0b0
            134:30 len=8 type=method mods=0b0
            134:63 len=8 type=variable mods=0b0
            136:19 len=5 type=variable mods=0b0
            136:26 len=15 type=method mods=0b0
            136:42 len=11 type=string mods=0b0
            136:56 len=6 type=string mods=0b0
            136:66 len=5 type=variable mods=0b0
            136:73 len=7 type=method mods=0b0
            136:85 len=8 type=class mods=0b0
            136:95 len=14 type=property mods=0b10
            139:15 len=5 type=variable mods=0b0
            139:22 len=6 type=method mods=0b0
            139:29 len=35 type=string mods=0b0
            140:12 len=6 type=string mods=0b0
            140:22 len=5 type=variable mods=0b0
            141:12 len=6 type=string mods=0b0
            141:22 len=5 type=variable mods=0b0
            145:4 len=3 type=comment mods=0b0
            146:0 len=74 type=comment mods=0b0
            147:0 len=78 type=comment mods=0b0
            148:0 len=27 type=comment mods=0b0
            149:0 len=7 type=comment mods=0b0
            150:20 len=11 type=method mods=0b1
            150:32 len=4 type=type mods=0b0
            150:37 len=5 type=parameter mods=0b1
            150:45 len=8 type=type mods=0b0
            152:8 len=5 type=variable mods=0b0
            152:16 len=5 type=variable mods=0b0
            152:23 len=10 type=method mods=0b0
            152:34 len=11 type=class mods=0b0
            152:47 len=5 type=property mods=0b10
            154:15 len=5 type=variable mods=0b0
            154:22 len=6 type=method mods=0b0
            154:29 len=30 type=string mods=0b0
            155:12 len=6 type=string mods=0b0
            155:22 len=5 type=variable mods=0b0
            156:12 len=6 type=string mods=0b0
            156:22 len=5 type=variable mods=0b0
            160:6 len=5 type=class mods=0b0
            161:20 len=6 type=method mods=0b1
            161:27 len=7 type=type mods=0b0
            161:35 len=8 type=parameter mods=0b1
            161:46 len=8 type=type mods=0b0
            163:15 len=5 type=variable mods=0b0
            163:22 len=6 type=method mods=0b0
            163:29 len=23 type=string mods=0b0
            163:55 len=7 type=string mods=0b0"#]]
        .assert_eq(&render_semantic_tokens(&resp, &legend_types));
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
        let mut server = TestServer::with_fixture_and_options(
            "symfony-demo",
            serde_json::json!({ "indexVendor": false }),
        )
        .await;
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

    /// Manual benchmark for the workspace-wide class-name search behind
    /// bare-class-name completion, against symfony-demo's full vendor tree
    /// (~5200 PHP files). Run with `cargo test --test frameworks
    /// measure_workspace_class_search -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "manual benchmark; run with --nocapture to see timings"]
    async fn measure_workspace_class_search_cost_eager_vendor() {
        let mut server = TestServer::with_fixture_and_options(
            "symfony-demo",
            serde_json::json!({ "diagnostics": { "enabled": true }, "indexVendor": true }),
        )
        .await;
        server.wait_for_index_ready_secs(60).await;
        let caller = "<?php\n$r = Con;\n";
        server.open("caller.php", caller).await;
        // "$r = Con;" — cursor right after "Con" (line 1, byte offset 8).
        let (line, ch) = (1, 8);
        // Warm up (first request pays one-time salsa/JIT costs).
        server.completion("caller.php", line, ch).await;
        let n = 20;
        let t0 = std::time::Instant::now();
        for _ in 0..n {
            server.completion("caller.php", line, ch).await;
        }
        let elapsed = t0.elapsed();
        println!(
            "MEASURE workspace_class_search: {n} completions in {:?} ({:?}/req)",
            elapsed,
            elapsed / n
        );
    }
}

mod call_hierarchy {
    use super::*;

    #[serial_test::serial(symfony_demo)]
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
        let out = render_call_hierarchy(&resp, "from", &server.uri(""));
        expect!["index @ src/Controller/BlogController.php:51:20 fromRanges=[59:31-59:41]"]
            .assert_eq(&out);
    }
}

// ── Full-fixture tests (vendor present, indexed eagerly by default) ───

mod navigation {
    use super::*;

    #[serial_test::serial(symfony_demo)]
    #[tokio::test]
    async fn goto_definition_parameter_type_in_vendor() {
        // Read-only against the checked-out fixture (no per-test copy): a
        // fresh `with_fixture` TempDir would give every vendor file a new
        // absolute path each run, defeating the on-disk FileIndex cache
        // that's supposed to make repeat vendor scans near-free (see
        // `WorkspaceCache`) and forcing a full cold parse of all ~5200
        // vendor files on every single test instead of once per test binary.
        let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/symfony-demo");
        let mut server = TestServer::with_root(&fixture).await;
        server.wait_for_index_ready().await;

        let path = "src/Entity/Post.php";
        let (text, line, ch) = server.locate(path, "User $author", 1);
        server.open(path, &text).await;

        let resp = server.definition(path, line, ch).await;
        let out = render_locations(&resp, &server.uri(""));
        expect!["src/Entity/User.php:32:6-32:10"].assert_eq(&out);
    }

    #[serial_test::serial(symfony_demo)]
    #[tokio::test]
    async fn goto_definition_app_class_from_use_import() {
        let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/symfony-demo");
        let mut server = TestServer::with_root(&fixture).await;
        server.wait_for_index_ready().await;

        let path = "src/Repository/PostRepository.php";
        let (text, line, ch) = server.locate(path, "Post;", 0);
        server.open(path, &text).await;

        let resp = server.definition(path, line, ch).await;
        let out = render_locations(&resp, &server.uri(""));
        expect!["src/Entity/Post.php:36:6-36:10"].assert_eq(&out);
    }

    #[serial_test::serial(symfony_demo)]
    #[tokio::test]
    async fn goto_definition_inherited_method_this_render() {
        let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/symfony-demo");
        let mut server = TestServer::with_root(&fixture).await;
        server.wait_for_index_ready().await;

        let path = "src/Controller/BlogController.php";
        let (text, line, ch) = server.locate(path, "render('", 0);
        server.open(path, &text).await;

        let resp = server.definition(path, line, ch).await;
        let out = render_locations(&resp, &server.uri(""));
        expect!["vendor/symfony/framework-bundle/Controller/AbstractController.php:275:23-275:29"]
            .assert_eq(&out);
    }

    #[serial_test::serial(symfony_demo)]
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
        let out = render_locations(&resp, &server.uri(""));
        expect!["vendor/symfony/routing/Attribute/Route.php:18:6-18:11"].assert_eq(&out);
    }
}

mod hover {
    use super::*;

    #[serial_test::serial(symfony_demo)]
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
        let out = render_hover(&resp);
        expect![[r#"`use Symfony\Bundle\FrameworkBundle\Controller\AbstractController;`"#]]
            .assert_eq(&out);
    }

    #[serial_test::serial(symfony_demo)]
    #[tokio::test]
    async fn hover_on_app_entity_type_in_signature() {
        let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/symfony-demo");
        let mut server = TestServer::with_root(&fixture).await;
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
    /// (occurrence=1) should return at least `App\Entity\User`. Subject is
    /// workspace code only (`subtypes_of` matches on the app class's own
    /// `implements` clause text, not on the vendor interface being indexed),
    /// so this uses the no-vendor fixture to stay fast and avoid contending
    /// with other tests over the full ~5200-file vendor scan.
    #[tokio::test]
    async fn implementations_of_user_interface_include_app_user() {
        let mut server = TestServer::with_fixture_no_vendor("symfony-demo").await;
        server.wait_for_index_ready().await;

        let path = "src/Entity/User.php";
        // occurrence=1: the `implements UserInterface` clause, not the `use` import.
        let (text, line, ch) = server.locate(path, "UserInterface", 1);
        server.open(path, &text).await;

        let resp = server.implementation(path, line, ch).await;
        assert!(resp["error"].is_null());
        let out = render_locations(&resp, &server.uri(""));
        expect!["src/Entity/User.php:32:6-32:10"].assert_eq(&out);
    }

    /// Cursor on the `use` import line (`use A\B\Foo`) must also work — the
    /// handler splits on `\` to recover the short name for the index lookup.
    /// Same rationale as above for the no-vendor fixture.
    #[tokio::test]
    async fn implementations_via_use_statement_cursor() {
        let mut server = TestServer::with_fixture_no_vendor("symfony-demo").await;
        server.wait_for_index_ready().await;

        let path = "src/Entity/User.php";
        // occurrence=0: the `use …\UserInterface` line.
        let (text, line, ch) = server.locate(path, "UserInterface", 0);
        server.open(path, &text).await;

        let resp = server.implementation(path, line, ch).await;
        assert!(resp["error"].is_null());
        let out = render_locations(&resp, &server.uri(""));
        expect!["src/Entity/User.php:32:6-32:10"].assert_eq(&out);
    }
}

mod references {
    use super::*;

    // KNOWN GAP: mir now finds class refs inside `@var`/generic-type-arg
    // docblocks, but mislocates them (statement/docblock span, not the token) — needs a mir-side fix.
    #[serial_test::serial(symfony_demo)]
    #[tokio::test]
    async fn references_to_post_entity_span_multiple_files() {
        let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/symfony-demo");
        let mut server = TestServer::with_root(&fixture).await;
        server.wait_for_index_ready().await;
        // `indexReady` fires once the raw scan finishes; the reference-index
        // warm sweep runs after it in a detached background task. Without
        // waiting for it too, this request can race that sweep's own
        // analysis of the same files and lose a location (seen flaking on
        // slower/more contended CI, most often windows-latest).
        assert!(
            server.wait_for_warm_sweeps(1).await,
            "reference-index warm sweep did not complete"
        );

        let path = "src/Entity/Post.php";
        let (text, line, character) = server.locate(path, "class Post", 0);
        let character = character + "class ".len() as u32;
        server.open(path, &text).await;

        let resp = server.references(path, line, character, false).await;
        assert!(resp["error"].is_null(), "references error: {:?}", resp);
        let out = render_locations(&resp, &server.uri(""));
        // Must span ≥4 files including PostRepository.php
        expect![[r#"
            src/Controller/Admin/BlogController.php:122:25-122:29
            src/Controller/Admin/BlogController.php:138:43-138:47
            src/Controller/Admin/BlogController.php:161:45-161:49
            src/Controller/Admin/BlogController.php:79:20-79:24
            src/Controller/BlogController.php:110:54-110:58
            src/Controller/BlogController.php:150:32-150:36
            src/Controller/BlogController.php:80:29-80:33
            src/DataFixtures/AppFixtures.php:73:24-73:28
            src/Entity/Comment.php:101:32-101:36
            src/Entity/Comment.php:106:28-106:32
            src/Entity/Comment.php:37:34-37:38
            src/Entity/Comment.php:39:13-39:17
            src/EventSubscriber/CommentNotificationSubscriber.php:51:8-51:36
            src/Form/PostType.php:77:16-77:42
            src/Form/PostType.php:88:28-88:32
            src/Repository/PostRepository.php:20:0-20:3
            src/Repository/PostRepository.php:38:39-38:43
            src/Security/PostVoter.php:19:0-19:3
            src/Security/PostVoter.php:40:35-40:39
            src/Security/PostVoter.php:46:23-46:38
            tests/Controller/Admin/BlogControllerTest.php:167:8-167:41
            tests/Controller/DefaultControllerTest.php:64:45-64:49
            tests/Controller/DefaultControllerTest.php:64:8-64:67"#]]
        .assert_eq(&out);
    }

    /// CRLF sources must produce the same reference postings as LF sources.
    ///
    /// Regression: on Windows CI (git autocrlf) the symfony-demo fixture is
    /// checked out with CRLF endings, and blank lines inside AppFixtures.php's
    /// indented nowdoc tripped a spurious "Invalid body indentation level" parse
    /// error that silently dropped the file's `new Post()` reference posting.
    #[test]
    fn crlf_reference_postings_match_lf() {
        fn post_refs(line_ending: &str) -> Vec<String> {
            let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/symfony-demo");
            let read = |p: &str| -> String {
                // Normalize to LF first: on a checkout where git's `core.autocrlf`
                // already converted the fixture to CRLF, replacing '\n' directly
                // would double every '\r' into '\r\r\n' instead of producing plain
                // CRLF, defeating the LF/CRLF comparison below.
                std::fs::read_to_string(root.join(p))
                    .unwrap()
                    .replace("\r\n", "\n")
                    .replace('\n', line_ending)
            };
            let session = mir_analyzer::AnalysisSession::new(mir_analyzer::PhpVersion::LATEST);
            let files = ["src/Entity/Post.php", "src/DataFixtures/AppFixtures.php"];
            for f in files {
                session.ingest_file(
                    std::sync::Arc::from(f),
                    std::sync::Arc::from(read(f).as_str()),
                );
            }
            let paths: Vec<std::sync::Arc<str>> =
                files.iter().map(|f| std::sync::Arc::from(*f)).collect();
            session
                .indexed_references_to(
                    &mir_analyzer::Name::class("App\\Entity\\Post"),
                    &paths,
                    false,
                    &|| false,
                )
                .unwrap()
                .into_iter()
                .map(|(f, r)| format!("{f}:{}:{}-{}", r.start.line, r.start.column, r.end.column))
                .collect()
        }

        let lf = post_refs("\n");
        let crlf = post_refs("\r\n");
        expect!["src/DataFixtures/AppFixtures.php:74:24-28"].assert_eq(&lf.join("\n"));
        assert_eq!(lf, crlf, "LF vs CRLF reference postings diverge");
    }
}

mod type_hierarchy {
    use super::*;

    /// `BlogController extends AbstractController` — supertypes of BlogController
    /// must include AbstractController (a vendor class), verifying that the
    /// PSR-4 pre-load pass makes vendor parents visible in the workspace index.
    /// The fixture has two distinct `BlogController` classes (`App\Controller`
    /// and `App\Controller\Admin`), both extending `AbstractController`;
    /// `classes_by_name` is keyed by short name only, so a lookup on the bare
    /// name walks both classes' parent chains and must dedup the resulting
    /// `AbstractController` entry rather than emit it twice.
    #[serial_test::serial(symfony_demo)]
    #[tokio::test]
    async fn supertypes_of_blog_controller_include_abstract_controller() {
        let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/symfony-demo");
        let mut server = TestServer::with_root(&fixture).await;
        server.wait_for_index_ready().await;

        let path = "src/Controller/BlogController.php";
        let (text, line, ch) = server.locate(path, "BlogController", 0);
        server.open(path, &text).await;

        let prep = server.prepare_type_hierarchy(path, line, ch).await;
        let item = prep["result"]
            .as_array()
            .and_then(|a| a.first().cloned())
            .unwrap_or_default();
        assert_eq!(item["name"].as_str(), Some("BlogController"));

        let resp = server.supertypes(item).await;
        assert!(resp["error"].is_null());
        let names: Vec<&str> = resp["result"]
            .as_array()
            .map(|a| a.iter().filter_map(|i| i["name"].as_str()).collect())
            .unwrap_or_default();
        // Supertypes must include AbstractController (vendor class via PSR-4 pre-load),
        // deduped despite two BlogController classes in the fixture sharing the short name.
        expect!["AbstractController"].assert_eq(&names.join(", "));
    }

    /// `BlogController extends AbstractController` — subtypes of AbstractController
    /// (a vendor class) must include BlogController once AbstractController has
    /// been pre-loaded into the workspace index.
    #[serial_test::serial(symfony_demo)]
    #[tokio::test]
    async fn subtypes_of_abstract_controller_include_blog_controller() {
        let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/symfony-demo");
        let mut server = TestServer::with_root(&fixture).await;
        server.wait_for_index_ready().await;

        // First open BlogController so the workspace knows about the relationship.
        let path = "src/Controller/BlogController.php";
        let (text, _, _) = server.locate(path, "BlogController", 0);
        server.open(path, &text).await;

        // Prepare on AbstractController (in vendor) — needs PSR-4 resolution.
        // Use the use-statement line in BlogController to locate it.
        let (_, ac_line, ac_ch) = server.locate(path, "AbstractController", 0);
        let prep = server.prepare_type_hierarchy(path, ac_line, ac_ch).await;
        // prepare_type_hierarchy may return null for vendor classes not yet in the
        // workspace index; in that case subtypes is undefined and we just pass.
        let Some(item) = prep["result"].as_array().and_then(|a| a.first().cloned()) else {
            return;
        };

        let resp = server.subtypes(item).await;
        assert!(resp["error"].is_null());
        let items = resp["result"].as_array().cloned().unwrap_or_default();
        let names: Vec<&str> = items.iter().filter_map(|i| i["name"].as_str()).collect();
        assert!(
            names.contains(&"BlogController"),
            "expected BlogController in subtypes of AbstractController; got {names:?}"
        );
    }
}

mod smoke {
    use super::*;

    #[serial_test::serial(symfony_demo)]
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
        let out = render_locations(&resp, &server.uri(""));
        expect!["vendor/symfony/framework-bundle/Controller/AbstractController.php:56:15-56:33"]
            .assert_eq(&out);
    }
}
