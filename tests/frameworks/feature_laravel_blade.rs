//! Protocol-wired tests for Blade template (`.blade.php`) support — hover,
//! go-to-definition, completion and document links for Laravel string-key
//! helpers inside `{{ }}` expressions, view/Livewire-referencing directives
//! (`@include`, `@livewire`), and Blade/Livewire component tags
//! (`<x-alert>`, `<livewire:counter>`), against a synthetic minimal Laravel
//! project.

use super::*;

use expect_test::expect;
use serde_json::Value;

fn write_full_laravel_project(root: &std::path::Path) {
    std::fs::write(root.join("artisan"), "#!/usr/bin/env php\n").unwrap();
    std::fs::create_dir_all(root.join("routes")).unwrap();
    std::fs::write(
        root.join("routes").join("web.php"),
        "<?php\nRoute::get('/', HomeController::class)->name('home');\n",
    )
    .unwrap();
    let views = root.join("resources").join("views");
    std::fs::create_dir_all(views.join("layouts")).unwrap();
    std::fs::create_dir_all(views.join("components")).unwrap();
    std::fs::write(views.join("welcome.blade.php"), "<h1>Welcome</h1>\n").unwrap();
    std::fs::write(
        views.join("layouts").join("app.blade.php"),
        "<html></html>\n",
    )
    .unwrap();
    std::fs::write(
        views.join("components").join("alert.blade.php"),
        "<div class=\"alert\"></div>\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("app").join("View").join("Components")).unwrap();
    std::fs::write(
        root.join("app")
            .join("View")
            .join("Components")
            .join("Badge.php"),
        "<?php\nnamespace App\\View\\Components;\nclass Badge extends \\Illuminate\\View\\Component {}\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("app").join("Livewire")).unwrap();
    std::fs::write(
        root.join("app").join("Livewire").join("Counter.php"),
        "<?php\nnamespace App\\Livewire;\nclass Counter extends \\Livewire\\Component {}\n",
    )
    .unwrap();
}

fn render_document_links(result: &Value, root_uri: &str) -> String {
    let links = result.as_array().cloned().unwrap_or_default();
    if links.is_empty() {
        return "<no links>".to_owned();
    }
    let prefix = if root_uri.ends_with('/') {
        root_uri.to_owned()
    } else {
        format!("{root_uri}/")
    };
    let mut rows: Vec<String> = links
        .iter()
        .map(|l| {
            let sl = l["range"]["start"]["line"].as_u64().unwrap_or(0);
            let sc = l["range"]["start"]["character"].as_u64().unwrap_or(0);
            let ec = l["range"]["end"]["character"].as_u64().unwrap_or(0);
            let target = l["target"].as_str().unwrap_or("<no target>");
            let target = target.strip_prefix(&prefix).unwrap_or(target);
            format!("{sl}:{sc}-{ec} target={target}")
        })
        .collect();
    rows.sort();
    rows.join("\n")
}

#[tokio::test]
async fn blade_expr_route_call_hover_and_definition() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_full_laravel_project(workspace.path());
    let blade = "<div>{{ route('home') }}</div>\n";
    std::fs::write(
        workspace.path().join("resources/views/page.blade.php"),
        blade,
    )
    .unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("resources/views/page.blade.php", blade).await;

    // Line 0, character 16 = inside "home".
    let hover_resp = s.hover("resources/views/page.blade.php", 0, 16).await;
    let hover_out = render_hover(&hover_resp);
    expect![[r#"
        **route('home')**

        ```php
        Route::get('/', HomeController::class)->name('home');
        ```

        `routes/web.php`"#]]
    .assert_eq(&hover_out);

    let def_resp = s.definition("resources/views/page.blade.php", 0, 16).await;
    let def_out = render_locations(&def_resp, &s.uri(""));
    expect!["routes/web.php:1:46-1:50"].assert_eq(&def_out);
}

#[tokio::test]
async fn blade_include_directive_definition_and_completion() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_full_laravel_project(workspace.path());
    let blade = "@include('layouts.app')\n";
    std::fs::write(
        workspace.path().join("resources/views/page.blade.php"),
        blade,
    )
    .unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("resources/views/page.blade.php", blade).await;

    // Line 0, character 15 = inside "layouts.app".
    let def_resp = s.definition("resources/views/page.blade.php", 0, 15).await;
    let def_out = render_locations(&def_resp, &s.uri(""));
    expect!["resources/views/layouts/app.blade.php:0:0-0:0"].assert_eq(&def_out);

    let partial = "@include('layouts.\n";
    s.change("resources/views/page.blade.php", 2, partial).await;
    // Line 0, character 18 = right after the trailing '.'.
    let comp_resp = s.completion("resources/views/page.blade.php", 0, 18).await;
    let comp_out = render_completion(&comp_resp);
    expect!["File        layouts.app"].assert_eq(&comp_out);
}

#[tokio::test]
async fn blade_anonymous_component_tag_resolves_to_view() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_full_laravel_project(workspace.path());
    let blade = "<x-alert />\n";
    std::fs::write(
        workspace.path().join("resources/views/page.blade.php"),
        blade,
    )
    .unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("resources/views/page.blade.php", blade).await;

    // Line 0, character 5 = inside "alert".
    let def_resp = s.definition("resources/views/page.blade.php", 0, 5).await;
    let def_out = render_locations(&def_resp, &s.uri(""));
    expect!["resources/views/components/alert.blade.php:0:0-0:0"].assert_eq(&def_out);

    let hover_resp = s.hover("resources/views/page.blade.php", 0, 5).await;
    let hover_out = render_hover(&hover_resp);
    expect![[r#"
        **<x-alert>**

        `resources/views/components/alert.blade.php`"#]]
    .assert_eq(&hover_out);
}

#[tokio::test]
async fn blade_component_tag_falls_back_to_class_when_no_view_exists() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_full_laravel_project(workspace.path());
    let blade = "<x-badge />\n";
    std::fs::write(
        workspace.path().join("resources/views/page.blade.php"),
        blade,
    )
    .unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("resources/views/page.blade.php", blade).await;

    // Line 0, character 5 = inside "badge".
    let def_resp = s.definition("resources/views/page.blade.php", 0, 5).await;
    let def_out = render_locations(&def_resp, &s.uri(""));
    expect!["app/View/Components/Badge.php:0:0-0:0"].assert_eq(&def_out);
}

#[tokio::test]
async fn blade_livewire_tag_and_directive_resolve_to_class() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_full_laravel_project(workspace.path());
    let blade_tag = "<livewire:counter />\n";
    std::fs::write(
        workspace.path().join("resources/views/page.blade.php"),
        blade_tag,
    )
    .unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("resources/views/page.blade.php", blade_tag).await;

    // Line 0, character 13 = inside "counter".
    let def_resp = s.definition("resources/views/page.blade.php", 0, 13).await;
    let def_out = render_locations(&def_resp, &s.uri(""));
    expect!["app/Livewire/Counter.php:0:0-0:0"].assert_eq(&def_out);

    let blade_directive = "@livewire('counter')\n";
    s.change("resources/views/page.blade.php", 2, blade_directive)
        .await;
    // Line 0, character 13 = inside "counter".
    let def_resp2 = s.definition("resources/views/page.blade.php", 0, 13).await;
    let def_out2 = render_locations(&def_resp2, &s.uri(""));
    expect!["app/Livewire/Counter.php:0:0-0:0"].assert_eq(&def_out2);
}

#[tokio::test]
async fn blade_component_tag_completion_lists_by_prefix() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_full_laravel_project(workspace.path());
    let blade = "<x-al\n";
    std::fs::write(
        workspace.path().join("resources/views/page.blade.php"),
        blade,
    )
    .unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("resources/views/page.blade.php", blade).await;

    // Line 0, character 5 = right after "al".
    let resp = s.completion("resources/views/page.blade.php", 0, 5).await;
    let out = render_completion(&resp);
    expect!["Class       alert"].assert_eq(&out);
}

#[tokio::test]
async fn blade_helper_call_completion_already_works_inside_double_brace() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_full_laravel_project(workspace.path());
    let blade = "{{ route('ho\n";
    std::fs::write(
        workspace.path().join("resources/views/page.blade.php"),
        blade,
    )
    .unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("resources/views/page.blade.php", blade).await;

    // Line 0, character 12 = right after "ho".
    let resp = s.completion("resources/views/page.blade.php", 0, 12).await;
    let out = render_completion(&resp);
    expect!["Reference   home"].assert_eq(&out);
}

#[tokio::test]
async fn blade_document_link_sweeps_expr_directive_and_tag() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_full_laravel_project(workspace.path());
    let blade = "{{ view('welcome') }}\n@include('layouts.app')\n<x-alert />\n<livewire:counter />\n";
    std::fs::write(
        workspace.path().join("resources/views/page.blade.php"),
        blade,
    )
    .unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("resources/views/page.blade.php", blade).await;

    let resp = s.document_link("resources/views/page.blade.php").await;
    assert!(resp["error"].is_null(), "error: {resp:?}");
    let out = render_document_links(&resp["result"], &s.uri(""));
    expect![[r#"
        0:9-16 target=resources/views/welcome.blade.php
        1:10-21 target=resources/views/layouts/app.blade.php
        2:3-8 target=resources/views/components/alert.blade.php
        3:10-17 target=app/Livewire/Counter.php"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn blade_resolution_none_outside_laravel_project() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    // No `artisan` marker — plain (non-Laravel) workspace.
    std::fs::create_dir_all(workspace.path().join("resources/views/components")).unwrap();
    std::fs::write(
        workspace
            .path()
            .join("resources/views/components/alert.blade.php"),
        "<div></div>\n",
    )
    .unwrap();
    let blade = "<x-alert />\n";
    std::fs::write(
        workspace.path().join("resources/views/page.blade.php"),
        blade,
    )
    .unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("resources/views/page.blade.php", blade).await;

    let def_resp = s.definition("resources/views/page.blade.php", 0, 5).await;
    let def_out = render_locations(&def_resp, &s.uri(""));
    expect!["<none>"].assert_eq(&def_out);
}
